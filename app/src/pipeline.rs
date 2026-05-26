//! Core blueprint processing pipeline (app wrapper).
//!
//! Provides [`run_blueprint_pipeline`] which delegates to the core
//! [`blueprint::run_pipeline`] on a blocking thread and streams progress
//! updates back via a channel so the webview event loop is never starved.

use crate::models::{ProcessingState, ProcessingStep};

use base64::Engine;
use image::ImageEncoder;

/// Run the full blueprint pipeline asynchronously.
///
/// On native targets the heavy computation runs on a blocking thread
/// (`tokio::task::spawn_blocking`) so the webview connection stays alive.
/// Progress updates are streamed through a channel and applied on the async
/// side between awaits.
#[allow(dead_code)]
pub async fn run_blueprint_pipeline(
    files: &[(String, Vec<u8>)],
    profile: Option<String>,
    mut on_progress: impl FnMut(&ProcessingState),
) {
    use blueprint::{HtmlConfig, PipelineConfig, PipelineEvent, PipelineStep as CoreStep};

    let mut state = ProcessingState::new();

    // Load profile fonts before running the pipeline so the font manager
    // has the right typefaces available during PDF parsing.
    if let Some(ref profile_name) = profile
        && let Err(e) = blueprint::load_profile_fonts(profile_name)
    {
        state
            .warnings
            .push(format!("Failed to load profile fonts: {e}"));
    }

    // Helper: report an error and return early.
    macro_rules! fail {
        ($msg:expr) => {{
            state.error = Some($msg);
            on_progress(&state);
            return;
        }};
    }

    state.step = ProcessingStep::Parsing;
    state.step_progress = Some(0.0);
    on_progress(&state);

    let config = PipelineConfig::default();

    // Run the core pipeline on a blocking thread so the webview stays responsive.
    let files_owned: Vec<(String, Vec<u8>)> = files.to_vec();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PipelineEvent>();

    let mut pipeline_handle = tokio::task::spawn_blocking(move || {
        blueprint::run_pipeline(&files_owned, &config, |event| {
            let _ = tx.send(event);
        })
    });

    // Drain progress events while the pipeline runs.
    // `spawn_blocking` runs on a separate thread; we poll events here to
    // keep the UI updated without blocking the webview.
    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                match event {
                    PipelineEvent::StepChanged(step) => {
                        state.step = match step {
                            CoreStep::Parsing => ProcessingStep::Parsing,
                            CoreStep::ExhaustiveSearching => ProcessingStep::ExhaustiveSearching,
                            CoreStep::Flattening => ProcessingStep::Flattening,
                            CoreStep::Structuring => ProcessingStep::Structuring,
                            CoreStep::Merging => ProcessingStep::Merging,
                            CoreStep::Complete => ProcessingStep::Complete,
                        };
                        state.step_progress = Some(0.0);
                        on_progress(&state);
                    }
                    PipelineEvent::StatesFound { .. } => {}
                    PipelineEvent::PlainRender { label, image } => {
                        let mut png_bytes = Vec::new();
                        match encode_rgba_to_png(&image, &mut png_bytes) {
                            Ok(()) => {
                                let b64 = base64::prelude::BASE64_STANDARD.encode(&png_bytes);
                                state.plain_images.insert(label, b64);
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
                                state.labelled_images.insert(label, b64);
                            }
                            Err(e) => state
                                .warnings
                                .push(format!("PNG encode failed for {label}: {e}")),
                        }
                        on_progress(&state);
                    }
                    PipelineEvent::AnnotatedRender { .. } => {}
                    PipelineEvent::Warning(msg) => {
                        state.warnings.push(msg);
                        on_progress(&state);
                    }
                }
            }
            result = &mut pipeline_handle => {
                // Pipeline finished — drain any remaining events
                while let Ok(event) = rx.try_recv() {
                    match event {
                        PipelineEvent::PlainRender { label, image } => {
                            let mut png_bytes = Vec::new();
                            if let Ok(()) = encode_rgba_to_png(&image, &mut png_bytes) {
                                let b64 = base64::prelude::BASE64_STANDARD.encode(&png_bytes);
                                state.plain_images.insert(label, b64);
                            }
                        }
                        PipelineEvent::LabelledRender { label, image } => {
                            let mut png_bytes = Vec::new();
                            if let Ok(()) = encode_rgba_to_png(&image, &mut png_bytes) {
                                let b64 = base64::prelude::BASE64_STANDARD.encode(&png_bytes);
                                state.labelled_images.insert(label, b64);
                            }
                        }
                        _ => {}
                    }
                }
                on_progress(&state);

                // Handle pipeline result
                let output = match result {
                    Ok(Ok(o)) => o,
                    Ok(Err(e)) => fail!(format!("{e}")),
                    Err(e) => fail!(format!("Pipeline thread panicked: {e}")),
                };

                // ── Post-processing ──────────────────────────────────────
                let merged = output.merged;

                state.envelope = Some(merged.clone());
                state.step_progress = Some(0.6);
                on_progress(&state);

                let json = match serde_json::to_string_pretty(&merged) {
                    Ok(j) => j,
                    Err(e) => fail!(format!("Failed to serialize JSON: {e}")),
                };

                state.step_progress = Some(0.7);
                on_progress(&state);

                if let Some(ref profile_name) = profile
                    && blueprint::has_html_config(profile_name)
                {
                    let html_config = match blueprint::load_html_custom_styles(profile_name) {
                        Ok(styles) => HtmlConfig {
                            custom_styles: Some(styles),
                            ..HtmlConfig::default()
                        },
                        Err(e) => fail!(format!("Failed to load HTML profile: {e}")),
                    };
                    state.html_preview = Some(blueprint::to_html(&merged.content, &html_config));
                }

                state.step_progress = Some(0.8);
                on_progress(&state);

                if let Some(ref profile_name) = profile
                    && blueprint::has_aem_config(profile_name)
                    && !merged.context.variables.is_empty()
                {
                    let aem_config =
                        match blueprint::load_aem_config(profile_name, &merged.context) {
                            Ok(cfg) => cfg,
                            Err(e) => fail!(format!("Failed to load AEM profile: {e}")),
                        };
                    let aem_zip = blueprint::to_aem_package(&merged.content, &aem_config);
                    state.form_code = Some(aem_config.form_code.clone());
                    state.aem_package = Some(aem_zip);
                }

                state.step_progress = Some(0.9);
                on_progress(&state);

                if let Some(ref profile_name) = profile
                    && blueprint::has_xsd_config(profile_name)
                    && !merged.context.variables.is_empty()
                {
                    let mut xsd_config = match blueprint::load_xsd_config(profile_name) {
                        Ok(cfg) => cfg,
                        Err(e) => fail!(format!("Failed to load XSD profile: {e}")),
                    };
                    if let Some(ref fc) = state.form_code {
                        xsd_config.form_code = Some(fc.clone());
                    }
                    state.xsd_schema = Some(blueprint::to_xsd(&merged.content, &xsd_config));
                }

                state.step_progress = Some(1.0);
                on_progress(&state);

                state.step = ProcessingStep::Complete;
                state.step_progress = None;
                state.merged_json = Some(json);
                on_progress(&state);
                break;
            }
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

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
