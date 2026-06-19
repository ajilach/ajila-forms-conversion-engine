//! Platform-aware helpers: async sleep, file download, HTML preview, file explorer.

/// Platform-agnostic async sleep.
#[allow(dead_code)]
pub async fn async_sleep_ms(ms: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

// ── File download / preview helpers ──────────────────────────────────

pub fn download_file(data: &[u8], filename: &str, _mime_type: &str) {
    match dirs::home_dir() {
        Some(home) => {
            let download_path = home.join("Downloads").join(filename);
            match std::fs::write(&download_path, data) {
                Ok(_) => {
                    println!("✓ File saved to: {}", download_path.display());
                    reveal_in_file_explorer(&download_path);
                }
                Err(e) => {
                    eprintln!(
                        "✗ Failed to save file to {}: {}",
                        download_path.display(),
                        e
                    );
                }
            }
        }
        None => {
            eprintln!("✗ Failed to determine home directory for saving file");
        }
    }
}

// ── HTML preview ─────────────────────────────────────────────────────

pub fn show_html_preview(html: String, filename: &str) {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            eprintln!("✗ Failed to determine home directory for saving preview");
            return;
        }
    };

    let preview_path = home.join("Downloads").join(filename);
    if let Err(e) = std::fs::write(&preview_path, &html) {
        eprintln!(
            "✗ Failed to save preview to {}: {}",
            preview_path.display(),
            e
        );
        return;
    }

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg(&preview_path)
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open")
            .arg(&preview_path)
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", &preview_path.to_string_lossy()])
            .spawn();
    }
}

// ── Smart edit (LLM chat) ─────────────────────────────────────────────

/// Send one turn of a multi-turn chat to the Anthropic Messages API and return
/// the assistant's reply.
///
/// A single smart-edit session continues `history` across repair and follow-up
/// calls. `images` are `(label, base64_png)` pairs; `pdfs` are `(filename,
/// raw_bytes)` pairs attached as document inputs. `max_tokens` bounds the reply
/// length.
pub async fn chat_turn(
    history: &mut Vec<serde_json::Value>,
    user_text: &str,
    images: &[(String, String)],
    pdfs: &[(String, Vec<u8>)],
    api_key: &str,
    model: &str,
    max_tokens: u32,
) -> Result<String, String> {
    anthropic_chat_turn(history, user_text, images, pdfs, api_key, model, max_tokens).await
}

/// List the available Anthropic model identifiers.
pub async fn list_models(api_key: &str) -> Result<Vec<String>, String> {
    anthropic_list_models(api_key).await
}

/// Detect the image media type from a base64 payload by its leading bytes.
/// PNG base64 begins with `iVBOR` (`\x89PNG`); JPEG with `/9j/` (`\xff\xd8\xff`).
fn image_media_type(b64: &str) -> &'static str {
    if b64.starts_with("/9j/") {
        "image/jpeg"
    } else {
        "image/png"
    }
}

// ── Smart edit (Anthropic API) ───────────────────────────────────────

/// Format a `reqwest` error together with its underlying source chain.
///
/// `reqwest`'s top-level message is often opaque (e.g. "error decoding response
/// body"); the source chain reveals the real cause (e.g. "connection closed
/// before message completed").
fn describe_error(e: &reqwest::Error) -> String {
    use std::error::Error;
    let mut msg = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        msg.push_str(" — ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    msg
}

/// Send one turn of a multi-turn chat to the Anthropic Messages API and return
/// the assistant's reply.
///
/// Builds Anthropic-shaped content blocks and appends them to `history`, so the
/// same conversation thread continues across repair and follow-up calls within
/// a smart-edit session. Used for no-tool text turns; for tool-enabled turns see
/// [`anthropic_agentic_turn`].
pub async fn anthropic_chat_turn(
    history: &mut Vec<serde_json::Value>,
    user_text: &str,
    images: &[(String, String)],
    pdfs: &[(String, Vec<u8>)],
    api_key: &str,
    model: &str,
    max_tokens: u32,
) -> Result<String, String> {
    use base64::Engine;
    use futures_util::StreamExt;

    if api_key.is_empty() {
        return Err(
            "Anthropic API key is not configured. Open Settings and paste your API key."
                .to_string(),
        );
    }

    let mut content: Vec<serde_json::Value> =
        vec![serde_json::json!({"type": "text", "text": user_text})];

    for (_label, b64) in images {
        content.push(serde_json::json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": image_media_type(b64),
                "data": b64
            }
        }));
    }

    for (_filename, bytes) in pdfs {
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        content.push(serde_json::json!({
            "type": "document",
            "source": {
                "type": "base64",
                "media_type": "application/pdf",
                "data": b64
            }
        }));
    }

    history.push(serde_json::json!({"role": "user", "content": content}));

    // Streaming request — accumulate text deltas. Streaming avoids the
    // server timeout on long generations (e.g. whole-document AI processing).
    let request = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": history,
        "stream": true,
    });

    let response = reqwest::Client::new()
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("Anthropic API error: {}", describe_error(&e)))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(String::from))
            .unwrap_or(body);
        return Err(format!("Anthropic API error ({status}): {msg}"));
    }

    // Parse the SSE stream. Each `data:` line carries one complete JSON event;
    // accumulate `text_delta`s and surface any `error` event.
    let mut stream = response.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut response_text = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Anthropic API error: {}", describe_error(&e)))?;
        buf.extend_from_slice(&chunk);

        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line_bytes);
            let Some(data) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            match event["type"].as_str() {
                Some("content_block_delta") if event["delta"]["type"] == "text_delta" => {
                    if let Some(t) = event["delta"]["text"].as_str() {
                        response_text.push_str(t);
                    }
                }
                Some("error") => {
                    let msg = event["error"]["message"]
                        .as_str()
                        .unwrap_or("unknown error");
                    return Err(format!("Anthropic API error: {msg}"));
                }
                _ => {}
            }
        }
    }

    history.push(serde_json::json!({"role": "assistant", "content": response_text}));

    Ok(response_text)
}

// ── Agentic tool loop (Anthropic) ────────────────────────────────────

/// The result of executing one tool call, to be returned to the model as a
/// `tool_result` content block.
pub enum ToolReply {
    /// A textual result (JSON, plain text, …).
    Text(String),
    /// An image result (base64 + media type), e.g. a rendered page.
    Image {
        media_type: &'static str,
        b64: String,
    },
    /// The tool failed; the message is surfaced to the model as an error result.
    Error(String),
}

/// Maximum number of tool round-trips before the loop bails out. Guards against
/// a model that keeps calling tools without ever producing a final answer.
const MAX_TOOL_ITERATIONS: usize = 16;

/// A tool call requested by the model in one streamed turn.
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// The result of one streamed assistant turn ([`anthropic_stream_turn`]).
pub struct TurnOutput {
    /// The model's visible text for the turn.
    pub text: String,
    /// Tool calls the model requested (empty if it produced a final answer).
    pub tool_calls: Vec<ToolCall>,
    /// The turn's `stop_reason` (`"tool_use"` when tools were requested).
    pub stop_reason: Option<String>,
}

/// Run an agentic (tool-enabled) conversation turn against the Anthropic
/// Messages API and return the model's final text once it stops requesting
/// tools.
///
/// `tools` is the list of Anthropic tool definitions (`{name, description,
/// input_schema}`). On each round the model may emit `tool_use` blocks; for
/// each one `execute(name, &input)` is invoked and its [`ToolReply`] is fed back
/// as a `tool_result`. The loop continues until the model returns without a
/// `tool_use` stop reason (or [`MAX_TOOL_ITERATIONS`] is hit).
///
/// The assistant `tool_use` messages and the user `tool_result` messages are
/// appended to `history`, so a subsequent [`chat_turn`] (e.g. a repair turn)
/// continues the same thread.
pub async fn anthropic_agentic_turn(
    history: &mut Vec<serde_json::Value>,
    user_text: &str,
    api_key: &str,
    model: &str,
    max_tokens: u32,
    tools: &[serde_json::Value],
    mut execute: impl FnMut(&str, &serde_json::Value) -> ToolReply,
) -> Result<String, String> {
    use futures_util::StreamExt;

    if api_key.is_empty() {
        return Err(
            "Anthropic API key is not configured. Open Settings and paste your API key."
                .to_string(),
        );
    }

    // Seed the conversation with the user's prompt.
    history.push(serde_json::json!({
        "role": "user",
        "content": [{"type": "text", "text": user_text}],
    }));

    for _ in 0..MAX_TOOL_ITERATIONS {
        let request = serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": history,
            "tools": tools,
            "stream": true,
        });

        let response = reqwest::Client::new()
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| format!("Anthropic API error: {}", describe_error(&e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let msg = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(String::from))
                .unwrap_or(body);
            return Err(format!("Anthropic API error ({status}): {msg}"));
        }

        // Accumulate the streamed content blocks. Text deltas build up the
        // reply; `tool_use` blocks accumulate their streamed JSON input.
        let mut stream = response.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        let mut response_text = String::new();
        // index -> (id, name, partial input JSON)
        let mut tool_blocks: std::collections::BTreeMap<u64, (String, String, String)> =
            std::collections::BTreeMap::new();
        let mut stop_reason: Option<String> = None;

        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|e| format!("Anthropic API error: {}", describe_error(&e)))?;
            buf.extend_from_slice(&chunk);

            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line_bytes);
                let Some(data) = line.trim().strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data.is_empty() {
                    continue;
                }
                let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                    continue;
                };
                match event["type"].as_str() {
                    Some("content_block_start") => {
                        if event["content_block"]["type"] == "tool_use" {
                            let idx = event["index"].as_u64().unwrap_or(0);
                            let id = event["content_block"]["id"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string();
                            let name = event["content_block"]["name"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string();
                            tool_blocks.insert(idx, (id, name, String::new()));
                        }
                    }
                    Some("content_block_delta") => match event["delta"]["type"].as_str() {
                        Some("text_delta") => {
                            if let Some(t) = event["delta"]["text"].as_str() {
                                response_text.push_str(t);
                            }
                        }
                        Some("input_json_delta") => {
                            let idx = event["index"].as_u64().unwrap_or(0);
                            if let Some(entry) = tool_blocks.get_mut(&idx)
                                && let Some(pj) = event["delta"]["partial_json"].as_str()
                            {
                                entry.2.push_str(pj);
                            }
                        }
                        _ => {}
                    },
                    Some("message_delta") => {
                        if let Some(sr) = event["delta"]["stop_reason"].as_str() {
                            stop_reason = Some(sr.to_string());
                        }
                    }
                    Some("error") => {
                        let msg = event["error"]["message"]
                            .as_str()
                            .unwrap_or("unknown error");
                        return Err(format!("Anthropic API error: {msg}"));
                    }
                    _ => {}
                }
            }
        }

        // Rebuild the assistant message content array in block order: the text
        // block (if any) followed by each tool_use block with parsed input.
        let mut assistant_content: Vec<serde_json::Value> = Vec::new();
        if !response_text.is_empty() {
            assistant_content.push(serde_json::json!({"type": "text", "text": response_text}));
        }
        for (_idx, (id, name, input_buf)) in &tool_blocks {
            let input: serde_json::Value = serde_json::from_str(input_buf)
                .unwrap_or_else(|_| serde_json::json!({}));
            assistant_content.push(serde_json::json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input,
            }));
        }
        history.push(serde_json::json!({
            "role": "assistant",
            "content": assistant_content,
        }));

        // Done unless the model asked for tools.
        if stop_reason.as_deref() != Some("tool_use") || tool_blocks.is_empty() {
            return Ok(response_text);
        }

        // Execute each requested tool and return the results as a single user
        // message of tool_result blocks.
        let mut result_content: Vec<serde_json::Value> = Vec::new();
        for (_idx, (id, name, input_buf)) in &tool_blocks {
            let input: serde_json::Value =
                serde_json::from_str(input_buf).unwrap_or_else(|_| serde_json::json!({}));
            let block = match execute(name, &input) {
                ToolReply::Text(text) => serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": [{"type": "text", "text": text}],
                }),
                ToolReply::Image { media_type, b64 } => serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": [{
                        "type": "image",
                        "source": {"type": "base64", "media_type": media_type, "data": b64},
                    }],
                }),
                ToolReply::Error(msg) => serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": id,
                    "is_error": true,
                    "content": [{"type": "text", "text": msg}],
                }),
            };
            result_content.push(block);
        }
        history.push(serde_json::json!({
            "role": "user",
            "content": result_content,
        }));
    }

    Err(format!(
        "Anthropic tool loop did not converge after {MAX_TOOL_ITERATIONS} iterations."
    ))
}

/// Run **one** streamed assistant turn against the Messages API with `tools`
/// available, append the assistant message to `history`, and return its text +
/// any tool calls + `stop_reason`. The caller drives the multi-turn agent loop:
/// it executes the returned tool calls (which may be async) and appends a user
/// `tool_result` message via [`tool_result_message`] before the next turn.
pub async fn anthropic_stream_turn(
    history: &mut Vec<serde_json::Value>,
    tools: &[serde_json::Value],
    api_key: &str,
    model: &str,
    max_tokens: u32,
) -> Result<TurnOutput, String> {
    use futures_util::StreamExt;

    if api_key.is_empty() {
        return Err(
            "Anthropic API key is not configured. Open Settings and paste your API key."
                .to_string(),
        );
    }

    // Prompt caching: place `ephemeral` cache breakpoints so the stable prefix
    // (tool schemas + the system/instruction prefix + all prior turns) is billed
    // at the reduced cache-read rate on each turn instead of full input price.
    //
    // 1. Mark the last tool definition. The tool block is identical for the whole
    //    run, so this caches the entire tools array permanently (within the TTL).
    // 2. Mark the final content block of the conversation. Anthropic matches the
    //    longest previously-cached prefix, so moving this breakpoint to the new
    //    tail each turn reuses everything cached on earlier turns (rolling cache).
    let cache_control = serde_json::json!({"type": "ephemeral"});

    let mut tools_cached = tools.to_vec();
    if let Some(last_tool) = tools_cached.last_mut().and_then(|t| t.as_object_mut()) {
        last_tool.insert("cache_control".to_string(), cache_control.clone());
    }

    let mut messages = history.clone();
    if let Some(last_block) = messages
        .last_mut()
        .and_then(|m| m.get_mut("content"))
        .and_then(|c| c.as_array_mut())
        .and_then(|blocks| blocks.last_mut())
        .and_then(|b| b.as_object_mut())
    {
        last_block.insert("cache_control".to_string(), cache_control.clone());
    }

    let request = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": messages,
        "tools": tools_cached,
        "stream": true,
    });

    let response = reqwest::Client::new()
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("Anthropic API error: {}", describe_error(&e)))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(String::from))
            .unwrap_or(body);
        return Err(format!("Anthropic API error ({status}): {msg}"));
    }

    let mut stream = response.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut response_text = String::new();
    let mut tool_blocks: std::collections::BTreeMap<u64, (String, String, String)> =
        std::collections::BTreeMap::new();
    let mut stop_reason: Option<String> = None;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Anthropic API error: {}", describe_error(&e)))?;
        buf.extend_from_slice(&chunk);

        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line_bytes);
            let Some(data) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            match event["type"].as_str() {
                Some("content_block_start") => {
                    if event["content_block"]["type"] == "tool_use" {
                        let idx = event["index"].as_u64().unwrap_or(0);
                        let id = event["content_block"]["id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        let name = event["content_block"]["name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        tool_blocks.insert(idx, (id, name, String::new()));
                    }
                }
                Some("content_block_delta") => match event["delta"]["type"].as_str() {
                    Some("text_delta") => {
                        if let Some(t) = event["delta"]["text"].as_str() {
                            response_text.push_str(t);
                        }
                    }
                    Some("input_json_delta") => {
                        let idx = event["index"].as_u64().unwrap_or(0);
                        if let Some(entry) = tool_blocks.get_mut(&idx)
                            && let Some(pj) = event["delta"]["partial_json"].as_str()
                        {
                            entry.2.push_str(pj);
                        }
                    }
                    _ => {}
                },
                Some("message_delta") => {
                    if let Some(sr) = event["delta"]["stop_reason"].as_str() {
                        stop_reason = Some(sr.to_string());
                    }
                }
                Some("error") => {
                    let msg = event["error"]["message"]
                        .as_str()
                        .unwrap_or("unknown error");
                    return Err(format!("Anthropic API error: {msg}"));
                }
                _ => {}
            }
        }
    }

    // Append the assistant message (text + tool_use blocks) to history.
    let mut assistant_content: Vec<serde_json::Value> = Vec::new();
    if !response_text.is_empty() {
        assistant_content.push(serde_json::json!({"type": "text", "text": response_text}));
    }
    let mut tool_calls = Vec::new();
    for (_idx, (id, name, input_buf)) in &tool_blocks {
        let input: serde_json::Value =
            serde_json::from_str(input_buf).unwrap_or_else(|_| serde_json::json!({}));
        assistant_content.push(serde_json::json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input,
        }));
        tool_calls.push(ToolCall {
            id: id.clone(),
            name: name.clone(),
            input,
        });
    }
    history.push(serde_json::json!({
        "role": "assistant",
        "content": assistant_content,
    }));

    Ok(TurnOutput {
        text: response_text,
        tool_calls,
        stop_reason,
    })
}

/// Build the user `tool_result` message for a batch of executed tool calls.
/// Each entry is `(tool_use_id, ToolReply)`. Append the result to `history`
/// before the next [`anthropic_stream_turn`].
pub fn tool_result_message(results: Vec<(String, ToolReply)>) -> serde_json::Value {
    let content: Vec<serde_json::Value> = results
        .into_iter()
        .map(|(id, reply)| match reply {
            ToolReply::Text(text) => serde_json::json!({
                "type": "tool_result",
                "tool_use_id": id,
                "content": [{"type": "text", "text": text}],
            }),
            ToolReply::Image { media_type, b64 } => serde_json::json!({
                "type": "tool_result",
                "tool_use_id": id,
                "content": [{
                    "type": "image",
                    "source": {"type": "base64", "media_type": media_type, "data": b64},
                }],
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

/// Fetch the list of available model IDs from the Anthropic API, sorted
/// alphabetically. All Claude models support the chat + vision endpoint, so no
/// filtering is applied.
pub async fn anthropic_list_models(api_key: &str) -> Result<Vec<String>, String> {
    if api_key.is_empty() {
        return Err("Anthropic API key is not configured.".to_string());
    }

    let response = reqwest::Client::new()
        .get("https://api.anthropic.com/v1/models?limit=1000")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .map_err(|e| format!("Failed to list models: {e}"))?;

    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to list models: {e}"))?;

    if !status.is_success() {
        let msg = body["error"]["message"]
            .as_str()
            .unwrap_or("unknown error");
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

// ── File explorer reveal ─────────────────────────────────────────────

pub fn reveal_in_file_explorer(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(parent) = path.parent() {
            let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
        }
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer")
            .args(&["/select,", &path.to_string_lossy()])
            .spawn();
    }
}
