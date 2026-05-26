//! High-level conversion pipeline with incremental progress reporting.
//!
//! This module provides a single entry point, [`run_pipeline`], that drives the
//! full Blueprint processing pipeline from raw PDF bytes to a merged
//! [`DocumentEnvelope`] and optional renders.  Progress is reported
//! incrementally through a caller-supplied callback so that UIs and CLIs can
//! update as work completes rather than waiting for the entire pipeline to
//! finish.
//!
//! # Pipeline steps
//!
//! 1. **Parsing** — PDF bytes are decoded and the XFA / AcroForm structure is
//!    extracted to produce a [`Blueprint`].
//! 2. **ExhaustiveSearching** — All reachable form states (combinations of
//!    radio buttons, checkboxes, and dropdowns) are discovered.
//! 3. **Flattening** — Each state is rendered to a plain
//!    ([`PipelineEvent::PlainRender`]) image as soon as it is ready.
//! 4. **Structuring** — Each state is converted to a structured node tree and
//!    optionally rendered with analysis overlays
//!    ([`PipelineEvent::LabelledRender`]).
//! 5. **Merging** — Per-state trees are merged into a single
//!    [`DocumentEnvelope`]; if multiple files were provided their envelopes are
//!    merged as language variants.
//!
//! # Usage
//!
//! ```rust,ignore
//! use blueprint::pipeline::{PipelineConfig, PipelineEvent, run_pipeline};
//!
//! let files = vec![("form.pdf".into(), pdf_bytes)];
//! let config = PipelineConfig::default();
//!
//! let output = run_pipeline(&files, &config, |event| {
//!     match event {
//!         PipelineEvent::StepChanged(step) => eprintln!("[{step:?}]"),
//!         PipelineEvent::StatesFound { file, count } =>
//!             eprintln!("{file}: {count} states"),
//!         PipelineEvent::PlainRender { label, .. } =>
//!             eprintln!("plain render ready: {label}"),
//!         PipelineEvent::LabelledRender { label, .. } =>
//!             eprintln!("labelled render ready: {label}"),
//!         PipelineEvent::Warning(msg) =>
//!             eprintln!("warning: {msg}"),
//!     }
//! })?;
//!
//! println!("Merged {} nodes", output.merged.content.len());
//! # Ok::<(), blueprint::Error>(())
//! ```

use std::sync::Arc;
use std::{collections::BTreeSet, collections::HashMap};

use crate::{Blueprint, Context, DocumentEnvelope, Error, RgbaImage, Selection};

/// Per-state map from label to its selections and structured envelope.
pub type StateMap = HashMap<String, (Vec<Selection>, DocumentEnvelope)>;

// ============================================================================
// PipelineStep
// ============================================================================

/// Current step of the conversion pipeline.
///
/// Steps are emitted in order via [`PipelineEvent::StepChanged`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineStep {
    /// PDF bytes are being decoded and the XFA / AcroForm structure extracted.
    Parsing,
    /// All reachable form states are being discovered through exhaustive
    /// exploration of radio buttons, checkboxes, and dropdowns.
    ExhaustiveSearching,
    /// Each form state is being rendered to a plain (content-only) image.
    Flattening,
    /// Each form state is being converted to a structured node tree and
    /// optionally rendered with analysis overlays.
    Structuring,
    /// Per-state node trees are being merged into a single document envelope.
    Merging,
    /// The pipeline finished successfully.
    Complete,
}

// ============================================================================
// PipelineEvent
// ============================================================================

/// An event emitted during pipeline execution.
///
/// Pass a closure to [`run_pipeline`] to receive these events.  The closure is
/// called synchronously from the processing thread, so you can react to each
/// event immediately (e.g. send it through a channel to a UI thread).
///
/// Images are wrapped in [`Arc`] so they can be shared cheaply between the
/// event callback and the final [`PipelineOutput`] without duplicating the
/// pixel data.
#[derive(Debug)]
pub enum PipelineEvent {
    /// The pipeline advanced to a new processing step.
    StepChanged(PipelineStep),

    /// Exhaustive exploration completed for one file.
    ///
    /// `file` is the filename passed to [`run_pipeline`].
    /// `count` is the number of unique states discovered.
    StatesFound {
        /// Filename of the processed file.
        file: String,
        /// Number of unique form states discovered.
        count: usize,
    },

    /// A plain (content-only) render completed for one form state.
    ///
    /// `label` is `"{language}_{state_index}"` (e.g. `"de_0"`, `"en_1"`).
    PlainRender {
        /// State identifier (e.g. `"de_0"`).
        label: String,
        /// The rendered image.
        image: Arc<RgbaImage>,
    },

    /// A labelled (analysis-overlay) render completed for one form state.
    ///
    /// `label` is the same identifier as in the corresponding
    /// [`PlainRender`](PipelineEvent::PlainRender).
    LabelledRender {
        /// State identifier (e.g. `"de_0"`).
        label: String,
        /// The rendered image.
        image: Arc<RgbaImage>,
    },

    /// An annotated (red field-level debug annotations) render completed for
    /// one form state.
    ///
    /// `label` is the same identifier as in the corresponding
    /// [`PlainRender`](PipelineEvent::PlainRender).
    AnnotatedRender {
        /// State identifier (e.g. `"de_0"`).
        label: String,
        /// The rendered image.
        image: Arc<RgbaImage>,
    },

    /// A non-fatal warning occurred.
    ///
    /// Processing continues even when warnings are emitted.
    Warning(String),
}

// ============================================================================
// PipelineConfig
// ============================================================================

/// Configuration for [`run_pipeline`].
#[derive(Clone, Debug)]
pub struct PipelineConfig {
    /// Render scale factor.
    ///
    /// `1.0` corresponds to the native DPI of the PDF (typically 72 DPI).
    /// Use `1.5` or `2.0` for higher-resolution output.
    pub scale: f32,

    /// Whether to produce plain (content-only) renders.
    ///
    /// When `true`, [`PipelineEvent::PlainRender`] events are emitted and
    /// [`PipelineOutput::plain_renders`] is populated.
    pub render_plain: bool,

    /// Whether to produce annotated (red field-level debug annotations) renders.
    ///
    /// When `true`, [`PipelineEvent::AnnotatedRender`] events are emitted and
    /// [`PipelineOutput::annotated_renders`] is populated.
    pub render_annotated: bool,

    /// Whether to produce labelled (analysis-overlay) renders.
    ///
    /// When `true`, [`PipelineEvent::LabelledRender`] events are emitted and
    /// [`PipelineOutput::labelled_renders`] is populated.
    pub render_labelled: bool,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            scale: 1.5,
            render_plain: true,
            render_annotated: false,
            render_labelled: true,
        }
    }
}

// ============================================================================
// PipelineOutput
// ============================================================================

/// Output produced by a successful [`run_pipeline`] call.
pub struct PipelineOutput {
    /// Plain renders in discovery order: `(label, image)`.
    ///
    /// Empty when [`PipelineConfig::render_plain`] is `false`.
    pub plain_renders: Vec<(String, Arc<RgbaImage>)>,

    /// Annotated (red field-level debug annotations) renders in discovery
    /// order: `(label, image)`.
    ///
    /// Empty when [`PipelineConfig::render_annotated`] is `false`.
    pub annotated_renders: Vec<(String, Arc<RgbaImage>)>,

    /// Labelled renders in discovery order: `(label, image)`.
    ///
    /// Empty when [`PipelineConfig::render_labelled`] is `false`.
    pub labelled_renders: Vec<(String, Arc<RgbaImage>)>,

    /// Per-state labels and selections, in discovery order.
    ///
    /// Each entry is `(label, selections)` where `label` is the same
    /// identifier used in the render events (e.g. `"de_0"`).
    ///
    /// Useful for callers that need per-state information after the pipeline
    /// completes, for example when building a GraphViz decision-flow graph.
    pub state_labels: Vec<(String, Vec<Selection>)>,

    /// The merged document envelope.
    ///
    /// Combines all form states across all input files into a single
    /// structured node tree.  When multiple files were provided (multilingual
    /// mode) all language envelopes are merged.
    pub merged: DocumentEnvelope,
}

// ============================================================================
// run_pipeline
// ============================================================================

/// Run the full conversion pipeline on one or more PDF files (synchronous).
///
/// This is a convenience wrapper around [`run_pipeline_async`] that uses a
/// no-op yield function.  See [`run_pipeline_async`] for full documentation.
///
/// # Example
///
/// ```rust,ignore
/// use blueprint::pipeline::{PipelineConfig, PipelineEvent, run_pipeline};
///
/// let files = vec![("form.pdf".into(), std::fs::read("form.pdf").unwrap())];
/// let output = run_pipeline(&files, &PipelineConfig::default(), |_| {})?;
/// println!("states merged into {} nodes", output.merged.content.len());
/// # Ok::<(), blueprint::Error>(())
/// ```
pub fn run_pipeline(
    files: &[(String, Vec<u8>)],
    config: &PipelineConfig,
    on_event: impl FnMut(PipelineEvent),
) -> Result<PipelineOutput, Error> {
    // The no-op yield future is always immediately ready, so the async
    // function never actually suspends.  We can drive it to completion
    // with a trivial single-poll.
    let fut = run_pipeline_async(files, config, on_event, || std::future::ready(()));
    block_on_ready(fut)
}

/// Run the full conversion pipeline on one or more PDF files (async).
///
/// # Arguments
///
/// * `files` – Input files as `(filename, pdf_bytes)` pairs.  Multiple files
///   are treated as different language versions of the same form and merged
///   into a single multilingual envelope.
/// * `config` – Pipeline configuration (render options, scale factor).
/// * `on_event` – Progress callback invoked during processing.
///   Called whenever the pipeline advances to a new step, finishes exhaustive
///   exploration, or produces an individual render.  The callback is `FnMut`
///   so it can mutate captured state (e.g. a progress struct or a channel
///   sender).
/// * `yield_fn` – An async yield function called between pipeline phases and
///   between individual state iterations.  Pass `|| async {}` for a no-op
///   (equivalent to sync execution) or `|| async_sleep_ms(0)` to yield to a
///   UI runtime.
///
/// # Errors
///
/// Returns an [`Error`] if parsing, exhaustive exploration, rendering, or
/// merging fails fatally.  Non-fatal render errors are emitted as
/// [`PipelineEvent::Warning`] instead.
pub async fn run_pipeline_async<F, Fut>(
    files: &[(String, Vec<u8>)],
    config: &PipelineConfig,
    mut on_event: impl FnMut(PipelineEvent),
    yield_fn: F,
) -> Result<PipelineOutput, Error>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    // ── Phase 1: Parsing ─────────────────────────────────────────────────────
    on_event(PipelineEvent::StepChanged(PipelineStep::Parsing));
    yield_fn().await;

    let mut blueprints: Vec<(String, String, Blueprint)> = Vec::new();
    for (filename, bytes) in files {
        // Auto-detect AEM ZIP vs PDF
        let bp = if crate::aem::detect_aem_zip(bytes) {
            Blueprint::from_aem_zip(bytes)?
        } else {
            Blueprint::from_pdf_bytes(bytes)?
        };
        let language = bp.language().to_string();
        blueprints.push((filename.clone(), language, bp));
        yield_fn().await;
    }

    // ── Phase 2: Exhaustive exploration ──────────────────────────────────────
    on_event(PipelineEvent::StepChanged(
        PipelineStep::ExhaustiveSearching,
    ));
    yield_fn().await;

    // (filename, language, form_states, context)
    // For AEM inputs, form_states is None and we store the envelope directly.
    let mut explored: Vec<(String, String, crate::FormStates, Context)> = Vec::new();
    let mut aem_envelopes: Vec<(String, String, DocumentEnvelope)> = Vec::new();

    for (filename, language, mut bp) in blueprints {
        if bp.is_aem() {
            // AEM path: skip FormStates, get structured output directly
            let envelope = bp
                .aem_structured()
                .ok_or_else(|| Error::Conversion("AEM structured conversion failed".into()))?;
            on_event(PipelineEvent::StatesFound {
                file: filename.clone(),
                count: 1,
            });
            aem_envelopes.push((filename, language, envelope));
        } else {
            let form_states = bp.states()?;
            on_event(PipelineEvent::StatesFound {
                file: filename.clone(),
                count: form_states.len(),
            });
            let context = bp.context();
            explored.push((filename, language, form_states, context));
        }
        yield_fn().await;
    }

    // ── Phase 3: Flattening — plain and annotated renders ─────────────────────
    on_event(PipelineEvent::StepChanged(PipelineStep::Flattening));
    yield_fn().await;

    let mut plain_renders: Vec<(String, Arc<RgbaImage>)> = Vec::new();
    let mut annotated_renders: Vec<(String, Arc<RgbaImage>)> = Vec::new();

    if config.render_plain || config.render_annotated {
        for (_filename, language, form_states, _context) in &explored {
            for (state_idx, form_state) in form_states.iter().enumerate() {
                let label = format!("{}_{}", language, state_idx);

                if config.render_plain {
                    match form_state.render_plain(config.scale) {
                        Ok(image) => {
                            let image = Arc::new(image);
                            on_event(PipelineEvent::PlainRender {
                                label: label.clone(),
                                image: Arc::clone(&image),
                            });
                            plain_renders.push((label.clone(), image));
                        }
                        Err(e) => on_event(PipelineEvent::Warning(format!(
                            "Plain render failed for {label}: {e}"
                        ))),
                    }
                }

                if config.render_annotated {
                    match form_state.render_annotated(config.scale) {
                        Ok(image) => {
                            let image = Arc::new(image);
                            on_event(PipelineEvent::AnnotatedRender {
                                label: label.clone(),
                                image: Arc::clone(&image),
                            });
                            annotated_renders.push((label, image));
                        }
                        Err(e) => on_event(PipelineEvent::Warning(format!(
                            "Annotated render failed for {label}: {e}"
                        ))),
                    }
                }

                yield_fn().await;
            }
        }
    }

    // ── Phase 4: Structuring — labelled renders + structured output ───────────
    on_event(PipelineEvent::StepChanged(PipelineStep::Structuring));
    yield_fn().await;

    let mut labelled_renders: Vec<(String, Arc<RgbaImage>)> = Vec::new();
    let mut per_language_state_maps: Vec<(String, StateMap)> = Vec::new();
    let mut state_labels: Vec<(String, Vec<Selection>)> = Vec::new();

    for (_filename, language, form_states, context) in &explored {
        // Collect labels + selections
        let states_vec: Vec<_> = form_states.iter().enumerate().collect();
        for (state_idx, form_state) in &states_vec {
            let label = format!("{}_{}", language, state_idx);
            state_labels.push((label.clone(), form_state.selections.clone()));

            // Labelled render (sequential — emits events)
            if config.render_labelled {
                match form_state.render_labelled(config.scale) {
                    Ok(image) => {
                        let image = Arc::new(image);
                        on_event(PipelineEvent::LabelledRender {
                            label: label.clone(),
                            image: Arc::clone(&image),
                        });
                        labelled_renders.push((label.clone(), image));
                    }
                    Err(e) => on_event(PipelineEvent::Warning(format!(
                        "Labelled render failed for {label}: {e}"
                    ))),
                }
            }
        }

        // Structured output — parallelized across states
        #[cfg(not(target_arch = "wasm32"))]
        let structured_results: Vec<_> = {
            use rayon::prelude::*;
            states_vec
                .par_iter()
                .map(|(state_idx, form_state)| {
                    let label = format!("{}_{}", language, state_idx);
                    let envelope = form_state.structured(context.clone());
                    let signature = selection_signature(&form_state.selections);
                    (label, signature, form_state.selections.clone(), envelope)
                })
                .collect()
        };

        #[cfg(target_arch = "wasm32")]
        let structured_results: Vec<_> = states_vec
            .iter()
            .map(|(state_idx, form_state)| {
                let label = format!("{}_{}", language, state_idx);
                let envelope = form_state.structured(context.clone());
                let signature = selection_signature(&form_state.selections);
                (label, signature, form_state.selections.clone(), envelope)
            })
            .collect();

        let mut state_map: StateMap = HashMap::new();
        for (_label, signature, selections, envelope) in structured_results {
            if state_map
                .insert(signature.clone(), (selections, envelope))
                .is_some()
            {
                return Err(Error::Conversion(format!(
                    "Duplicate state signature '{signature}' found in language '{language}'"
                )));
            }
        }

        per_language_state_maps.push((language.clone(), state_map));
        yield_fn().await;
    }

    // ── Phase 5: Merging ─────────────────────────────────────────────────────
    on_event(PipelineEvent::StepChanged(PipelineStep::Merging));
    yield_fn().await;

    // If we only have AEM envelopes (no PDF states), merge them directly.
    if per_language_state_maps.is_empty() && !aem_envelopes.is_empty() {
        let merged = if aem_envelopes.len() == 1 {
            aem_envelopes.into_iter().next().unwrap().2
        } else {
            let envelopes: Vec<DocumentEnvelope> =
                aem_envelopes.into_iter().map(|(_, _, e)| e).collect();

            #[cfg(feature = "semantic-matching")]
            let semantic_matcher = crate::semantic::SemanticMatcher::new().ok();
            #[cfg(feature = "semantic-matching")]
            let semantic_ref = semantic_matcher.as_ref();
            #[cfg(not(feature = "semantic-matching"))]
            let semantic_ref: Option<&crate::structured::SemanticCtx> = None;

            crate::merge_translations(envelopes, semantic_ref)
                .map_err(|e| Error::Conversion(e.to_string()))?
        };

        on_event(PipelineEvent::StepChanged(PipelineStep::Complete));

        return Ok(PipelineOutput {
            plain_renders,
            annotated_renders,
            labelled_renders,
            state_labels,
            merged,
        });
    }

    if per_language_state_maps.is_empty() {
        return Err(Error::Conversion("No envelopes to merge".into()));
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

            return Err(Error::Conversion(format!(
                "State signature mismatch for language '{language}'. Missing: [{}], Extra: [{}]",
                missing.join(", "),
                extra.join(", ")
            )));
        }
    }

    // Construct semantic matcher once for the entire merge phase (feature-gated).
    #[cfg(feature = "semantic-matching")]
    let semantic_matcher = {
        log::info!("Loading semantic matcher for translation merge …");
        let start = std::time::Instant::now();
        match crate::semantic::SemanticMatcher::new() {
            Ok(m) => {
                log::info!("Semantic matcher loaded in {:?}", start.elapsed());
                Some(m)
            }
            Err(e) => {
                log::warn!("Failed to load semantic matcher, continuing without: {e}");
                None
            }
        }
    };

    #[cfg(feature = "semantic-matching")]
    let semantic_ref = semantic_matcher.as_ref();
    #[cfg(not(feature = "semantic-matching"))]
    let semantic_ref: Option<&crate::structured::SemanticCtx> = None;

    let mut translated_states: Vec<(Vec<Selection>, DocumentEnvelope)> = Vec::new();
    for signature in &expected_signatures {
        let mut canonical_selections: Option<Vec<Selection>> = None;
        let mut state_envelopes: Vec<DocumentEnvelope> = Vec::new();

        for (language, state_map) in &per_language_state_maps {
            let (selections, envelope) = state_map.get(signature).ok_or_else(|| {
                Error::Conversion(format!(
                    "State signature '{signature}' missing for language '{language}'"
                ))
            })?;

            if let Some(existing) = &canonical_selections {
                if !selections_exact_match(existing, selections) {
                    return Err(Error::Conversion(format!(
                        "Selection mismatch for state signature '{signature}' between languages"
                    )));
                }
            } else {
                canonical_selections = Some(selections.clone());
            }

            state_envelopes.push(envelope.clone());
        }

        let merged_state = if state_envelopes.len() > 1 {
            crate::merge_translations(state_envelopes, semantic_ref)
                .map_err(|e| Error::Conversion(e.to_string()))?
        } else {
            state_envelopes.into_iter().next().unwrap()
        };

        translated_states.push((canonical_selections.unwrap_or_default(), merged_state));
        yield_fn().await;
    }

    if translated_states.is_empty() {
        return Err(Error::Conversion("No translated states to merge".into()));
    }

    let merged_context = translated_states[0].1.context.clone();
    let merged_content = crate::merge_structured_outputs(
        translated_states
            .iter()
            .map(|(selections, envelope)| (selections.clone(), envelope.content.clone()))
            .collect(),
    );

    let merged = DocumentEnvelope {
        context: merged_context,
        content: merged_content,
        state_count: translated_states.len(),
    };

    on_event(PipelineEvent::StepChanged(PipelineStep::Complete));

    Ok(PipelineOutput {
        plain_renders,
        annotated_renders,
        labelled_renders,
        state_labels,
        merged,
    })
}

// ============================================================================
// block_on_ready — trivial executor for futures that never suspend
// ============================================================================

/// Drive a future to completion assuming it never actually suspends.
///
/// This is used by [`run_pipeline`] which passes `|| std::future::ready(())`
/// as the yield function, guaranteeing the future is always immediately ready.
///
/// # Panics
///
/// Panics if the future returns `Poll::Pending`.
fn block_on_ready<T>(fut: impl std::future::Future<Output = T>) -> T {
    use std::pin::pin;
    use std::task::{Context as TaskContext, Poll, Waker};

    let waker = Waker::noop();
    let mut cx = TaskContext::from_waker(waker);

    let mut fut = pin!(fut);
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(val) => val,
        Poll::Pending => panic!("block_on_ready: future returned Pending unexpectedly"),
    }
}

fn selection_signature(selections: &[Selection]) -> String {
    selections
        .iter()
        .map(|selection| {
            format!(
                "{}|{}",
                selection_kind_name(selection),
                selection.option_index,
            )
        })
        .collect::<Vec<_>>()
        .join("->")
}

fn selection_kind_name(selection: &Selection) -> &'static str {
    match selection.kind {
        crate::structured::SelectionKind::Radio => "radio",
        crate::structured::SelectionKind::Checkbox => "checkbox",
        crate::structured::SelectionKind::Dropdown => "dropdown",
    }
}

fn selections_exact_match(a: &[Selection], b: &[Selection]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(left, right)| left.kind == right.kind && left.option_index == right.option_index)
}
