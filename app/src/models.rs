use std::collections::HashMap;

// Re-export DocumentEnvelope for the editor
pub use blueprint::DocumentEnvelope;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ProcessingStep {
    #[default]
    Idle,
    Parsing,
    ExhaustiveSearching,
    Flattening,
    Structuring,
    Merging,
    /// Generating the structured document directly from PDFs via the LLM
    /// (the "Start AI Processing" path, which skips the staged pipeline).
    AiGenerating,
    Complete,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ProcessingState {
    pub step: ProcessingStep,
    /// Progress within the current step, from 0.0 to 1.0.
    pub step_progress: Option<f32>,
    pub available_states: Vec<String>,
    pub plain_images: HashMap<String, String>,
    pub labelled_images: HashMap<String, String>,
    pub form_code: Option<String>,
    pub merged_json: Option<String>,
    pub html_preview: Option<String>,
    pub aem_package: Option<Vec<u8>>,
    pub xsd_schema: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// The merged document envelope for the editor.
    /// This is the structured representation before JSON serialization.
    #[serde(skip)]
    pub envelope: Option<DocumentEnvelope>,
}

impl PartialEq for ProcessingState {
    fn eq(&self, other: &Self) -> bool {
        // Compare all fields except envelope (which doesn't implement PartialEq)
        self.step == other.step
            && self.step_progress.map(|p| (p * 100.0) as u32)
                == other.step_progress.map(|p| (p * 100.0) as u32)
            && self.available_states == other.available_states
            && self.plain_images == other.plain_images
            && self.labelled_images == other.labelled_images
            && self.form_code == other.form_code
            && self.merged_json == other.merged_json
            && self.html_preview == other.html_preview
            && self.aem_package == other.aem_package
            && self.xsd_schema == other.xsd_schema
            && self.error == other.error
            && self.warnings == other.warnings
        // Note: envelope is skipped in comparison since DocumentEnvelope doesn't impl PartialEq
    }
}

impl ProcessingState {
    pub fn new() -> Self {
        Self::default()
    }
}
