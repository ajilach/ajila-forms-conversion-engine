//! Platform-aware helpers: async sleep, file download, HTML preview, file explorer.

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

    // Bound context growth on long repair threads before adding this turn.
    evict_stale_history(history);

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

// `ToolReply` is the engine executor's return type; it lives in the headless
// `agent` crate. Re-export so `crate::platform::ToolReply` keeps resolving.
pub use agent::ToolReply;

/// Maximum number of tool round-trips before the loop bails out. Guards against
/// a model that keeps calling tools without ever producing a final answer.
const MAX_TOOL_ITERATIONS: usize = 16;

// ── Prompt caching + history eviction (shared by every Messages-API path) ────

use std::sync::atomic::{AtomicUsize, Ordering};

/// Default trailing messages kept verbatim by [`evict_stale_history`]. Even, so
/// whole assistant+`tool_result` turn-pairs survive (the latest data stays
/// intact). Overridable at runtime via [`configure_eviction`].
pub const DEFAULT_KEEP_RECENT_MESSAGES: usize = 4;
/// Default: tool-result text longer than this (chars) is elided once stale.
pub const DEFAULT_ELIDE_TEXT_OVER_CHARS: usize = 2000;
/// Default: `tool_use` input longer than this (chars) is elided once stale.
pub const DEFAULT_ELIDE_INPUT_OVER_CHARS: usize = 2000;
/// Default: eviction is a no-op until the serialized history exceeds this size,
/// so short calls (most smart-edits, chat turns) are never touched.
pub const DEFAULT_EVICT_TRIGGER_BYTES: usize = 200_000;
/// Sentinel prefix marking an already-elided block. Makes eviction idempotent:
/// repeated passes are byte-identical, so the cached prefix is not invalidated.
const ELIDED_MARKER: &str = "\u{1}elided";

// Live, runtime-configurable eviction tuning (synced from `AppSettings`).
static CFG_KEEP_RECENT: AtomicUsize = AtomicUsize::new(DEFAULT_KEEP_RECENT_MESSAGES);
static CFG_TEXT_OVER: AtomicUsize = AtomicUsize::new(DEFAULT_ELIDE_TEXT_OVER_CHARS);
static CFG_INPUT_OVER: AtomicUsize = AtomicUsize::new(DEFAULT_ELIDE_INPUT_OVER_CHARS);
static CFG_TRIGGER_BYTES: AtomicUsize = AtomicUsize::new(DEFAULT_EVICT_TRIGGER_BYTES);

/// Override the history-eviction tuning (called from settings on startup and on
/// change). A `0` argument resets that parameter to its default. `keep_recent`
/// is clamped to an even number ≥ 2 so whole turn-pairs always stay verbatim.
pub fn configure_eviction(
    keep_recent: usize,
    text_over: usize,
    input_over: usize,
    trigger_bytes: usize,
) {
    let keep = if keep_recent == 0 {
        DEFAULT_KEEP_RECENT_MESSAGES
    } else {
        (keep_recent + (keep_recent & 1)).max(2) // round up to even, min 2
    };
    CFG_KEEP_RECENT.store(keep, Ordering::Relaxed);
    CFG_TEXT_OVER.store(
        if text_over == 0 {
            DEFAULT_ELIDE_TEXT_OVER_CHARS
        } else {
            text_over
        },
        Ordering::Relaxed,
    );
    CFG_INPUT_OVER.store(
        if input_over == 0 {
            DEFAULT_ELIDE_INPUT_OVER_CHARS
        } else {
            input_over
        },
        Ordering::Relaxed,
    );
    CFG_TRIGGER_BYTES.store(
        if trigger_bytes == 0 {
            DEFAULT_EVICT_TRIGGER_BYTES
        } else {
            trigger_bytes
        },
        Ordering::Relaxed,
    );
}

/// Shrink heavy, stale content in `history` **in place** to bound context growth
/// on long tool loops. Older base64 images, oversized `tool_result` text, and
/// oversized `set_*` tool inputs are replaced with short stubs; the model can
/// re-fetch the real data via tools (the engine + SQLite are the source of
/// truth, not the transcript). Blocks are never removed, so the API's
/// `tool_use`↔`tool_result` pairing stays intact.
///
/// Protects `history[0]` (the instruction prefix) and the last
/// [`DEFAULT_KEEP_RECENT_MESSAGES`] messages. Size-gated by [`EVICT_TRIGGER_BYTES`] and
/// idempotent (already-stubbed blocks are skipped), so it cooperates with prompt
/// caching instead of busting the cached prefix.
fn evict_stale_history(history: &mut [serde_json::Value]) {
    let keep_recent = CFG_KEEP_RECENT.load(Ordering::Relaxed);
    let text_over = CFG_TEXT_OVER.load(Ordering::Relaxed);
    let input_over = CFG_INPUT_OVER.load(Ordering::Relaxed);
    let trigger_bytes = CFG_TRIGGER_BYTES.load(Ordering::Relaxed);

    let total = serde_json::to_string(&history)
        .map(|s| s.len())
        .unwrap_or(0);
    if total < trigger_bytes {
        return;
    }
    let len = history.len();
    if len <= 1 + keep_recent {
        return;
    }
    let cutoff = len - keep_recent; // index >= cutoff is protected

    // Pass 1 (read-only): map tool_use_id -> tool name, to label result stubs.
    let mut names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for msg in history.iter() {
        let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                && let (Some(id), Some(name)) = (
                    block.get("id").and_then(|v| v.as_str()),
                    block.get("name").and_then(|v| v.as_str()),
                )
            {
                names.insert(id.to_string(), name.to_string());
            }
        }
    }

    // Pass 2 (mutate): elide older messages, skipping index 0 + the recent tail.
    for msg in history.iter_mut().take(cutoff).skip(1) {
        let Some(blocks) = msg.get_mut("content").and_then(|c| c.as_array_mut()) else {
            continue;
        };
        for block in blocks.iter_mut() {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("tool_use") => elide_tool_use(block, input_over),
                Some("tool_result") => elide_tool_result(block, &names, text_over),
                _ => {}
            }
        }
    }
}

/// Replace an oversized `tool_use` `input` with a small stub object. `input`
/// must stay a JSON object; history replay does not re-validate it against the
/// tool schema, so the stub is safe.
fn elide_tool_use(block: &mut serde_json::Value, input_over: usize) {
    let Some(input) = block.get("input") else {
        return;
    };
    if input.get("_elided").is_some() {
        return; // already stubbed
    }
    let size = serde_json::to_string(input).map(|s| s.len()).unwrap_or(0);
    if size <= input_over {
        return;
    }
    let name = block
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("tool")
        .to_string();
    block["input"] = serde_json::json!({
        "_elided": ELIDED_MARKER,
        "note": format!("{name} input elided: {size} chars — re-read current state with a get_* tool"),
    });
}

/// Elide the inner blocks of a stale `tool_result`: images become a text stub,
/// oversized text is truncated to a stub. Preserves `is_error` / `tool_use_id`
/// and keeps at least one block (never an empty `content` array).
fn elide_tool_result(
    block: &mut serde_json::Value,
    names: &std::collections::HashMap<String, String>,
    text_over: usize,
) {
    let tool = block
        .get("tool_use_id")
        .and_then(|v| v.as_str())
        .and_then(|id| names.get(id))
        .map(|s| s.as_str())
        .unwrap_or("tool")
        .to_string();
    let Some(inner) = block.get_mut("content").and_then(|c| c.as_array_mut()) else {
        return;
    };
    for b in inner.iter_mut() {
        match b.get("type").and_then(|t| t.as_str()) {
            Some("image") => {
                *b = serde_json::json!({
                    "type": "text",
                    "text": format!("{ELIDED_MARKER} image elided — re-fetch with the tool if needed"),
                });
            }
            Some("text") => {
                let text = b.get("text").and_then(|t| t.as_str()).unwrap_or("");
                if text.starts_with(ELIDED_MARKER) || text.len() <= text_over {
                    continue;
                }
                let n = text.len();
                b["text"] = serde_json::Value::String(format!(
                    "{ELIDED_MARKER} {tool} output elided: {n} chars — re-read for current state"
                ));
            }
            _ => {}
        }
    }
}

/// Clone `history` and tag the final content block with an `ephemeral`
/// cache_control breakpoint. Anthropic matches the longest previously-cached
/// prefix, so moving the breakpoint to the new tail each turn reuses earlier
/// turns' cache (rolling cache).
fn cache_marked_messages(history: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let mut messages = history.to_vec();
    if let Some(last_block) = messages
        .last_mut()
        .and_then(|m| m.get_mut("content"))
        .and_then(|c| c.as_array_mut())
        .and_then(|blocks| blocks.last_mut())
        .and_then(|b| b.as_object_mut())
    {
        last_block.insert(
            "cache_control".to_string(),
            serde_json::json!({"type": "ephemeral"}),
        );
    }
    messages
}

/// Clone `tools` and tag the last tool definition with an `ephemeral`
/// cache_control breakpoint. The tool block is identical for the whole run, so
/// this caches the entire tools array.
fn cache_marked_tools(tools: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let mut tools_cached = tools.to_vec();
    if let Some(last_tool) = tools_cached.last_mut().and_then(|t| t.as_object_mut()) {
        last_tool.insert(
            "cache_control".to_string(),
            serde_json::json!({"type": "ephemeral"}),
        );
    }
    tools_cached
}

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
    // Seed the conversation with the user's prompt.
    history.push(serde_json::json!({
        "role": "user",
        "content": [{"type": "text", "text": user_text}],
    }));

    // Thin loop over the shared turn primitive: [`anthropic_stream_turn`] owns
    // request building, SSE parsing, prompt caching and history eviction. Here
    // we only drive the tool round-trips with the synchronous `execute` closure,
    // reusing [`tool_result_message`] for the result blocks.
    for _ in 0..MAX_TOOL_ITERATIONS {
        let turn = anthropic_stream_turn(history, tools, api_key, model, max_tokens).await?;

        // Done unless the model asked for tools.
        if turn.stop_reason.as_deref() != Some("tool_use") || turn.tool_calls.is_empty() {
            return Ok(turn.text);
        }

        let results: Vec<(String, ToolReply)> = turn
            .tool_calls
            .iter()
            .map(|tc| (tc.id.clone(), execute(&tc.name, &tc.input)))
            .collect();
        history.push(tool_result_message(results));
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

    // Bound context growth before billing this turn (no-op until history is
    // large; see [`evict_stale_history`]). Every caller runs through here.
    evict_stale_history(history);

    // Prompt caching: `ephemeral` breakpoints on the last tool and the final
    // message block bill the stable prefix (tools + instruction prefix + prior
    // turns) at the cache-read rate. See [`cache_marked_messages`] /
    // [`cache_marked_tools`].
    let request = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": cache_marked_messages(history),
        "tools": cache_marked_tools(tools),
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
                Some("content_block_start") if event["content_block"]["type"] == "tool_use" => {
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
    for (id, name, input_buf) in tool_blocks.values() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user_text(text: &str) -> serde_json::Value {
        json!({"role": "user", "content": [{"type": "text", "text": text}]})
    }
    fn assistant_tool_use(id: &str, name: &str, input: serde_json::Value) -> serde_json::Value {
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": id, "name": name, "input": input},
        ]})
    }
    fn result_text(id: &str, text: &str) -> serde_json::Value {
        json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": id, "content": [{"type": "text", "text": text}]},
        ]})
    }
    fn result_image(id: &str, data: &str) -> serde_json::Value {
        json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": id, "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": data}},
            ]},
        ]})
    }

    /// History with stale heavy content (big image, big text, big tool input) in
    /// old turns and a small recent turn-pair. Total exceeds the size gate.
    fn big_history() -> Vec<serde_json::Value> {
        let big_input = json!({"tree": "X".repeat(3000)});
        vec![
            user_text("SYSTEM PROMPT"),                             // 0 protected
            assistant_tool_use("tu1", "set_structured", big_input), // 1 evict
            result_image("tu1", &"A".repeat(250_000)),              // 2 evict (drives size)
            assistant_tool_use("tu2", "get_xfa", json!({})),        // 3 evict
            result_text("tu2", &"x".repeat(5000)),                  // 4 evict
            assistant_tool_use("tu3", "get_structured", json!({})), // 5 recent
            result_text("tu3", "small recent result"),              // 6 recent
            assistant_tool_use("tu4", "finish", json!({})),         // 7 recent
            result_text("tu4", "done"),                             // 8 recent
        ]
    }

    fn block_text(msg: &serde_json::Value) -> String {
        msg["content"][0]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    #[test]
    fn evicts_stale_protects_recent() {
        let original = big_history();
        let mut h = original.clone();
        evict_stale_history(&mut h);

        // Index 0 and the last DEFAULT_KEEP_RECENT_MESSAGES are byte-identical.
        assert_eq!(h[0], original[0]);
        let len = h.len();
        for i in (len - DEFAULT_KEEP_RECENT_MESSAGES)..len {
            assert_eq!(h[i], original[i], "recent message {i} changed");
        }

        // Old set_structured input is stubbed to an object with `_elided`.
        assert!(h[1]["content"][0]["input"].get("_elided").is_some());
        assert!(h[1]["content"][0]["input"].is_object());

        // Old image became a text stub; old big text shrank to a marker stub.
        assert!(block_text(&h[2]).starts_with(ELIDED_MARKER));
        assert!(block_text(&h[2]).contains("image elided"));
        assert!(block_text(&h[4]).starts_with(ELIDED_MARKER));
        // The stub names the originating tool (get_xfa).
        assert!(block_text(&h[4]).contains("get_xfa"));
    }

    #[test]
    fn pairing_preserved() {
        let mut h = big_history();
        evict_stale_history(&mut h);
        let count = |role: &str, kind: &str| {
            h.iter()
                .filter(|m| m["role"] == role)
                .flat_map(|m| m["content"].as_array().unwrap().clone())
                .filter(|b| b["type"] == kind)
                .count()
        };
        // No blocks deleted: every tool_use still has its tool_result.
        assert_eq!(count("assistant", "tool_use"), 4);
        assert_eq!(count("user", "tool_result"), 4);
    }

    #[test]
    fn idempotent() {
        let mut once = big_history();
        evict_stale_history(&mut once);
        let mut twice = once.clone();
        evict_stale_history(&mut twice);
        assert_eq!(once, twice, "second pass must be a no-op");
    }

    #[test]
    fn size_gated_below_threshold() {
        // A small history (well under EVICT_TRIGGER_BYTES) is left untouched even
        // though it contains an over-threshold text block.
        let original = vec![
            user_text("SYSTEM"),
            assistant_tool_use("tu1", "get_xfa", json!({})),
            result_text("tu1", &"x".repeat(DEFAULT_ELIDE_TEXT_OVER_CHARS + 100)),
            assistant_tool_use("tu2", "get_structured", json!({})),
            result_text("tu2", "recent"),
            assistant_tool_use("tu3", "finish", json!({})),
            result_text("tu3", "done"),
        ];
        let mut h = original.clone();
        evict_stale_history(&mut h);
        assert_eq!(h, original);
    }
}
