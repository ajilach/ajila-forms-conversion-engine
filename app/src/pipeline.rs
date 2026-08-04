//! Core blueprint processing pipeline (app wrapper).
//!
//! Provides [`run_blueprint_pipeline`] which delegates to the core
//! [`blueprint::run_pipeline`] on a blocking thread and streams progress
//! updates back via a channel so the webview event loop is never starved.

use crate::models::{ProcessingState, ProcessingStep};

use base64::Engine;

/// JPEG quality for plain page renders attached to AI requests / shown in the
/// UI. Balances legibility against payload size.
const PLAIN_JPEG_QUALITY: u8 = 82;

/// Run the full blueprint pipeline asynchronously.
///
/// On native targets the heavy computation runs on a blocking thread
/// (`tokio::task::spawn_blocking`) so the webview connection stays alive.
/// Progress updates are streamed through a channel and applied on the async
/// side between awaits.
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
                    PipelineEvent::PlainRender { label, images } => {
                        // JPEG-compress plain renders (one per page): far smaller
                        // than PNG for page renders, keeping the Smart Edit request
                        // (which attaches these) within provider size limits.
                        match encode_plain_pages(&images) {
                            Ok(pages) => {
                                state.plain_images.insert(label, pages);
                            }
                            Err(e) => state
                                .warnings
                                .push(format!("JPEG encode failed for {label}: {e}")),
                        }
                        on_progress(&state);
                    }
                    PipelineEvent::LabelledRender { label, images } => {
                        match encode_labelled_pages(&images) {
                            Ok(pages) => {
                                state.labelled_images.insert(label, pages);
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
                        PipelineEvent::PlainRender { label, images } => {
                            if let Ok(pages) = encode_plain_pages(&images) {
                                state.plain_images.insert(label, pages);
                            }
                        }
                        PipelineEvent::LabelledRender { label, images } => {
                            if let Ok(pages) = encode_labelled_pages(&images) {
                                state.labelled_images.insert(label, pages);
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
                    let html = blueprint::to_html(&merged.content, &html_config);
                    state.html_preview = Some(html);
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

                state.step_progress = Some(0.95);
                on_progress(&state);

                if let Some(ref profile_name) = profile
                    && blueprint::has_redacto_config(profile_name)
                    && !merged.context.variables.is_empty()
                {
                    let redacto_config =
                        match blueprint::load_redacto_config(profile_name, &merged.context) {
                            Ok(cfg) => cfg,
                            Err(e) => fail!(format!("Failed to load Redacto profile: {e}")),
                        };
                    state.redacto_sql =
                        Some(blueprint::to_redacto_sql(&merged.content, &redacto_config));
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

// Image encoding lives in the headless `agent` crate; re-export so the
// historical `crate::pipeline::encode_*` paths keep working across the app.
pub use agent::image_encode::{encode_rgba_to_jpeg, encode_rgba_to_png};

/// Encode per-page plain renders to base64 JPEG strings (page order preserved).
fn encode_plain_pages(images: &[std::sync::Arc<blueprint::RgbaImage>]) -> Result<Vec<String>, String> {
    images
        .iter()
        .map(|img| {
            encode_rgba_to_jpeg(img, PLAIN_JPEG_QUALITY)
                .map(|jpeg| base64::prelude::BASE64_STANDARD.encode(&jpeg))
        })
        .collect()
}

/// Encode per-page labelled renders to base64 PNG strings (page order preserved).
fn encode_labelled_pages(images: &[std::sync::Arc<blueprint::RgbaImage>]) -> Result<Vec<String>, String> {
    images
        .iter()
        .map(|img| {
            let mut png_bytes = Vec::new();
            encode_rgba_to_png(img, &mut png_bytes)
                .map(|()| base64::prelude::BASE64_STANDARD.encode(&png_bytes))
        })
        .collect()
}
