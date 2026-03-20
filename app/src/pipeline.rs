//! Core blueprint processing pipeline.
//!
//! Delegates the heavy lifting to [`blueprint::run_pipeline`] and translates
//! the resulting [`blueprint::PipelineEvent`]s into incremental updates to
//! the app-level [`ProcessingState`], including PNG-encoding renders and
//! generating HTML / AEM output.

use crate::models::{ProcessingState, ProcessingStep};

use base64::Engine;
use image::ImageEncoder;

#[allow(dead_code)]
pub fn run_blueprint_pipeline(
    files: &[(String, Vec<u8>)],
    profile: Option<String>,
    on_progress: impl Fn(&ProcessingState),
) -> ProcessingState {
    use blueprint::{
        HtmlConfig, PipelineConfig, PipelineEvent, PipelineStep as CoreStep, run_pipeline,
    };

    let mut state = ProcessingState::new();
    let config = PipelineConfig::default();

    // Load profile fonts before running the pipeline so the font manager
    // has the right typefaces available during PDF parsing.
    if let Some(ref profile_name) = profile {
        if let Err(e) = blueprint::load_profile_fonts(profile_name) {
            state
                .warnings
                .push(format!("Failed to load profile fonts: {e}"));
        }
    }

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

            // Generate HTML only when an explicit html/config.toml exists.
            if let Some(ref profile_name) = profile {
                if blueprint::has_html_config(profile_name) {
                    let html_config = match blueprint::load_html_custom_styles(profile_name) {
                        Ok(styles) => HtmlConfig {
                            custom_styles: Some(styles),
                            ..HtmlConfig::default()
                        },
                        Err(e) => {
                            state.error = Some(format!("Failed to load HTML profile: {e}"));
                            on_progress(&state);
                            return state;
                        }
                    };
                    state.html_preview = Some(blueprint::to_html(&merged.content, &html_config));
                }
            }

            // AEM package generation requires an explicit aem/config.toml.
            if let Some(ref profile_name) = profile
                && blueprint::has_aem_config(profile_name)
            {
                let aem_config = match blueprint::load_aem_config(profile_name, &merged.context) {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        state.error = Some(format!("Failed to load AEM profile: {e}"));
                        on_progress(&state);
                        return state;
                    }
                };
                let aem_zip = blueprint::to_aem_package(&merged.content, &aem_config);
                state.form_code = Some(aem_config.form_code.clone());
                state.aem_package = Some(aem_zip);
            }

            // XSD schema generation requires an explicit xsd/config.toml.
            if let Some(ref profile_name) = profile
                && blueprint::has_xsd_config(profile_name)
            {
                let xsd_config = match blueprint::load_xsd_config(profile_name) {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        state.error = Some(format!("Failed to load XSD profile: {e}"));
                        on_progress(&state);
                        return state;
                    }
                };
                state.xsd_schema = Some(blueprint::to_xsd(&merged.content, &xsd_config));
            }

            state.step = ProcessingStep::Complete;
            state.merged_json = Some(json);
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
