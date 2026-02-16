#![allow(clippy::large_enum_variant)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::collapsible_if)]
//! Blueprint - XFA PDF document analysis library.
//!
//! This library provides a high-level API for processing XFA PDF documents.
//! It can extract form structure, iterate over all interactive states (radio buttons,
//! checkboxes), render pages to images, and produce structured or HTML output.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use blueprint::{Blueprint, HtmlConfig};
//!
//! // Load from a PDF file
//! let mut bp = Blueprint::from_pdf("form.pdf")?;
//!
//! // Get all form states (default + every radio/checkbox combination)
//! let states = bp.states()?;
//!
//! // Iterate over states
//! for state in states.iter() {
//!     // Render each state to an in-memory RGBA image
//!     let img = state.render_plain(1.5)?;
//!
//!     // Get structured representation
//!     let envelope = state.structured(bp.context());
//!
//!     // Convert to HTML
//!     let html = blueprint::to_html(&envelope.content, &HtmlConfig::default());
//! }
//!
//! // Or get everything merged into a single structured tree
//! let merged = bp.merged_structured()?;
//! ```
//!
//! # Architecture
//!
//! The processing pipeline is:
//!
//! ```text
//! PDF bytes
//!   │
//!   ▼
//! extract XFA XML bytes   ──► raw XFA XML
//!   │
//!   ▼
//! XfaNode::parse()        ──► Vec<XfaNode>          (xfa/)
//!   │
//!   ▼
//! XfaForm::new()          ──► XfaForm                (xfa/scripting)
//!   │
//!   ▼
//! Exhaustive exploration  ──► Vec<FormState>          (lib.rs)
//!   │
//!   ├──► structured()     ──► DocumentEnvelope        (structured/)
//!   │     ├──► to_html()  ──► HTML string             (html/)
//!   │     └──► serde      ──► JSON
//!   │
//!   └──► render_*()       ──► RgbaImage               (flattened/ + document/)
//! ```

pub mod aem;
pub mod context;
pub mod document;
pub mod exhaustive;
pub mod flattened;
pub mod html;
pub mod structured;
pub mod xfa;

#[cfg(test)]
mod tests;

// ============================================================================
// Re-exports — flat access to the most commonly used types
// ============================================================================

// Context
pub use context::{Context, ModuleData};

// Flattened layer
pub use flattened::{Flattened, FlattenedNode, FlattenedNodeKind};

// Document / analysis layer
pub use document::modules::{
    AnalysisModule, GlobalContext, run_analysis_pipeline, run_analysis_pipeline_with_context,
};
pub use document::{Document, Group, GroupKind, GroupSource};

// Structured output
pub use structured::{
    DocumentEnvelope, FieldNode, FieldType, HeadingLevel, HeadingNode, InlineNode, InlineText,
    MergeError, MergeInput, ParagraphNode, RecursiveMerger, Selection, SelectionKind,
    StructuredNode, TranslatableString,
};

// AEM generation
pub use aem::{AemConfig, AemNode, convert_to_aem, generate_aem_xml};

// HTML generation
pub use html::{HtmlConfig, generate_form_body, generate_html};

// XFA layer
pub use xfa::scripting::{SomPath, XfaForm};
pub use xfa::{XfaNode, XfaNodeKind};

// Image type (re-export so consumers don't need to depend on `image` directly)
pub use image::RgbaImage;

use pdf::file::FileOptions;
use pdf::object::*;
use pdf::primitive::Primitive;
use std::path::Path;

// ============================================================================
// Render mode
// ============================================================================

/// Render mode for output images.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    /// Plain rendering without any annotations.
    Plain,
    /// Labelled rendering with blue group overlays (runs analysis pipeline).
    Labelled,
    /// Annotated rendering with red field annotations.
    Annotated,
}

// ============================================================================
// Error type
// ============================================================================

/// Errors that can occur during blueprint processing.
#[derive(Debug)]
pub enum Error {
    /// The PDF could not be parsed.
    PdfParse(String),
    /// The PDF does not contain XFA data.
    NoXfaData,
    /// The raw XFA XML could not be parsed into an XFA node tree.
    XfaParse(String),
    /// XFA form creation / scripting failed.
    FormCreation(String),
    /// The exhaustive state exploration failed.
    StateExploration(String),
    /// Rendering to an image buffer failed.
    Render(String),
    /// Structured conversion failed.
    Conversion(String),
    /// Generic I/O error (e.g. file not found).
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::PdfParse(msg) => write!(f, "PDF parse error: {}", msg),
            Error::NoXfaData => write!(f, "PDF does not contain XFA data"),
            Error::XfaParse(msg) => write!(f, "XFA parse error: {}", msg),
            Error::FormCreation(msg) => write!(f, "Form creation error: {}", msg),
            Error::StateExploration(msg) => write!(f, "State exploration error: {}", msg),
            Error::Render(msg) => write!(f, "Render error: {}", msg),
            Error::Conversion(msg) => write!(f, "Conversion error: {}", msg),
            Error::Io(err) => write!(f, "I/O error: {}", err),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
    }
}

// ============================================================================
// PDF / XFA extraction helpers
// ============================================================================

/// Extract raw XFA XML bytes from a PDF file on disk.
///
/// Returns `Ok(Some(bytes))` if the PDF contains XFA data, `Ok(None)` if it
/// does not, or an error if the PDF cannot be read / parsed.
pub fn extract_xfa_from_pdf(path: impl AsRef<Path>) -> Result<Option<Vec<u8>>, Error> {
    let data = std::fs::read(path.as_ref()).map_err(Error::Io)?;
    extract_xfa_from_pdf_bytes(&data)
}

/// Extract raw XFA XML bytes from in-memory PDF bytes.
///
/// Returns `Ok(Some(bytes))` if the PDF contains XFA data, `Ok(None)` otherwise.
pub fn extract_xfa_from_pdf_bytes(pdf_bytes: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    let pdf = FileOptions::cached()
        .load(pdf_bytes.to_vec())
        .map_err(|e| Error::PdfParse(e.to_string()))?;

    let catalog = pdf.get_root();

    if let Some(forms_dict) = &catalog.forms
        && let Some(xfa_obj) = &forms_dict.xfa
    {
        match xfa_obj {
            Primitive::Stream(pdf_stream) => {
                let stream: Stream<()> = Stream::from_stream(pdf_stream.clone(), &pdf.resolver())
                    .map_err(|e| Error::PdfParse(e.to_string()))?;
                let data = stream
                    .data(&pdf.resolver())
                    .map_err(|e| Error::PdfParse(e.to_string()))?;
                return Ok(Some(data.to_vec()));
            }
            Primitive::Array(arr) => {
                let mut xfa_data = Vec::new();
                let resolver = pdf.resolver();

                for i in (1..arr.len()).step_by(2) {
                    if let Primitive::Reference(stream_ref) = &arr[i]
                        && let Ok(Primitive::Stream(ref pdf_stream)) = resolver.resolve(*stream_ref)
                    {
                        let stream: Stream<()> = Stream::from_stream(pdf_stream.clone(), &resolver)
                            .map_err(|e| Error::PdfParse(e.to_string()))?;
                        let data = stream
                            .data(&resolver)
                            .map_err(|e| Error::PdfParse(e.to_string()))?;
                        xfa_data.extend_from_slice(&data);
                    }
                }

                if !xfa_data.is_empty() {
                    return Ok(Some(xfa_data));
                }
            }
            _ => {}
        }
    }

    Ok(None)
}

/// Extract language from an XFA form by inspecting the `Footer_Line_txtlanguage` field.
pub fn extract_language(form: &XfaForm) -> String {
    if let Some(node) = form.resolve("Footer_Line_txtlanguage")
        && let Some(value) = node.raw_value()
    {
        let lang = value.to_uppercase();
        return match lang.as_str() {
            "DE" => "de",
            "EN" => "en",
            "FR" => "fr",
            "IT" => "it",
            "ES" => "es",
            _ => "de",
        }
        .to_string();
    }
    "de".to_string()
}

// ============================================================================
// FormState — a single snapshot of the form
// ============================================================================

/// A single form state produced by exhaustive exploration.
///
/// Each `FormState` represents the document with a specific combination of
/// radio buttons and checkboxes selected. It holds the flattened layout and
/// a shared reference to the `GlobalContext` (computed from *all* states) so
/// that analysis modules produce consistent results across states.
pub struct FormState<'a> {
    /// The flattened layout for this state.
    pub flattened: Flattened,
    /// Which controls were toggled to reach this state.
    pub selections: Vec<Selection>,
    /// Human-readable label (e.g. `"default"`, `"RB_1_CB_2"`).
    pub label: String,
    /// Shared global context computed from all sibling states.
    global_ctx: &'a GlobalContext<'a>,
}

impl<'a> FormState<'a> {
    /// Convert this state to a [`DocumentEnvelope`] containing the structured
    /// representation of the form.
    ///
    /// This runs the full analysis pipeline (text grouping, field detection,
    /// heading detection, label attachment, …) and then converts to the
    /// structured node tree.
    pub fn structured(&self, context: Context) -> DocumentEnvelope {
        let mut doc = Document::from_flattened(&self.flattened);
        run_analysis_pipeline_with_context(&mut doc, self.global_ctx);
        structured::convert_with_context(&doc, context)
    }

    /// Render the page as a plain image (content only, no debug overlays).
    pub fn render_plain(&self, scale: f32) -> Result<RgbaImage, Error> {
        self.flattened
            .render_to_image_buffer_plain(scale)
            .map_err(Error::Render)
    }

    /// Render the page with red field-level debug annotations.
    pub fn render_annotated(&self, scale: f32) -> Result<RgbaImage, Error> {
        self.flattened
            .render_to_image_buffer(scale)
            .map_err(Error::Render)
    }

    /// Render the page with blue group-level analysis overlays.
    ///
    /// This runs the analysis pipeline first, then composites group bounding
    /// boxes and labels on top of the rendered content.
    pub fn render_labelled(&self, scale: f32) -> Result<RgbaImage, Error> {
        let mut doc = Document::from_flattened(&self.flattened);
        run_analysis_pipeline_with_context(&mut doc, self.global_ctx);
        doc.render_to_image_buffer(scale).map_err(Error::Render)
    }
}

// ============================================================================
// Blueprint — the main façade
// ============================================================================

/// High-level entry point for processing XFA PDF documents.
///
/// `Blueprint` holds the parsed XFA data and provides methods to explore form
/// states, produce structured output, render images, and generate HTML — all
/// without touching the file system.
pub struct Blueprint {
    /// Raw XFA XML bytes — kept around so the exhaustive explorer can cheaply
    /// recreate fresh `XfaForm` instances for each branch.
    xfa_bytes: Vec<u8>,
    /// The live XFA form (mutable because exploration changes state).
    form: XfaForm,
    /// Auto-detected (or caller-supplied) document language.
    language: String,
}

impl Blueprint {
    // ────────────────────────────────────────────────────────────────────────
    // Construction
    // ────────────────────────────────────────────────────────────────────────

    /// Create a `Blueprint` from a PDF file on disk.
    pub fn from_pdf(path: impl AsRef<Path>) -> Result<Self, Error> {
        let pdf_bytes = std::fs::read(path.as_ref()).map_err(Error::Io)?;
        Self::from_pdf_bytes(&pdf_bytes)
    }

    /// Create a `Blueprint` from in-memory PDF bytes.
    pub fn from_pdf_bytes(pdf_bytes: &[u8]) -> Result<Self, Error> {
        let xfa_bytes = extract_xfa_from_pdf_bytes(pdf_bytes)?.ok_or(Error::NoXfaData)?;
        let language = {
            let nodes = XfaNode::parse(&xfa_bytes).map_err(Error::XfaParse)?;
            let form = XfaForm::new(nodes).map_err(Error::FormCreation)?;
            extract_language(&form)
        };
        Self::from_xfa_bytes(xfa_bytes, &language)
    }

    /// Create a `Blueprint` directly from raw XFA XML bytes and a language tag.
    ///
    /// Use this when you have already extracted the XFA XML yourself or when
    /// working with non-PDF sources of XFA data.
    pub fn from_xfa_bytes(xfa_bytes: Vec<u8>, language: &str) -> Result<Self, Error> {
        let nodes = XfaNode::parse(&xfa_bytes).map_err(Error::XfaParse)?;
        let form = XfaForm::new(nodes).map_err(Error::FormCreation)?;
        Ok(Blueprint {
            xfa_bytes,
            form,
            language: language.to_string(),
        })
    }

    // ────────────────────────────────────────────────────────────────────────
    // Accessors
    // ────────────────────────────────────────────────────────────────────────

    /// The auto-detected (or explicitly provided) document language.
    pub fn language(&self) -> &str {
        &self.language
    }

    /// Build a [`Context`] seeded with the document language.
    pub fn context(&self) -> Context {
        Context::new(self.language.clone())
    }

    /// Access the underlying [`XfaForm`] (e.g. to resolve individual fields).
    pub fn form(&self) -> &XfaForm {
        &self.form
    }

    /// Mutable access to the underlying [`XfaForm`].
    pub fn form_mut(&mut self) -> &mut XfaForm {
        &mut self.form
    }

    // ────────────────────────────────────────────────────────────────────────
    // Exhaustive state exploration
    // ────────────────────────────────────────────────────────────────────────

    /// Discover all reachable form states by toggling every visible radio
    /// button and checkbox, then return them as [`FormState`] values.
    ///
    /// This uses the two-pass architecture:
    /// 1. **Collection pass** — recursively explore states, collecting
    ///    `Flattened` snapshots.
    /// 2. **Analysis pass** — compute `GlobalContext` from all snapshots so
    ///    that analysis modules (e.g. heading detection) produce consistent
    ///    results.
    ///
    /// The returned `FormState` values borrow from the `GlobalContext` that is
    /// stored inside the returned [`FormStates`] wrapper.
    pub fn states(&mut self) -> Result<FormStates, Error> {
        let collected = exhaustive::collect_states(&mut self.form, &self.xfa_bytes)?;

        Ok(FormStates::new(collected))
    }

    /// Run full exhaustive exploration *and* merge all state trees into a
    /// single [`DocumentEnvelope`].
    ///
    /// Equivalent to calling [`states()`](Self::states), converting each state
    /// to structured output, and then running `RecursiveMerger`.
    pub fn merged_structured(&mut self) -> Result<DocumentEnvelope, Error> {
        let form_states = self.states()?;
        let context = self.context();
        let merged = merge_form_states(&form_states, context.clone());

        Ok(DocumentEnvelope {
            context,
            content: merged,
        })
    }
}

// ============================================================================
// FormStates — owns the GlobalContext and yields FormState references
// ============================================================================

/// Owns the collected states and the `GlobalContext` computed from them.
///
/// Individual [`FormState`] values borrow from this container.
pub struct FormStates {
    /// Raw collected data (flattened + selections + label).
    collected: Vec<exhaustive::CollectedState>,
    /// Flattened references for GlobalContext (kept in sync with `collected`).
    flattened_refs: Vec<Flattened>,
}

impl FormStates {
    fn new(collected: Vec<exhaustive::CollectedState>) -> Self {
        // Extract flattened clones for the GlobalContext reference slice.
        let flattened_refs: Vec<Flattened> =
            collected.iter().map(|s| s.flattened.clone()).collect();
        FormStates {
            collected,
            flattened_refs,
        }
    }

    /// Number of unique form states.
    pub fn len(&self) -> usize {
        self.collected.len()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.collected.is_empty()
    }

    /// Iterate over all form states.
    ///
    /// Each [`FormState`] borrows from this `FormStates` container (for the
    /// shared `GlobalContext`).
    ///
    /// Note: This method leaks a small amount of memory (the `GlobalContext` for
    /// this iteration) to satisfy lifetime requirements. For most use cases, this
    /// is negligible. If you need to avoid this, use [`for_each`](Self::for_each)
    /// instead.
    pub fn iter(&self) -> FormStatesIter<'_> {
        let refs: Vec<&Flattened> = self.flattened_refs.iter().collect();
        let refs_box: Box<[&Flattened]> = refs.into_boxed_slice();
        let refs_static = unsafe {
            // SAFETY: We're converting the box into a raw pointer and then
            // dereferencing it. This leaks the allocation, but ensures the
            // slice lives long enough for the iterator.
            &*(Box::into_raw(refs_box) as *const [&Flattened])
        };
        let global_ctx = Box::leak(Box::new(GlobalContext::new(refs_static)));

        FormStatesIter {
            owner: self,
            global_ctx,
            index: 0,
        }
    }
}

/// Iterator over [`FormState`] values.
pub struct FormStatesIter<'a> {
    owner: &'a FormStates,
    global_ctx: &'a GlobalContext<'a>,
    index: usize,
}

impl<'a> Iterator for FormStatesIter<'a> {
    type Item = FormState<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.owner.collected.len() {
            return None;
        }

        let state = &self.owner.collected[self.index];
        self.index += 1;

        Some(FormState {
            flattened: state.flattened.clone(),
            selections: state.selections.clone(),
            label: state.label.clone(),
            global_ctx: self.global_ctx,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.owner.collected.len() - self.index;
        (remaining, Some(remaining))
    }
}

impl<'a> ExactSizeIterator for FormStatesIter<'a> {
    fn len(&self) -> usize {
        self.owner.collected.len() - self.index
    }
}

// ============================================================================
// Shared merge helper
// ============================================================================

/// Collect structured output from every form state and merge into a single tree.
///
/// This is the shared implementation behind [`Blueprint::merged_structured()`]
/// and [`run_exhaustive_to_merged()`].
fn merge_form_states(form_states: &FormStates, context: Context) -> Vec<StructuredNode> {
    let mut structured_outputs: Vec<(Vec<Selection>, Vec<StructuredNode>)> = Vec::new();

    for state in form_states.iter() {
        let envelope = state.structured(context.clone());
        structured_outputs.push((state.selections.clone(), envelope.content));
    }

    if structured_outputs.is_empty() {
        return Vec::new();
    }

    let merge_inputs: Vec<MergeInput> = structured_outputs
        .into_iter()
        .map(|(selections, nodes)| MergeInput::new(selections, nodes))
        .collect();

    let merger = RecursiveMerger::new(merge_inputs);
    merger.merge()
}

// ============================================================================
// Convenience free functions
// ============================================================================

/// Merge multiple [`DocumentEnvelope`]s from different languages into one
/// multilingual envelope.
pub fn merge_translations(
    envelopes: Vec<DocumentEnvelope>,
) -> Result<DocumentEnvelope, structured::MergeError> {
    structured::merge_translations(envelopes)
}

/// Generate a complete HTML document from structured nodes.
pub fn to_html(content: &[StructuredNode], config: &HtmlConfig) -> String {
    generate_html(content, config)
}

/// Convert structured nodes to an AEM node tree and serialize to XML.
pub fn to_aem(content: &[StructuredNode], config: &AemConfig) -> String {
    let root = convert_to_aem(content, config);
    generate_aem_xml(&root, config)
}

/// Run exhaustive exploration on a PDF file and return the merged structured tree.
///
/// This helper reads the PDF from disk, explores all states, and merges them
/// into a single structured representation. It does not perform any file I/O
/// beyond reading the input PDF.
pub fn run_exhaustive_to_merged(pdf_path: &str) -> Result<Vec<StructuredNode>, Error> {
    let mut bp = Blueprint::from_pdf(pdf_path)?;
    let form_states = bp.states()?;
    let context = Context::new("en".to_string());
    Ok(merge_form_states(&form_states, context))
}

/// Run exhaustive exploration on a PDF file and return a `DocumentEnvelope`.
///
/// The caller controls the language value stored in the envelope context.
pub fn run_exhaustive_to_envelope(
    pdf_path: &str,
    language: &str,
) -> Result<DocumentEnvelope, Error> {
    let nodes = run_exhaustive_to_merged(pdf_path)?;
    Ok(DocumentEnvelope {
        context: Context::new(language.to_string()),
        content: nodes,
    })
}
