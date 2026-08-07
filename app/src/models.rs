#[derive(Clone, Debug, Default, PartialEq)]
pub enum ProcessingStep {
    #[default]
    Idle,
    /// The agent run is under way.
    Running,
    Complete,
}

/// Kind of an agent activity step.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentStepKind {
    /// The model's visible text for a turn.
    Thought,
    /// A tool call.
    Tool,
}

/// Status of an agent activity step (drives the spinner / checkmark).
#[derive(Clone, Debug, PartialEq)]
pub enum AgentStepStatus {
    Running,
    Done,
    Error,
}

/// What the user chose when an agent run paused on a failed API turn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RetryAction {
    /// Re-send the failed turn and carry on.
    Retry,
    /// Give up on the run (the agent keeps whatever it had built).
    Cancel,
}

/// One entry in the Agent Processing activity panel.
#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcessingState {
    pub step: ProcessingStep,
    pub form_code: Option<String>,
    pub merged_json: Option<String>,
    pub html_preview: Option<String>,
    pub aem_package: Option<Vec<u8>>,
    pub xsd_schema: Option<String>,
    /// PostgreSQL dump for the Redacto platform (text-only documents).
    pub redacto_sql: Option<String>,
    pub error: Option<String>,
    /// `true` while an agent run is paused on a failed API turn, waiting for the
    /// user to press Retry (or give up). The run's future is still alive — the
    /// agent, its working tree and the stage history are all held in memory — so
    /// a retry resumes at the failed turn instead of restarting the run.
    pub retry_pending: bool,
    /// The user's answer to a pending retry prompt, set by the progress UI and
    /// consumed by the paused agent loop.
    pub retry_action: Option<RetryAction>,
    pub warnings: Vec<String>,
    /// Live activity log for the Agent Processing run (thoughts + tool calls).
    pub agent_steps: Vec<AgentStep>,
    /// `true` once the agent has successfully uploaded + installed the built
    /// package on the configured AEM instance during its run.
    pub aem_uploaded: bool,
    /// JCR path of the uploaded form on AEM, shown on the agent "done" screen.
    pub aem_form_path: Option<String>,
    /// Wall-clock duration of the most recent agent run, in seconds. Shown
    /// next to "Finished" on the agent "done" screen.
    pub elapsed_secs: Option<u64>,
    /// Latest real prompt-token count sent to the model this run (from the API's
    /// reported usage), for the context-window fill indicator. 0 before the first
    /// turn reports usage.
    pub context_used_tokens: usize,
    /// The model's context window in tokens — the denominator of the fill
    /// indicator. 0 until the agent run sets it.
    pub context_window: usize,
}
