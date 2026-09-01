//! The OpenAI-compatible transport: the same streamed turn primitive as
//! [`crate::llm`], against any `/chat/completions` endpoint (OpenRouter, a local
//! gateway, OpenAI itself).
//!
//! The conversation stays Anthropic-shaped everywhere else in the app — the
//! eviction ladder, the edit log and a resumed session all read that shape — so
//! the translation lives here and only here: Anthropic blocks go out as OpenAI
//! messages, and what comes back is appended as Anthropic blocks again.
//!
//! What this path does not do is prompt caching. `cache_control` is an Anthropic
//! extension, and a strict OpenAI-compatible server rejects unknown fields
//! inside content parts, so the prompt is sent plain. Expect the input cost of a
//! long run to be higher here than on the Anthropic path.

use serde_json::{Value, json};

use crate::llm;
use crate::provider::LlmEndpoint;
use pipeline::TurnOutput;

/// What an error from this transport is prefixed with. Deliberately not
/// "OpenAI": the endpoint is usually somebody else's.
const LABEL: &str = "LLM API";

// ── Anthropic blocks → OpenAI messages ──────────────────────────────────────

/// Convert one Anthropic tool definition (`name` / `description` /
/// `input_schema`) into the OpenAI function-tool shape. Fields are copied
/// explicitly, which is also what drops `cache_control` if the caller set it.
fn openai_tool(tool: &Value) -> Value {
    let mut function = json!({
        "name": tool["name"],
        "parameters": tool.get("input_schema").cloned().unwrap_or_else(|| json!({
            "type": "object",
            "properties": {},
        })),
    });
    if let Some(desc) = tool.get("description").and_then(Value::as_str) {
        function["description"] = json!(desc);
    }
    json!({"type": "function", "function": function})
}

/// One Anthropic content block rendered as an OpenAI content part, or `None`
/// for a block this path cannot carry inline (`tool_result`, handled by
/// [`openai_messages`]).
fn content_part(block: &Value) -> Option<Value> {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => Some(json!({"type": "text", "text": block["text"].as_str().unwrap_or("")})),
        Some("image") => {
            let source = block.get("source")?;
            let media_type = source["media_type"].as_str().unwrap_or("image/png");
            let data = source["data"].as_str().unwrap_or("");
            Some(json!({
                "type": "image_url",
                "image_url": {"url": format!("data:{media_type};base64,{data}")},
            }))
        }
        _ => None,
    }
}

/// Collapse content parts into the smallest shape every server accepts: a plain
/// string when there is nothing but text, the part array otherwise.
fn collapse_parts(parts: Vec<Value>) -> Value {
    if parts.iter().all(|p| p["type"] == "text") {
        let text: Vec<&str> = parts
            .iter()
            .map(|p| p["text"].as_str().unwrap_or(""))
            .collect();
        return json!(text.join("\n"));
    }
    json!(parts)
}

/// Text of a `tool_result`'s inner blocks, with images replaced by a note.
///
/// A `tool` message may only carry text in this dialect, so images ride along in
/// the user message that follows (see [`openai_messages`]) and the note is what
/// ties the two together.
fn tool_result_text(block: &Value, has_images: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    if block["is_error"].as_bool() == Some(true) {
        lines.push("Error:".to_string());
    }
    match block.get("content") {
        Some(Value::String(s)) => lines.push(s.clone()),
        Some(Value::Array(inner)) => {
            for b in inner {
                if b.get("type").and_then(Value::as_str) == Some("text") {
                    lines.push(b["text"].as_str().unwrap_or("").to_string());
                }
            }
        }
        _ => {}
    }
    if has_images {
        lines.push("(image output follows in the next message)".to_string());
    }
    if lines.is_empty() {
        // Never an empty `tool` message: some servers reject one, and an empty
        // result is itself information the model needs.
        lines.push("(no output)".to_string());
    }
    lines.join("\n")
}

/// Translate the Anthropic-shaped `history` (plus `system`) into OpenAI chat
/// messages.
///
/// Three rules do the work:
///  * an assistant message becomes one message with its text plus a `tool_calls`
///    array (`input` re-serialized as the string arguments this dialect wants);
///  * every `tool_result` in a user message becomes its own `tool` message,
///    emitted **before** the rest of that message so it directly follows the
///    assistant turn that called it — the ordering these endpoints enforce;
///  * images cannot live in a `tool` message, so they are deferred into the
///    user message that follows the tool batch.
pub(crate) fn openai_messages(history: &[Value], system: Option<&str>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    if let Some(s) = system.filter(|s| !s.is_empty()) {
        out.push(json!({"role": "system", "content": s}));
    }

    for message in history {
        let role = message["role"].as_str().unwrap_or("user");
        let blocks: Vec<Value> = match message.get("content") {
            Some(Value::Array(a)) => a.clone(),
            Some(Value::String(s)) => vec![json!({"type": "text", "text": s})],
            _ => Vec::new(),
        };

        if role == "assistant" {
            let mut text_parts: Vec<&str> = Vec::new();
            let mut tool_calls: Vec<Value> = Vec::new();
            for b in &blocks {
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => text_parts.push(b["text"].as_str().unwrap_or("")),
                    Some("tool_use") => tool_calls.push(json!({
                        "id": b["id"],
                        "type": "function",
                        "function": {
                            "name": b["name"],
                            "arguments": serde_json::to_string(&b["input"])
                                .unwrap_or_else(|_| "{}".to_string()),
                        },
                    })),
                    _ => {}
                }
            }
            let text = text_parts.join("\n");
            let mut msg = json!({
                "role": "assistant",
                "content": if text.is_empty() { Value::Null } else { json!(text) },
            });
            if !tool_calls.is_empty() {
                msg["tool_calls"] = json!(tool_calls);
            }
            out.push(msg);
            continue;
        }

        // A user message: tool results first (each its own message), then
        // whatever else it carried, with any images from those results.
        let mut trailing: Vec<Value> = Vec::new();
        for b in &blocks {
            if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                let images: Vec<Value> = b
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|inner| inner.iter().filter_map(content_part).collect::<Vec<_>>())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|p| p["type"] == "image_url")
                    .collect();
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": b["tool_use_id"],
                    "content": tool_result_text(b, !images.is_empty()),
                }));
                trailing.extend(images);
            } else if let Some(part) = content_part(b) {
                trailing.push(part);
            }
        }
        if !trailing.is_empty() {
            out.push(json!({"role": "user", "content": collapse_parts(trailing)}));
        }
    }
    out
}

/// The chat-completions request body.
fn openai_request_body(
    history: &[Value],
    tools: &[Value],
    system: Option<&str>,
    model: &str,
    max_tokens: u32,
) -> Value {
    let mut request = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": openai_messages(history, system),
        "stream": true,
        // Not every server honours this; the ones that do let the estimate
        // calibrate itself against real usage the way the Anthropic path does.
        "stream_options": {"include_usage": true},
    });
    if !tools.is_empty() {
        request["tools"] = json!(tools.iter().map(openai_tool).collect::<Vec<_>>());
        request["tool_choice"] = json!("auto");
    }
    request
}

// ── OpenAI stream → Anthropic turn output ───────────────────────────────────

/// Map a chat-completions `finish_reason` onto the `stop_reason` vocabulary the
/// controller reads.
///
/// `has_tool_calls` overrides a plain `"stop"`: several OpenAI-compatible
/// servers report `stop` on a turn that did request tools, and the controller
/// treats anything but `"tool_use"` as the end of the stage — which would drop
/// the calls on the floor.
fn normalize_stop_reason(raw: Option<&str>, has_tool_calls: bool) -> Option<String> {
    let mapped = match raw {
        Some("tool_calls") | Some("function_call") => "tool_use",
        Some("length") | Some("max_tokens") => "max_tokens",
        Some("stop") | Some("end_turn") if has_tool_calls => "tool_use",
        Some("stop") => "end_turn",
        Some(other) => return Some(other.to_string()),
        // No finish_reason at all (a truncated stream): infer from the content.
        None if has_tool_calls => "tool_use",
        None => return None,
    };
    Some(mapped.to_string())
}

/// Accumulated `tool_calls` deltas: streamed index → (id, name, arguments).
type ToolBlocks = std::collections::BTreeMap<u64, (String, String, String)>;

/// Fold one `choices[].delta` into the turn being assembled.
fn apply_delta(delta: &Value, text: &mut String, tool_blocks: &mut ToolBlocks) {
    match delta.get("content") {
        Some(Value::String(s)) => text.push_str(s),
        // A few servers stream content as parts rather than a bare string.
        Some(Value::Array(parts)) => {
            for p in parts {
                if let Some(t) = p["text"].as_str() {
                    text.push_str(t);
                }
            }
        }
        _ => {}
    }

    let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) else {
        return;
    };
    for call in calls {
        let index = call["index"].as_u64().unwrap_or(0);
        let entry = tool_blocks.entry(index).or_default();
        if let Some(id) = call["id"].as_str()
            && !id.is_empty()
        {
            entry.0 = id.to_string();
        }
        if let Some(name) = call["function"]["name"].as_str() {
            entry.1.push_str(name);
        }
        if let Some(args) = call["function"]["arguments"].as_str() {
            entry.2.push_str(args);
        }
    }
}

/// A tool call with no id: some servers omit it entirely. The id only has to be
/// unique within the turn and match the `tool_result` we send back, so the
/// stream index is a fine substitute — but it has to be filled in, or the pair
/// cannot be matched at all.
fn fill_missing_ids(tool_blocks: ToolBlocks) -> Vec<(String, String, String)> {
    tool_blocks
        .into_iter()
        .map(|(index, (id, name, args))| {
            let id = if id.is_empty() {
                format!("call_{index}")
            } else {
                id
            };
            (id, name, args)
        })
        .collect()
}

/// Run **one** streamed assistant turn against an OpenAI-compatible endpoint.
/// Same contract as [`crate::llm::anthropic_stream_turn`]: the assistant message
/// is appended to `history` in Anthropic shape and the turn's text, tool calls
/// and `stop_reason` are returned.
pub async fn openai_stream_turn(
    history: &mut Vec<Value>,
    tools: &[Value],
    endpoint: &LlmEndpoint,
    max_tokens: u32,
    system: Option<&str>,
    abort: &pipeline::AbortFlag,
) -> Result<TurnOutput, String> {
    endpoint.check()?;

    let client = llm::streaming_client();
    let model = endpoint.model.clone();
    let url = endpoint.url("/chat/completions");
    let api_key = endpoint.api_key.clone();

    let (response, sent_estimate) = llm::stream_request(
        llm::TurnRequest {
            history,
            tools,
            system,
            model: &endpoint.model,
            max_tokens,
            label: LABEL,
        },
        move |h, t, s| openai_request_body(h, t, s, &model, max_tokens),
        |body| {
            client
                .post(&url)
                .header("authorization", format!("Bearer {api_key}"))
                .header("content-type", "application/json")
                .body(body)
        },
    )
    .await?;

    let mut response_text = String::new();
    let mut tool_blocks = ToolBlocks::new();
    let mut finish_reason: Option<String> = None;
    // Real prompt-token count, when the server reports usage. 0 otherwise, which
    // simply leaves the estimate uncalibrated.
    let mut prompt_tokens: usize = 0;

    llm::for_each_sse_data(response, abort, LABEL, |data| {
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            return Ok(());
        };
        // Errors arrive mid-stream as a chunk with an `error` object rather than
        // an HTTP status, so a failed generation must be caught here too.
        if let Some(msg) = event["error"]["message"].as_str() {
            return Err(format!("{LABEL} error: {msg}"));
        }
        if let Some(n) = event["usage"]["prompt_tokens"].as_u64() {
            prompt_tokens = n as usize;
        }
        if let Some(choice) = event["choices"].as_array().and_then(|c| c.first()) {
            apply_delta(&choice["delta"], &mut response_text, &mut tool_blocks);
            if let Some(reason) = choice["finish_reason"].as_str() {
                finish_reason = Some(reason.to_string());
            }
        }
        Ok(())
    })
    .await?;

    llm::record_token_calibration(prompt_tokens, sent_estimate);

    let calls = fill_missing_ids(tool_blocks);
    let stop_reason = normalize_stop_reason(finish_reason.as_deref(), !calls.is_empty());
    Ok(llm::finish_turn(
        history,
        response_text,
        calls,
        stop_reason,
        prompt_tokens,
    ))
}

/// The model ids the endpoint offers, sorted. `GET {base}/models` is the one
/// discovery call every OpenAI-compatible server implements.
pub async fn openai_list_models(endpoint: &LlmEndpoint) -> Result<Vec<String>, String> {
    let mut request = reqwest::Client::new().get(endpoint.url("/models"));
    // OpenRouter serves its catalogue unauthenticated; OpenAI does not. Send the
    // key when there is one and let the server decide.
    if !endpoint.api_key.trim().is_empty() {
        request = request.header("authorization", format!("Bearer {}", endpoint.api_key));
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("Failed to list models: {e}"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to list models: {e}"))?;

    if !status.is_success() {
        let msg = body["error"]["message"].as_str().unwrap_or("unknown error");
        return Err(format!("Failed to list models ({status}): {msg}"));
    }

    let mut ids: Vec<String> = body["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history() -> Vec<Value> {
        vec![
            json!({"role": "user", "content": [{"type": "text", "text": "convert this"}]}),
            json!({"role": "assistant", "content": [
                {"type": "text", "text": "reading it"},
                {"type": "tool_use", "id": "t1", "name": "get_xfa", "input": {"path": "/a"}},
            ]}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": [
                    {"type": "text", "text": "<xfa/>"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAA"}},
                ]},
                {"type": "text", "text": "carry on"},
            ]}),
        ]
    }

    /// The ordering rule these endpoints enforce: every `tool` message directly
    /// follows the assistant turn that called it, before any user content.
    #[test]
    fn tool_results_become_tool_messages_ahead_of_the_user_text() {
        let msgs = openai_messages(&history(), Some("be careful"));
        let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["system", "user", "assistant", "tool", "user"]);
        assert_eq!(msgs[3]["tool_call_id"], "t1");
        assert!(msgs[3]["content"].as_str().unwrap().contains("<xfa/>"));
    }

    /// Tool calls have to survive the round trip as function calls with string
    /// arguments — the model cannot act on an object it never receives.
    #[test]
    fn tool_use_becomes_a_function_call_with_string_arguments() {
        let msgs = openai_messages(&history(), None);
        let call = &msgs[1]["tool_calls"][0];
        assert_eq!(call["id"], "t1");
        assert_eq!(call["function"]["name"], "get_xfa");
        assert_eq!(call["function"]["arguments"], "{\"path\":\"/a\"}");
    }

    /// An image inside a tool result cannot ride in the `tool` message, so it has
    /// to reappear in the user message that follows — dropping it would silently
    /// blind the model to every rendered page.
    #[test]
    fn images_in_a_tool_result_move_into_the_following_user_message() {
        let msgs = openai_messages(&history(), None);
        let last = msgs.last().unwrap();
        let parts = last["content"].as_array().expect("mixed parts");
        let url = parts
            .iter()
            .find(|p| p["type"] == "image_url")
            .expect("the image survives")["image_url"]["url"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(url, "data:image/png;base64,AAA");
        assert!(parts.iter().any(|p| p["text"].as_str() == Some("carry on")));
    }

    #[test]
    fn an_error_result_is_marked_as_one() {
        let h = vec![json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t9", "is_error": true,
             "content": [{"type": "text", "text": "boom"}]},
        ]})];
        let msgs = openai_messages(&h, None);
        let text = msgs[0]["content"].as_str().unwrap();
        assert!(text.starts_with("Error:"), "{text}");
        assert!(text.contains("boom"));
    }

    /// A text-only user turn should not become a part array: the plain string is
    /// what every server accepts.
    #[test]
    fn text_only_content_stays_a_string() {
        let msgs = openai_messages(&history(), None);
        assert_eq!(msgs[0]["content"], "convert this");
    }

    #[test]
    fn tools_translate_to_function_definitions() {
        let tool = json!({
            "name": "set_xfa",
            "description": "write it",
            "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}},
        });
        let converted = openai_tool(&tool);
        assert_eq!(converted["type"], "function");
        assert_eq!(converted["function"]["name"], "set_xfa");
        assert_eq!(converted["function"]["description"], "write it");
        assert_eq!(converted["function"]["parameters"]["type"], "object");
    }

    /// A server that reports `stop` on a turn that did call tools must not end
    /// the stage: the controller reads `tool_use` and nothing else as "keep going".
    #[test]
    fn a_stop_finish_reason_with_tool_calls_still_reads_as_tool_use() {
        assert_eq!(
            normalize_stop_reason(Some("stop"), true).as_deref(),
            Some("tool_use")
        );
        assert_eq!(
            normalize_stop_reason(Some("stop"), false).as_deref(),
            Some("end_turn")
        );
        assert_eq!(
            normalize_stop_reason(Some("tool_calls"), true).as_deref(),
            Some("tool_use")
        );
        assert_eq!(
            normalize_stop_reason(Some("length"), false).as_deref(),
            Some("max_tokens")
        );
        assert_eq!(normalize_stop_reason(None, false), None);
    }

    /// Arguments arrive in fragments across chunks, and the id only in the first.
    #[test]
    fn tool_call_deltas_accumulate_across_chunks() {
        let mut text = String::new();
        let mut blocks = ToolBlocks::new();
        apply_delta(
            &json!({"content": "th", "tool_calls": [
                {"index": 0, "id": "call_1", "function": {"name": "set_xfa", "arguments": "{\"pa"}}
            ]}),
            &mut text,
            &mut blocks,
        );
        apply_delta(
            &json!({"content": "inking", "tool_calls": [
                {"index": 0, "function": {"arguments": "th\":\"/a\"}"}}
            ]}),
            &mut text,
            &mut blocks,
        );
        assert_eq!(text, "thinking");
        let calls = fill_missing_ids(blocks);
        assert_eq!(
            calls,
            vec![(
                "call_1".to_string(),
                "set_xfa".to_string(),
                "{\"path\":\"/a\"}".to_string(),
            )]
        );
    }

    #[test]
    fn a_tool_call_without_an_id_still_gets_one() {
        let mut blocks = ToolBlocks::new();
        apply_delta(
            &json!({"tool_calls": [{"index": 2, "function": {"name": "n", "arguments": "{}"}}]}),
            &mut String::new(),
            &mut blocks,
        );
        assert_eq!(fill_missing_ids(blocks)[0].0, "call_2");
    }

    /// The request has to carry the model, the cap and the tools, and must not
    /// leak Anthropic's `cache_control` into a dialect that rejects it.
    #[test]
    fn the_request_body_carries_no_anthropic_extensions() {
        let tools = vec![json!({
            "name": "t", "description": "d", "input_schema": {"type": "object"},
            "cache_control": {"type": "ephemeral"},
        })];
        let body = openai_request_body(&history(), &tools, Some("sys"), "some/model", 4096);
        assert_eq!(body["model"], "some/model");
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["stream"], true);
        let serialized = serde_json::to_string(&body).unwrap();
        assert!(!serialized.contains("cache_control"), "{serialized}");
    }
}
