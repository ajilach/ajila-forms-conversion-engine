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
    on_progress: impl Fn(&ProcessingState),
) -> ProcessingState {
    use blueprint::{
        AemConfig, AemProfile, HtmlConfig, PipelineConfig, PipelineEvent,
        PipelineStep as CoreStep, run_pipeline,
    };
    use std::collections::HashMap;

    let mut state = ProcessingState::new();
    let config = PipelineConfig::default();

    let result = run_pipeline(files, &config, |event| match event {
        PipelineEvent::StepChanged(step) => {
            state.step = match step {
                CoreStep::Parsing => ProcessingStep::Parsing,
                CoreStep::ExhaustiveSearching => ProcessingStep::ExhaustiveSearching,
                CoreStep::Flattening => ProcessingStep::Flattening,
                CoreStep::Structuring => ProcessingStep::Structuring,
                CoreStep::Merging => ProcessingStep::Merging,
                CoreStep::Complete => ProcessingStep::Complete,
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

            let html = blueprint::to_html(&merged.content, &HtmlConfig::default());

            // AEM package generation — fails gracefully for non-XFA PDFs.
            let profile = AemProfile {
                master_language: None,
                title: None,
                form_path: None,
                form_dir: None,
                variables: HashMap::new(),
                language_synonyms: HashMap::new(),
            };
            if let Ok(aem_config) =
                AemConfig::from_profile(&profile, HashMap::new(), &merged.context)
            {
                let aem_zip = blueprint::to_aem_package(&merged.content, &aem_config);
                state.form_code = Some(aem_config.form_code.clone());
                state.aem_package = Some(aem_zip);
            }

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

