//! The LLM seam. The controller drives turns through [`TurnProvider`] and never
//! names a provider, a model, or an API key.
//!
//! Note what the trait does *not* take: `max_tokens` and the model id. Those are
//! provider knowledge, so they live inside the implementation — which is why the
//! controller carries no model tables.

use std::future::Future;

use crate::observer::AbortFlag;

/// One tool call the model requested.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// The result of one assistant turn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnOutput {
    /// The model's visible text for the turn.
    pub text: String,
    /// Tool calls the model requested (empty if it produced a final answer).
    pub tool_calls: Vec<ToolCall>,
    /// The turn's `stop_reason` (`"tool_use"` when tools were requested).
    pub stop_reason: Option<String>,
    /// Real prompt-token count the API billed for this turn's request — i.e. how
    /// full the context window was. 0 if the API didn't report usage.
    pub prompt_tokens: usize,
}

/// Runs one assistant turn against a model.
///
/// Implementations own the transport, the credentials, the model choice, and any
/// history eviction or caching: the controller hands over the transcript and
/// gets a [`TurnOutput`] back, with the assistant's reply already appended to
/// `history`.
pub trait TurnProvider {
    fn turn(
        &self,
        history: &mut Vec<serde_json::Value>,
        tools: &[serde_json::Value],
        system: &str,
        abort: &AbortFlag,
    ) -> impl Future<Output = Result<TurnOutput, String>>;
}

/// Build the `user` message carrying a batch of tool results.
pub fn tool_result_message(results: Vec<(String, agent::ToolReply)>) -> serde_json::Value {
    use agent::ToolReply;
    let content: Vec<serde_json::Value> = results
        .into_iter()
        .map(|(id, reply)| match reply {
            ToolReply::Text(text) => serde_json::json!({
                "type": "tool_result",
                "tool_use_id": id,
                "content": [{"type": "text", "text": text}],
            }),
            ToolReply::Image { media_type, images } => serde_json::json!({
                "type": "tool_result",
                "tool_use_id": id,
                "content": images
                    .into_iter()
                    .map(|b64| serde_json::json!({
                        "type": "image",
                        "source": {"type": "base64", "media_type": media_type, "data": b64},
                    }))
                    .collect::<Vec<_>>(),
            }),
            ToolReply::Error(msg) => serde_json::json!({
                "type": "tool_result",
                "tool_use_id": id,
                "is_error": true,
                "content": [{"type": "text", "text": msg}],
            }),
        })
        .collect();
    serde_json::json!({ "role": "user", "content": content })
}
