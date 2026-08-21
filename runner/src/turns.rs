//! The LLM seam, filled in: Anthropic behind [`pipeline::TurnProvider`].
//!
//! The model id and the output cap live here rather than in the controller —
//! they are provider knowledge, and keeping them on this side is what lets the
//! `pipeline` crate carry no model tables at all.

use pipeline::{AbortFlag, TurnOutput, TurnProvider};

use crate::settings::AppSettings;

/// The provider-side numbers a run is about to work with.
///
/// Resolved before the first turn and reported, so a mis-detected context window
/// is visible in the transcript instead of showing up as unexplained eviction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnPlan {
    pub model: String,
    /// Output-token cap sent with every request.
    pub max_tokens: u32,
    /// The model's full context window.
    pub context_window: usize,
    /// How much of that window a request's prompt may occupy.
    pub prompt_target: usize,
}

impl TurnPlan {
    pub fn for_model(model: &str) -> Self {
        let max_tokens = crate::llm::max_output_tokens_for(model);
        Self {
            context_window: crate::llm::context_window_for(model),
            prompt_target: crate::llm::prompt_token_target(model, max_tokens),
            model: model.to_string(),
            max_tokens,
        }
    }

    pub fn for_settings(settings: &AppSettings) -> Self {
        Self::for_model(settings.active_model())
    }

    /// The banner every consumer reports before the first turn. One wording, so
    /// a CLI transcript and the app's timeline say the same thing.
    pub fn describe(&self) -> String {
        format!(
            "Context window: {} tokens · per-turn budget: {} tokens · output cap: {} · model: {}",
            self.context_window, self.prompt_target, self.max_tokens, self.model
        )
    }

    /// The turn provider these numbers describe, authenticated with `api_key`.
    pub fn provider(&self, api_key: impl Into<String>) -> AnthropicTurns {
        AnthropicTurns {
            api_key: api_key.into(),
            model: self.model.clone(),
            max_tokens: self.max_tokens,
        }
    }
}

/// Runs the controller's turns against the Anthropic Messages API.
pub struct AnthropicTurns {
    api_key: String,
    model: String,
    max_tokens: u32,
}

impl TurnProvider for AnthropicTurns {
    async fn turn(
        &self,
        history: &mut Vec<serde_json::Value>,
        tools: &[serde_json::Value],
        system: &str,
        abort: &AbortFlag,
    ) -> Result<TurnOutput, String> {
        crate::llm::anthropic_stream_turn(
            history,
            tools,
            &self.api_key,
            &self.model,
            self.max_tokens,
            Some(system),
            abort,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The banner has to carry the resolved numbers, not the defaults: a window
    /// reported as the fallback is how a silent eviction loop stays invisible.
    #[test]
    fn the_plan_reports_the_model_it_resolved() {
        let plan = TurnPlan::for_model(crate::llm::DEFAULT_MODEL);
        let text = plan.describe();
        assert!(text.contains(crate::llm::DEFAULT_MODEL), "{text}");
        assert!(text.contains(&plan.context_window.to_string()), "{text}");
        assert_eq!(
            plan.context_window,
            crate::llm::context_window_for(&plan.model)
        );
        assert_eq!(
            plan.max_tokens,
            crate::llm::max_output_tokens_for(&plan.model)
        );
    }
}
