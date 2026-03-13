//! Core blueprint processing pipeline (native only).
//!
//! Delegates the heavy lifting to [`blueprint::run_pipeline`] and translates
//! the resulting [`blueprint::PipelineEvent`]s into incremental updates to
//! the app-level [`ProcessingState`], including PNG-encoding renders and
//! generating HTML / AEM output.

use crate::models::{ProcessingState, ProcessingStep};

#[cfg(not(target_arch = "wasm32"))]
use base64::Engine;
#[cfg(not(target_arch = "wasm32"))]
use image::ImageEncoder;

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
pub fn run_blueprint_pipeline(
    files: &[(String, Vec<u8>)],
    profile: Option<String>,
    on_progress: impl Fn(&ProcessingState),
) -> ProcessingState {
    use blueprint::{
        AemConfig, AemProfile, HtmlConfig, PipelineConfig, PipelineEvent, PipelineStep as CoreStep,
        XsdConfig, XsdProfile, run_pipeline,
    };
    use std::collections::HashMap;

    let mut state = ProcessingState::new();
    let config = PipelineConfig::default();

    let result = run_pipeline(files, &config, |event| match event {
        PipelineEvent::StepChanged(step) => {
            // Don't forward Complete here — the app sets Complete only after
            // post-processing (JSON/HTML/AEM) finishes in the Ok(output) branch.
            state.step = match step {
                CoreStep::Parsing => ProcessingStep::Parsing,
                CoreStep::ExhaustiveSearching => ProcessingStep::ExhaustiveSearching,
                CoreStep::Flattening => ProcessingStep::Flattening,
                CoreStep::Structuring => ProcessingStep::Structuring,
                CoreStep::Merging => ProcessingStep::Merging,
                CoreStep::Complete => return,
            };
            on_progress(&state);
        }

        PipelineEvent::StatesFound { .. } => {
            // State counts are informational; step changes cover progress reporting.
        }

        PipelineEvent::PlainRender { label, image } => {
            let mut png_bytes = Vec::new();
            match encode_rgba_to_png(&image, &mut png_bytes) {
                Ok(()) => {
                    let b64 = base64::prelude::BASE64_STANDARD.encode(&png_bytes);
                    state.plain_images.insert(label.clone(), b64);
                }
                Err(e) => state
                    .warnings
                    .push(format!("PNG encode failed for {label}: {e}")),
            }
            on_progress(&state);
        }

        PipelineEvent::LabelledRender { label, image } => {
            let mut png_bytes = Vec::new();
            match encode_rgba_to_png(&image, &mut png_bytes) {
                Ok(()) => {
                    let b64 = base64::prelude::BASE64_STANDARD.encode(&png_bytes);
                    state.labelled_images.insert(label.clone(), b64);
                }
                Err(e) => state
                    .warnings
                    .push(format!("PNG encode failed for {label}: {e}")),
            }
            on_progress(&state);
        }

        // The app does not request annotated renders (PipelineConfig::render_annotated
        // is false by default), but we must match all variants.
        PipelineEvent::AnnotatedRender { .. } => {}

        PipelineEvent::Warning(msg) => {
            state.warnings.push(msg);
        }
    });

    match result {
        Ok(output) => {
            let merged = output.merged;

            let json = match serde_json::to_string_pretty(&merged) {
                Ok(j) => j,
                Err(e) => {
                    state.error = Some(format!("Failed to serialize JSON: {e}"));
                    on_progress(&state);
                    return state;
                }
            };

            // Generate HTML, applying a profile's custom styles when provided.
            let html = {
                let html_config = if let Some(ref profile_name) = profile {
                    match crate::profiles::load_html_custom_styles(profile_name) {
                        Ok(Some(styles)) => HtmlConfig {
                            custom_styles: Some(styles),
                            ..HtmlConfig::default()
                        },
                        Ok(None) => HtmlConfig::default(),
                        Err(e) => {
                            state
                                .warnings
                                .push(format!("Failed to load HTML profile: {e}"));
                            HtmlConfig::default()
                        }
                    }
                } else {
                    HtmlConfig::default()
                };
                blueprint::to_html(&merged.content, &html_config)
            };

            // AEM package generation — load profile config when provided, fall back
            // to empty defaults otherwise.  Fails gracefully for non-XFA PDFs.
            let (aem_profile, templates) = if let Some(ref profile_name) = profile {
                match crate::profiles::load_aem_profile(profile_name) {
                    Ok((p, t)) => (p, t),
                    Err(e) => {
                        state
                            .warnings
                            .push(format!("Failed to load AEM profile: {e}"));
                        (
                            AemProfile {
                                master_language: None,
                                title: None,
                                form_path: None,
                                form_dir: None,
                                variables: HashMap::new(),
                                language_synonyms: HashMap::new(),
                                bind_to_xsd: None,
                                use_fragments: None,
                                fragment_xsd_ref: None,
                                fragment_ref_prefix: None,
                            },
                            HashMap::new(),
                        )
                    }
                }
            } else {
                (
                    AemProfile {
                        master_language: None,
                        title: None,
                        form_path: None,
                        form_dir: None,
                        variables: HashMap::new(),
                        language_synonyms: HashMap::new(),
                        bind_to_xsd: None,
                        use_fragments: None,
                        fragment_xsd_ref: None,
                        fragment_ref_prefix: None,
                    },
                    HashMap::new(),
                )
            };
            if let Ok(mut aem_config) =
                AemConfig::from_profile(&aem_profile, templates, &merged.context)
            {
                if aem_config.bind_to_xsd {
                    if let Some(ref profile_name) = profile {
                        match crate::profiles::load_xsd_config(profile_name) {
                            Ok(xsd_cfg) => {
                                aem_config.xsd_config = Some(xsd_cfg);
                            }
                            Err(e) => {
                                state
                                    .warnings
                                    .push(format!("Failed to load XSD config for AEM bind: {e}"));
                            }
                        }
                    }
                }
                let aem_zip = blueprint::to_aem_package(&merged.content, &aem_config);
                state.form_code = Some(aem_config.form_code.clone());
                state.aem_package = Some(aem_zip);
            }

            // XSD schema generation
            let xsd_config = if let Some(ref profile_name) = profile {
                match crate::profiles::load_xsd_config(profile_name) {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        state
                            .warnings
                            .push(format!("Failed to load XSD profile: {e}"));
                        XsdConfig::from_profile(XsdProfile::default())
                    }
                }
            } else {
                XsdConfig::from_profile(XsdProfile::default())
            };
            let xsd = blueprint::to_xsd(&merged.content, &xsd_config);
            state.xsd_schema = Some(xsd);

            state.step = ProcessingStep::Complete;
            state.merged_json = Some(json);
            state.html_preview = Some(html);
            on_progress(&state);
        }
        Err(e) => {
            state.error = Some(format!("{e}"));
            on_progress(&state);
        }
    }

    state
}

/// Encode an RGBA image to PNG bytes.
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
pub fn encode_rgba_to_png(img: &blueprint::RgbaImage, output: &mut Vec<u8>) -> Result<(), String> {
    use image::ExtendedColorType;
    use image::codecs::png::PngEncoder;

    let (width, height) = img.dimensions();
    let encoder = PngEncoder::new(output);

    encoder
        .write_image(img.as_raw(), width, height, ExtendedColorType::Rgba8)
        .map_err(|e| format!("PNG encoding error: {}", e))
}
