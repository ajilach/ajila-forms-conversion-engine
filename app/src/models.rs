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

/// Kind of an agent activity step.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AgentStepKind {
    /// The model's visible text for a turn.
    Thought,
    /// A tool call.
    Tool,
}

/// Status of an agent activity step (drives the spinner / checkmark).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AgentStepStatus {
    Running,
    Done,
    Error,
}

/// One entry in the Agent Processing activity panel.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AgentStep {
    /// Tool-call id (for matching start→finish); empty for thoughts.
    pub id: String,
    pub kind: AgentStepKind,
    /// Tool name, or the thought text.
    pub label: String,
    /// Short input summary for tool steps.
    pub detail: String,
    pub status: AgentStepStatus,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ProcessingState {
    pub step: ProcessingStep,
    /// Progress within the current step, from 0.0 to 1.0.
    pub step_progress: Option<f32>,
    pub available_states: Vec<String>,
    /// label → per-page base64 images (JPEG), in page order.
    pub plain_images: HashMap<String, Vec<String>>,
    /// label → per-page base64 images (PNG), in page order.
    pub labelled_images: HashMap<String, Vec<String>>,
    pub form_code: Option<String>,
    pub merged_json: Option<String>,
    pub html_preview: Option<String>,
    pub aem_package: Option<Vec<u8>>,
    pub xsd_schema: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// When `true`, the progress UI shows the AI-processing step layout
    /// (pipeline up to state rendering, then a single "AI Generation" step)
    /// instead of the full staged pipeline.
    #[serde(default)]
    pub ai_mode: bool,
    /// Live activity log for the Agent Processing run (thoughts + tool calls).
    #[serde(default)]
    pub agent_steps: Vec<AgentStep>,
    /// Edit-history session id holding the agent's AEM-tree history, so the AEM
    /// editor can show every step the agent took. `None` outside agent mode.
    #[serde(default)]
    pub agent_aem_session: Option<String>,
    /// `true` once the agent has successfully uploaded + installed the built
    /// package on the configured AEM instance during its run.
    #[serde(default)]
    pub aem_uploaded: bool,
    /// JCR path of the uploaded form on AEM, shown on the agent "done" screen.
    #[serde(default)]
    pub aem_form_path: Option<String>,
    /// Wall-clock duration of the most recent agent run, in seconds. Shown
    /// next to "Finished" on the agent "done" screen.
    #[serde(default)]
    pub elapsed_secs: Option<u64>,
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
            && self.ai_mode == other.ai_mode
            && self.agent_steps == other.agent_steps
            && self.agent_aem_session == other.agent_aem_session
            && self.aem_uploaded == other.aem_uploaded
            && self.aem_form_path == other.aem_form_path
            && self.elapsed_secs == other.elapsed_secs
        // Note: envelope is skipped in comparison since DocumentEnvelope doesn't impl PartialEq
    }
}

impl ProcessingState {
    pub fn new() -> Self {
        Self::default()
    }
}
