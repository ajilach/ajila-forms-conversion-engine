//! Core blueprint processing pipeline (native only).

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
    use blueprint::{AemConfig, AemProfile, Blueprint, HtmlConfig, MergeInput, RecursiveMerger};
    use std::collections::HashMap;

    let mut state = ProcessingState::new();

    // ── Phase 1: Parsing all files ──────────────────────────────────
    state.step = ProcessingStep::Parsing;
    on_progress(&state);

    let mut blueprints = Vec::new();
    let mut any_xfa = false;
    for (filename, bytes) in files {
        let bp = match Blueprint::from_pdf_bytes(bytes) {
            Ok(bp) => bp,
            Err(e) => {
                state.error = Some(format!("Failed to parse {filename}: {e}"));
                on_progress(&state);
                return state;
            }
        };

        if bp.is_xfa() {
            any_xfa = true;
        }

        let language = bp.language().to_string();
        blueprints.push((filename.clone(), language, bp));
    }

    // ── Phase 2: Exhaustive Searching ───────────────────────────────
    state.step = ProcessingStep::ExhaustiveSearching;
    on_progress(&state);

    let mut parsed = Vec::new();
    for (filename, language, mut bp) in blueprints {
        let form_states = match bp.states() {
            Ok(s) => s,
            Err(e) => {
                state.error = Some(format!("Failed to explore states for {filename}: {e}"));
                on_progress(&state);
                return state;
            }
        };

        let context = bp.context();
        parsed.push((language, form_states, context));
    }

    // ── Phase 3: Flattening – render plain images for all files ─────
    state.step = ProcessingStep::Flattening;
    on_progress(&state);

    for (language, form_states, _) in &parsed {
        for (state_idx, form_state) in form_states.iter().enumerate() {
            let state_name = format!("{language}_{state_idx}");
            match form_state.render_plain(1.5) {
                Ok(img) => {
                    let mut png_bytes = Vec::new();
                    match encode_rgba_to_png(&img, &mut png_bytes) {
                        Ok(()) => {
                            let b64 = base64::prelude::BASE64_STANDARD.encode(&png_bytes);
                            state.plain_images.insert(state_name, b64);
                        }
                        Err(e) => {
                            state
                                .warnings
                                .push(format!("PNG encode failed for {state_name}: {e}"));
                        }
                    }
                }
                Err(e) => {
                    state
                        .warnings
                        .push(format!("render_plain failed for {state_name}: {e}"));
                }
            }
        }
    }
    on_progress(&state);

    // ── Phase 4: Structuring – labelled images & structured data ────
    state.step = ProcessingStep::Structuring;
    on_progress(&state);

    let mut all_envelopes = Vec::new();
    for (language, form_states, context) in &parsed {
        let mut structured_outputs = Vec::new();
        for (state_idx, form_state) in form_states.iter().enumerate() {
            let state_name = format!("{language}_{state_idx}");
            match form_state.render_labelled(1.5) {
                Ok(img) => {
                    let mut png_bytes = Vec::new();
                    match encode_rgba_to_png(&img, &mut png_bytes) {
                        Ok(()) => {
                            let b64 = base64::prelude::BASE64_STANDARD.encode(&png_bytes);
                            state.labelled_images.insert(state_name.clone(), b64);
                        }
                        Err(e) => {
                            state
                                .warnings
                                .push(format!("PNG encode failed for {state_name}: {e}"));
                        }
                    }
                }
                Err(e) => {
                    state
                        .warnings
                        .push(format!("render_labelled failed for {state_name}: {e}"));
                }
            }
            let envelope = form_state.structured(context.clone());
            structured_outputs.push((form_state.selections.clone(), envelope.content));
        }

        // Merge exhaustive states for this document
        if !structured_outputs.is_empty() {
            let merge_inputs: Vec<MergeInput> = structured_outputs
                .into_iter()
                .map(|(selections, nodes)| MergeInput::new(selections, nodes))
                .collect();

            let merger = RecursiveMerger::new(merge_inputs);
            let merged_states = merger.merge();

            let merged_envelope = blueprint::DocumentEnvelope {
                context: context.clone(),
                content: merged_states,
            };
            all_envelopes.push(merged_envelope);
        }
    }
    on_progress(&state);

    // ── Phase 5: Merging ────────────────────────────────────────────
    state.step = ProcessingStep::Merging;
    on_progress(&state);

    let merged = if all_envelopes.is_empty() {
        state.error = Some("No envelopes to merge".into());
        on_progress(&state);
        return state;
    } else if files.len() > 1 && all_envelopes.len() > 1 {
        match blueprint::merge_translations(all_envelopes) {
            Ok(m) => m,
            Err(e) => {
                state.error = Some(format!("Failed to merge translations: {e}"));
                on_progress(&state);
                return state;
            }
        }
    } else {
        all_envelopes.into_iter().next().unwrap()
    };

    let json = match serde_json::to_string_pretty(&merged) {
        Ok(j) => j,
        Err(e) => {
            state.error = Some(format!("Failed to serialize JSON: {e}"));
            on_progress(&state);
            return state;
        }
    };
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

    // AEM package generation requires XFA variables; skip for non-XFA PDFs
    if any_xfa {
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
                },
                HashMap::new(),
            )
        };
        match AemConfig::from_profile(&aem_profile, templates, &merged.context) {
            Ok(aem_config) => {
                let aem_zip = blueprint::to_aem_package(&merged.content, &aem_config);
                state.form_code = Some(aem_config.form_code.clone());
                state.aem_package = Some(aem_zip);
            }
            Err(_) => {
                // AEM config not available (e.g. missing XFA variables) — skip
            }
        }
    }

    state.step = ProcessingStep::Complete;
    state.merged_json = Some(json);
    state.html_preview = Some(html);
    on_progress(&state);

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
