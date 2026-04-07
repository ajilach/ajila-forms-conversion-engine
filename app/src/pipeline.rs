//! Core blueprint processing pipeline.
//!
//! Provides a single async entry point, [`run_blueprint_pipeline`], that drives
//! each phase of the pipeline individually and yields between phases so the
//! caller can reflect incremental progress (UI re-render, session store update,
//! etc.).

use crate::models::{ProcessingState, ProcessingStep};
use crate::platform::async_sleep_ms;

use base64::Engine;
use image::ImageEncoder;

/// Run the full blueprint pipeline asynchronously.
///
/// Between each pipeline phase the `on_progress` callback is invoked with the
/// current [`ProcessingState`] and an async yield gives the runtime a chance to
/// process pending events (browser re-render on WASM, task scheduling on
/// native).
#[allow(dead_code)]
pub async fn run_blueprint_pipeline(
    files: &[(String, Vec<u8>)],
    profile: Option<String>,
    mut on_progress: impl FnMut(&ProcessingState),
) {
    use blueprint::{
        Blueprint, Context, DocumentEnvelope, HtmlConfig, MergeInput, RecursiveMerger, Selection,
        StateMap, merge_translations,
    };
    use std::collections::{BTreeSet, HashMap};

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

    // ── Phase 1: Parsing ─────────────────────────────────────────────────
    state.step = ProcessingStep::Parsing;
    state.step_progress = Some(0.0);
    on_progress(&state);
    async_sleep_ms(0).await;

    let total_files = files.len();
    let mut blueprints: Vec<(String, String, Blueprint)> = Vec::new();
    for (i, (filename, bytes)) in files.iter().enumerate() {
        match Blueprint::from_pdf_bytes(bytes) {
            Ok(bp) => {
                let language = bp.language().to_string();
                blueprints.push((filename.clone(), language, bp));
            }
            Err(e) => fail!(format!("{e}")),
        }
        state.step_progress = Some((i + 1) as f32 / total_files as f32);
        on_progress(&state);
        async_sleep_ms(0).await;
    }

    // ── Phase 2: Exhaustive exploration ──────────────────────────────────
    state.step = ProcessingStep::ExhaustiveSearching;
    state.step_progress = Some(0.0);
    on_progress(&state);
    async_sleep_ms(0).await;

    let total_blueprints = blueprints.len();
    let mut explored: Vec<(String, String, blueprint::FormStates, Context)> = Vec::new();
    for (i, (filename, language, mut bp)) in blueprints.into_iter().enumerate() {
        match bp.states() {
            Ok(form_states) => {
                let context = bp.context();
                explored.push((filename, language, form_states, context));
            }
            Err(e) => fail!(format!("{e}")),
        }
        state.step_progress = Some((i + 1) as f32 / total_blueprints as f32);
        on_progress(&state);
        async_sleep_ms(0).await;
    }

    // ── Phase 3: Flattening ──────────────────────────────────────────────
    state.step = ProcessingStep::Flattening;
    state.step_progress = Some(0.0);
    on_progress(&state);
    async_sleep_ms(0).await;

    let config = blueprint::PipelineConfig::default();

    if config.render_plain {
        let total_plain: usize = explored.iter().map(|(_, _, fs, _)| fs.len()).sum();
        let mut done_plain: usize = 0;
        for (_filename, language, form_states, _context) in &explored {
            for (state_idx, form_state) in form_states.iter().enumerate() {
                let label = format!("{}_{}", language, state_idx);
                match form_state.render_plain(config.scale) {
                    Ok(image) => {
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
                    }
                    Err(e) => state
                        .warnings
                        .push(format!("Plain render failed for {label}: {e}")),
                }
                done_plain += 1;
                state.step_progress = Some(done_plain as f32 / total_plain as f32);
                on_progress(&state);
                async_sleep_ms(0).await;
            }
        }
    }

    // ── Phase 4: Structuring ─────────────────────────────────────────────
    state.step = ProcessingStep::Structuring;
    state.step_progress = Some(0.0);
    on_progress(&state);
    async_sleep_ms(0).await;

    let mut per_language_state_maps: Vec<(String, StateMap)> = Vec::new();

    let total_structuring: usize = explored.iter().map(|(_, _, fs, _)| fs.len()).sum();
    let mut done_structuring: usize = 0;

    for (_filename, language, form_states, context) in &explored {
        let mut state_map: StateMap = HashMap::new();

        for (state_idx, form_state) in form_states.iter().enumerate() {
            let label = format!("{}_{}", language, state_idx);

            if config.render_labelled {
                match form_state.render_labelled(config.scale) {
                    Ok(image) => {
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
                    }
                    Err(e) => state
                        .warnings
                        .push(format!("Labelled render failed for {label}: {e}")),
                }
            }

            let envelope = form_state.structured(context.clone());
            let signature = selection_signature(&form_state.selections);

            if state_map
                .insert(signature.clone(), (form_state.selections.clone(), envelope))
                .is_some()
            {
                fail!(format!(
                    "Duplicate state signature '{signature}' found in language '{language}'"
                ));
            }

            done_structuring += 1;
            state.step_progress = Some(done_structuring as f32 / total_structuring as f32);
            on_progress(&state);
            async_sleep_ms(0).await;
        }

        per_language_state_maps.push((language.clone(), state_map));
    }

    // ── Phase 5: Merging ─────────────────────────────────────────────────
    state.step = ProcessingStep::Merging;
    state.step_progress = Some(0.0);
    on_progress(&state);
    async_sleep_ms(0).await;

    if per_language_state_maps.is_empty() {
        fail!("No envelopes to merge".into());
    }

    let mut expected_signatures = BTreeSet::new();
    if let Some((_, first_map)) = per_language_state_maps.first() {
        expected_signatures.extend(first_map.keys().cloned());
    }

    for (language, state_map) in per_language_state_maps.iter().skip(1) {
        let signatures: BTreeSet<String> = state_map.keys().cloned().collect();
        if signatures != expected_signatures {
            let missing: Vec<String> = expected_signatures
                .difference(&signatures)
                .cloned()
                .collect();
            let extra: Vec<String> = signatures
                .difference(&expected_signatures)
                .cloned()
                .collect();
            fail!(format!(
                "State signature mismatch for language '{language}'. Missing: [{}], Extra: [{}]",
                missing.join(", "),
                extra.join(", ")
            ));
        }
    }

    let total_signatures = expected_signatures.len();
    let mut translated_states: Vec<(Vec<Selection>, DocumentEnvelope)> = Vec::new();
    for (i, signature) in expected_signatures.iter().enumerate() {
        let mut canonical_selections: Option<Vec<Selection>> = None;
        let mut state_envelopes: Vec<DocumentEnvelope> = Vec::new();

        for (language, state_map) in &per_language_state_maps {
            let Some((selections, envelope)) = state_map.get(signature) else {
                fail!(format!(
                    "State signature '{signature}' missing for language '{language}'"
                ));
            };

            if let Some(existing) = &canonical_selections {
                if !selections_match(existing, selections) {
                    fail!(format!(
                        "Selection mismatch for state signature '{signature}' between languages"
                    ));
                }
            } else {
                canonical_selections = Some(selections.clone());
            }

            state_envelopes.push(envelope.clone());
        }

        let merged_state = if state_envelopes.len() > 1 {
            match merge_translations(state_envelopes, None) {
                Ok(m) => m,
                Err(e) => fail!(format!("{e}")),
            }
        } else {
            state_envelopes.into_iter().next().unwrap()
        };

        translated_states.push((canonical_selections.unwrap_or_default(), merged_state));

        state.step_progress = Some((i + 1) as f32 / total_signatures as f32 * 0.5);
        on_progress(&state);
        async_sleep_ms(0).await;
    }

    if translated_states.is_empty() {
        fail!("No translated states to merge".into());
    }

    let merged_context = translated_states[0].1.context.clone();

    let merge_inputs: Vec<MergeInput> = translated_states
        .iter()
        .map(|(selections, envelope)| MergeInput::new(selections.clone(), envelope.content.clone()))
        .collect();
    let merged_content = if merge_inputs.is_empty() {
        Vec::new()
    } else {
        RecursiveMerger::new(merge_inputs).merge()
    };

    let merged = DocumentEnvelope {
        context: merged_context,
        content: merged_content,
        state_count: translated_states.len(),
    };
    // Store the envelope for the editor
    state.envelope = Some(merged.clone());
    state.step_progress = Some(0.6);
    on_progress(&state);
    async_sleep_ms(0).await;

    // ── Post-processing ──────────────────────────────────────────────────
    let json = match serde_json::to_string_pretty(&merged) {
        Ok(j) => j,
        Err(e) => fail!(format!("Failed to serialize JSON: {e}")),
    };

    state.step_progress = Some(0.7);
    on_progress(&state);
    async_sleep_ms(0).await;

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
    async_sleep_ms(0).await;

    if let Some(ref profile_name) = profile
        && blueprint::has_aem_config(profile_name)
    {
        let aem_config = match blueprint::load_aem_config(profile_name, &merged.context) {
            Ok(cfg) => cfg,
            Err(e) => fail!(format!("Failed to load AEM profile: {e}")),
        };
        let aem_zip = blueprint::to_aem_package(&merged.content, &aem_config);
        state.form_code = Some(aem_config.form_code.clone());
        state.aem_package = Some(aem_zip);
    }

    state.step_progress = Some(0.9);
    on_progress(&state);
    async_sleep_ms(0).await;

    if let Some(ref profile_name) = profile
        && blueprint::has_xsd_config(profile_name)
    {
        let xsd_config = match blueprint::load_xsd_config(profile_name) {
            Ok(cfg) => cfg,
            Err(e) => fail!(format!("Failed to load XSD profile: {e}")),
        };
        state.xsd_schema = Some(blueprint::to_xsd(&merged.content, &xsd_config));
    }

    state.step_progress = Some(1.0);
    on_progress(&state);
    async_sleep_ms(0).await;

    state.step = ProcessingStep::Complete;
    state.step_progress = None;
    state.merged_json = Some(json);
    on_progress(&state);
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

fn selection_signature(selections: &[blueprint::Selection]) -> String {
    selections
        .iter()
        .map(|s| {
            let kind = match s.kind {
                blueprint::SelectionKind::Radio => "radio",
                blueprint::SelectionKind::Checkbox => "checkbox",
                blueprint::SelectionKind::Dropdown => "dropdown",
            };
            format!("{}|{}|{}", s.condition_path(), kind, s.option_index)
        })
        .collect::<Vec<_>>()
        .join("->")
}

fn selections_match(a: &[blueprint::Selection], b: &[blueprint::Selection]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(left, right)| {
            left.condition_path() == right.condition_path()
                && left.kind == right.kind
                && left.option_index == right.option_index
        })
}
