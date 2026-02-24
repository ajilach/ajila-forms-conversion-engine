use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ProcessingStep {
    #[default]
    Idle,
    Parsing,
    ExhaustiveSearching,
    Flattening,
    Structuring,
    Merging,
    Complete,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProcessingState {
    pub step: ProcessingStep,
    pub available_states: Vec<String>,
    pub plain_images: HashMap<String, Vec<u8>>,
    pub labelled_images: HashMap<String, Vec<u8>>,
    pub form_code: Option<String>,
    pub merged_json: Option<String>,
    pub html_preview: Option<String>,
    pub aem_package: Option<Vec<u8>>,
    pub error: Option<String>,
}

impl ProcessingState {
    pub fn new() -> Self {
        Self::default()
    }
}
