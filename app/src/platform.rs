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

    // Bound the prompt to the model's context window — a no-op while under budget
    // (so short repair threads keep full context) and escalating only when over.
    // No tools/system on this path. Histories here are small, so this stays on the
    // caller's thread rather than off-loading like the agent's streaming turn.
    evict_to_fit(history, &[], None, prompt_token_target(model, max_tokens));

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

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Default trailing messages kept verbatim by [`evict_stale_history_with`]. Even,
/// so whole assistant+`tool_result` turn-pairs survive (the latest data stays
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

/// Build a `tool_use_id -> tool name` map over the whole transcript. Shared by
/// the eviction passes that need to label or target results by their originating
/// tool.
fn tool_name_by_id(history: &[serde_json::Value]) -> std::collections::HashMap<String, String> {
    let mut names = std::collections::HashMap::new();
    for msg in history.iter() {
        for block in msg
            .get("content")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
        {
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
    names
}

/// Shrink heavy, stale content in `history` **in place** to bound context growth
/// on long tool loops. Older base64 images, oversized `tool_result` text, and
/// oversized `set_*` tool inputs are replaced with short stubs; the model can
/// re-fetch the real data via tools (the engine + SQLite are the source of
/// truth, not the transcript). Blocks are never removed, so the API's
/// `tool_use`↔`tool_result` pairing stays intact.
///
/// Protects `history[0]` (the instruction prefix) and the last `keep_recent`
/// messages. Size-gated by `trigger_bytes` and idempotent (already-stubbed blocks
/// are skipped), so it cooperates with prompt caching instead of busting the
/// cached prefix. [`evict_to_fit`] drives it with escalating tuning (a tiny recent
/// window, near-zero thresholds, and `trigger_bytes = 0`) as a turn approaches the
/// context window.
fn evict_stale_history_with(
    history: &mut [serde_json::Value],
    keep_recent: usize,
    text_over: usize,
    input_over: usize,
    trigger_bytes: usize,
) {
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
    let names = tool_name_by_id(history);

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

/// Tokens reserved below the context window for the model's own reply plus
/// estimation slack, so the assembled prompt lands comfortably under the hard
/// limit even when the char-based estimate runs low.
const CONTEXT_SAFETY_MARGIN: usize = 48_000;

/// Context window learned from the API (parsed from a `prompt is too long: …
/// > N maximum` error). `0` means "not learned yet — use the heuristic". This is
/// authoritative once set, so a wrong heuristic guess self-corrects after at most
/// one overflow. See [`learn_context_window_from_error`].
static CFG_CONTEXT_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// The model's maximum context window in tokens.
///
/// Prefer the value learned from the API; otherwise fall back to a heuristic that
/// is **optimistic** — modern large-context families default to 1M. Guessing high
/// is the safe direction: too-high costs at most one `400` (caught and learned
/// from by [`anthropic_stream_turn`]), whereas too-low silently shrinks the budget
/// and makes the agent evict its own context every turn (an amnesia loop). Only
/// known-small models (Haiku, pre-4 families) default to 200K.
pub fn context_window_for(model: &str) -> usize {
    let learned = CFG_CONTEXT_WINDOW.load(Ordering::Relaxed);
    if learned > 0 {
        return learned;
    }
    let m = model.to_ascii_lowercase();
    let large = m.contains("[1m]")
        || m.contains("-1m")
        || m.contains("opus-4")
        || m.contains("sonnet-4")
        || m.contains("sonnet-5")
        || m.contains("fable-5");
    if large { 1_000_000 } else { 200_000 }
}

/// Learn the real context window from a `prompt is too long: X tokens > N maximum`
/// error, clamping the stored window to the smallest maximum the API has reported.
fn learn_context_window_from_error(msg: &str) {
    let Some(n) = msg
        .split('>')
        .nth(1)
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|tok| tok.parse::<usize>().ok())
    else {
        return;
    };
    let prev = CFG_CONTEXT_WINDOW.load(Ordering::Relaxed);
    if prev == 0 || n < prev {
        CFG_CONTEXT_WINDOW.store(n, Ordering::Relaxed);
    }
}

/// Fallback per-image token cost when the base64 can't be decoded to read
/// dimensions (near the observed max, so a fallback errs high/safe).
const IMAGE_TOKEN_FALLBACK: usize = 1_600;
/// Anthropic's vision cost is `(width * height) / PX_PER_TOKEN`, after the image
/// is downscaled to at most `MAX_IMAGE_PX` pixels — so the cost is bounded at
/// `MAX_IMAGE_PX / PX_PER_TOKEN` (~1533).
const PX_PER_TOKEN: usize = 750;
const MAX_IMAGE_PX: usize = 1_150_000;

/// Real vision-token cost of one base64 image: decode just enough to read its
/// dimensions, then apply Anthropic's `min(w*h, MAX_IMAGE_PX) / PX_PER_TOKEN`.
/// Falls back to [`IMAGE_TOKEN_FALLBACK`] if the payload can't be read.
fn image_token_cost(data_b64: &str) -> usize {
    use base64::Engine;
    let Ok(bytes) = base64::prelude::BASE64_STANDARD.decode(data_b64) else {
        return IMAGE_TOKEN_FALLBACK;
    };
    match image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
    {
        Some((w, h)) => (w as usize * h as usize).min(MAX_IMAGE_PX) / PX_PER_TOKEN,
        None => IMAGE_TOKEN_FALLBACK,
    }
}

/// Total base64 payload length across all image blocks in `v` (to subtract from
/// the byte-based estimate) and their combined real vision-token cost (to add
/// back). Counting images by base64 length would dwarf everything and skew both
/// the budget and [`token_calibration`].
fn image_payload_stats(v: &serde_json::Value) -> (usize, usize) {
    match v {
        serde_json::Value::Object(o)
            if o.get("type").and_then(|t| t.as_str()) == Some("image") =>
        {
            let data = o
                .get("source")
                .and_then(|s| s.get("data"))
                .and_then(|d| d.as_str())
                .unwrap_or_default();
            (data.len(), image_token_cost(data))
        }
        serde_json::Value::Object(o) => o
            .values()
            .map(image_payload_stats)
            .fold((0, 0), |(a, b), (c, d)| (a + c, b + d)),
        serde_json::Value::Array(a) => a
            .iter()
            .map(image_payload_stats)
            .fold((0, 0), |(a, b), (c, d)| (a + c, b + d)),
        _ => (0, 0),
    }
}

/// Token estimate for a JSON value: serialized byte length ÷ 4 (the byte count
/// already includes every brace/quote/colon, so this tracks real tokens well for
/// text and JSON alike), except that image blocks are counted at their real
/// vision cost via [`image_token_cost`] rather than their base64 length. The
/// figure is scaled by the calibration factor before being compared to a budget.
fn estimate_tokens(v: &serde_json::Value) -> usize {
    let total = serde_json::to_string(v).map(|s| s.len()).unwrap_or(0);
    let (image_bytes, image_tokens) = image_payload_stats(v);
    total.saturating_sub(image_bytes) / 4 + image_tokens
}

/// Calibration factor (real prompt tokens ÷ raw char-based estimate), in
/// thousandths. Learned from the API's reported `usage`; starts at 1.0 and is
/// nudged toward each turn's observed ratio (see [`record_token_calibration`]).
static CFG_TOKEN_CALIBRATION_MILLI: AtomicU64 = AtomicU64::new(1000);

/// The current calibration factor as a float (defaults to 1.0).
fn token_calibration() -> f64 {
    CFG_TOKEN_CALIBRATION_MILLI.load(Ordering::Relaxed) as f64 / 1000.0
}

/// A raw estimate scaled by the learned calibration factor — the actual token
/// count we predict the API will bill for `raw` estimated tokens.
fn calibrated_tokens(raw: usize) -> usize {
    (raw as f64 * token_calibration()) as usize
}

/// Fold one observed (real prompt tokens, raw estimate) pair into the calibration
/// factor with an EMA. Clamped to `[1.0, 8.0]`: the factor may only ever make us
/// evict *more*, never less — under-counting is the direction that overflows the
/// context window, so calibration is not allowed to shrink the estimate below its
/// raw value. `real` comes from the API's `usage` (input + both cache buckets);
/// `estimate` is the [`estimate_tokens`] of the same assembled prompt.
fn record_token_calibration(real: usize, estimate: usize) {
    if real == 0 || estimate == 0 {
        return;
    }
    let observed = (real as f64 / estimate as f64).clamp(1.0, 8.0);
    let blended = (token_calibration() * 0.7 + observed * 0.3).clamp(1.0, 8.0);
    CFG_TOKEN_CALIBRATION_MILLI.store((blended * 1000.0) as u64, Ordering::Relaxed);
}

/// Raw char-based token estimate for the whole assembled prompt: messages +
/// `tools` + `system`.
fn assembled_prompt_estimate(
    history: &[serde_json::Value],
    tools: &[serde_json::Value],
    system: Option<&str>,
) -> usize {
    estimate_tokens(&serde_json::Value::Array(tools.to_vec()))
        + system.map_or(0, |s| s.len() / 4)
        + history.iter().map(estimate_tokens).sum::<usize>()
}

/// The estimated-token budget for the assembled prompt (messages + tools +
/// system), leaving room for the reply and estimation slack.
pub fn prompt_token_target(model: &str, max_tokens: u32) -> usize {
    context_window_for(model).saturating_sub(max_tokens as usize + CONTEXT_SAFETY_MARGIN)
}

/// Shrink `history` in place until the assembled prompt (messages + `tools` +
/// `system`) is estimated to fit under `target` input tokens. Does **nothing**
/// while the prompt is under budget — so with a 1M-token window, context is kept
/// intact right up to the limit; nothing is stubbed prematurely. Only once over
/// budget does it escalate through four stages, reusing the stubbing pass:
///   1. Normal-tuned stubbing (large stale blocks; keeps the recent window).
///   2. Aggressive stubbing — tiny thresholds and a minimal recent window — so
///      even recent / smaller heavy blocks are stubbed.
///   3. Sliding window — drop the oldest `(assistant, user)` turn-pairs, the only
///      lever that removes rather than shrinks, keeping `history[0]` and the most
///      recent pair, until it fits or nothing more is safe to drop.
///   4. Last resort — stub the most-recent pair too (no protected window), so a
///      single oversized tool result / tool input can't blow the limit alone.
fn evict_to_fit(
    history: &mut Vec<serde_json::Value>,
    tools: &[serde_json::Value],
    system: Option<&str>,
    target: usize,
) {
    let fits = |h: &[serde_json::Value]| {
        calibrated_tokens(assembled_prompt_estimate(h, tools, system)) <= target
    };

    // Under budget → keep the full transcript verbatim. This is the common case
    // and MUST not touch anything: stubbing while there is ample token headroom
    // (the old byte-gated behaviour) makes the agent re-fetch context it still
    // had room for. All stages below force-run (`trigger_bytes = 0`) because we
    // only reach them once genuinely over budget.
    if fits(history) {
        return;
    }

    // Stage 1: normal-tuned stubbing (stale big blocks; keep the recent window).
    evict_stale_history_with(
        history,
        CFG_KEEP_RECENT.load(Ordering::Relaxed),
        CFG_TEXT_OVER.load(Ordering::Relaxed),
        CFG_INPUT_OVER.load(Ordering::Relaxed),
        0,
    );
    if fits(history) {
        return;
    }

    // Stage 2: aggressive stubbing (keep only the last pair verbatim; stub any
    // text/input over ~200 chars).
    evict_stale_history_with(history, 2, 200, 200, 0);
    if fits(history) {
        return;
    }

    // Stage 3: drop oldest turn-pairs. Keep `history[0]` (the kickoff/user
    // prefix) and at least the most recent pair; stop if the head isn't a clean
    // (assistant, user) pair, rather than risk breaking tool_use↔tool_result
    // pairing.
    while !fits(history) && history.len() > 3 {
        let head_is_pair = history.get(1).and_then(|m| m["role"].as_str()) == Some("assistant")
            && history.get(2).and_then(|m| m["role"].as_str()) == Some("user");
        if !head_is_pair {
            break;
        }
        history.drain(1..3);
    }
    if fits(history) {
        return;
    }

    // Stage 4: nothing left to drop but still over budget — a single recent block
    // (e.g. a whole-XFA `get_xfa`, or a monolithic `set_*` input) exceeds it on
    // its own. Stub with no protected window (`keep_recent = 0`) so even the last
    // pair is shrunk; the model re-fetches from the engine if it still needs it.
    evict_stale_history_with(history, 0, 200, 200, 0);
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

/// Build the `system` request field as a single text block with a static
/// `ephemeral` cache_control breakpoint. The instruction prefix is byte-identical
/// on every turn of a run, so this caches it independently of the rolling message
/// cache — a stall past the cache TTL then only re-writes the message tail.
fn cache_marked_system(system: &str) -> serde_json::Value {
    serde_json::json!([{
        "type": "text",
        "text": system,
        "cache_control": {"type": "ephemeral"},
    }])
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
    /// Real prompt-token count the API billed for this turn's request (uncached
    /// input + both cache buckets) — i.e. how full the context window was. 0 if
    /// the API didn't report usage.
    pub prompt_tokens: usize,
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
        let turn = anthropic_stream_turn(history, tools, api_key, model, max_tokens, None).await?;

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

/// The output of [`prepare_request_body`]: the (possibly evicted) history, the
/// serialized request bytes, and the raw token estimate of what was assembled.
struct PreparedRequest {
    history: Vec<serde_json::Value>,
    body: Vec<u8>,
    sent_estimate: usize,
}

/// Do the CPU-heavy prompt assembly off the UI thread: evict to fit the target,
/// clone in the cache-control breakpoints, and serialize the request to bytes.
///
/// The agent loop is spawned on Dioxus's main-thread executor, so running this
/// synchronously (full-history serialization + recursive token walks + a deep
/// clone of a multi-MB history, several times per turn) freezes rendering. It
/// all happens inside [`tokio::task::spawn_blocking`] instead. Ownership of
/// `history` is moved in and returned so the caller can restore it.
async fn prepare_request_body(
    history: Vec<serde_json::Value>,
    tools: Vec<serde_json::Value>,
    system: Option<String>,
    model: String,
    max_tokens: u32,
    target: usize,
) -> Result<PreparedRequest, String> {
    tokio::task::spawn_blocking(move || {
        let mut history = history;
        evict_to_fit(&mut history, &tools, system.as_deref(), target);
        let sent_estimate = assembled_prompt_estimate(&history, &tools, system.as_deref());

        // Prompt caching: `ephemeral` breakpoints on the system prompt (static —
        // the instruction prefix is identical every turn), the last tool, and the
        // final message block bill the stable prefix at the cache-read rate. The
        // static system breakpoint means a >5-min stall (cache TTL) only re-writes
        // the rolling tail, never the whole prefix. See [`cache_marked_messages`]
        // / [`cache_marked_tools`] / [`cache_marked_system`].
        let mut request = serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": cache_marked_messages(&history),
            "tools": cache_marked_tools(&tools),
            "stream": true,
        });
        if let Some(s) = system.as_deref().filter(|s| !s.is_empty()) {
            request["system"] = cache_marked_system(s);
        }
        let body = serde_json::to_vec(&request)
            .map_err(|e| format!("Failed to serialize request: {e}"))?;
        Ok(PreparedRequest {
            history,
            body,
            sent_estimate,
        })
    })
    .await
    .map_err(|e| format!("Request preparation task failed: {e}"))?
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
    system: Option<&str>,
) -> Result<TurnOutput, String> {
    use futures_util::StreamExt;

    if api_key.is_empty() {
        return Err(
            "Anthropic API key is not configured. Open Settings and paste your API key."
                .to_string(),
        );
    }

    // Bound the assembled prompt below the model's context window before billing
    // this turn (no-op until history is large; escalates from stubbing to dropping
    // oldest turns — see [`evict_to_fit`]). This and the request serialization run
    // off the UI thread (see [`prepare_request_body`]).
    //
    // Retry on context overflow: the token estimate is char-based, so if the real
    // count still trips the hard `400 prompt is too long` limit we halve the
    // target, force another (harder) eviction, and retry — rather than failing the
    // whole run.
    let mut target = prompt_token_target(model, max_tokens);
    const MIN_TARGET: usize = 16_000;
    let client = reqwest::Client::new();

    let (response, sent_estimate) = loop {
        let prepared = prepare_request_body(
            std::mem::take(history),
            tools.to_vec(),
            system.map(str::to_string),
            model.to_string(),
            max_tokens,
            target,
        )
        .await?;
        *history = prepared.history;

        let response = client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .body(prepared.body)
            .send()
            .await
            .map_err(|e| format!("Anthropic API error: {}", describe_error(&e)))?;

        let status = response.status();
        if status.is_success() {
            break (response, prepared.sent_estimate);
        }

        let body = response.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(String::from))
            .unwrap_or(body);

        if status.as_u16() == 400 && msg.contains("prompt is too long") && target > MIN_TARGET {
            // Record the real window so future turns (and this retry) size to it,
            // then re-derive the target and shrink further before retrying.
            learn_context_window_from_error(&msg);
            let learned_target = prompt_token_target(model, max_tokens);
            target = learned_target.min(target / 2).max(MIN_TARGET);
            continue;
        }
        return Err(format!("Anthropic API error ({status}): {msg}"));
    };

    let mut stream = response.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut response_text = String::new();
    let mut tool_blocks: std::collections::BTreeMap<u64, (String, String, String)> =
        std::collections::BTreeMap::new();
    let mut stop_reason: Option<String> = None;
    // Real prompt-token count reported by the API (input + both cache buckets),
    // used to calibrate the char-based estimate. 0 until `message_start` arrives.
    let mut prompt_tokens: usize = 0;

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
                Some("message_start") => {
                    // Total prompt size counts uncached input plus both cache
                    // buckets — caching lowers cost, not the context-window count.
                    let u = &event["message"]["usage"];
                    prompt_tokens = (u["input_tokens"].as_u64().unwrap_or(0)
                        + u["cache_creation_input_tokens"].as_u64().unwrap_or(0)
                        + u["cache_read_input_tokens"].as_u64().unwrap_or(0))
                        as usize;
                }
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

    // Calibrate the token estimate against the API's reported usage. `sent_estimate`
    // was computed off-thread for exactly what was sent, so this adds no
    // main-thread work. Next turn's [`evict_to_fit`] scales its estimate by this.
    record_token_calibration(prompt_tokens, sent_estimate);

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
        prompt_tokens,
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

    /// The stubbing pass with the shipped defaults — exercises the exact tuning
    /// the live config resets to. (Production drives [`evict_stale_history_with`]
    /// via [`evict_to_fit`].)
    fn evict_stale_history(history: &mut [serde_json::Value]) {
        evict_stale_history_with(
            history,
            DEFAULT_KEEP_RECENT_MESSAGES,
            DEFAULT_ELIDE_TEXT_OVER_CHARS,
            DEFAULT_ELIDE_INPUT_OVER_CHARS,
            DEFAULT_EVICT_TRIGGER_BYTES,
        );
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
    fn images_survive_below_threshold() {
        // Images are evicted by the same size-gated pass as text: below the
        // trigger they are left intact (regression guard against an unconditional
        // image pass that stubbed the just-fetched render mid-comparison).
        let original = vec![
            user_text("SYSTEM"),
            assistant_tool_use("tu1", "get_plain_state_image", json!({})),
            result_image("tu1", "AAAA"),
            assistant_tool_use("tu2", "get_plain_state_image", json!({})),
            result_image("tu2", "BBBB"),
            assistant_tool_use("tu3", "get_plain_state_image", json!({})),
            result_image("tu3", "CCCC"),
        ];
        let mut h = original.clone();
        evict_stale_history(&mut h);
        assert_eq!(h, original);
    }

    #[test]
    fn verbose_text_results_survive_below_threshold() {
        // Two get_xfa reads below the size gate must BOTH stay intact — the agent
        // legitimately holds several verbose results at once (regression guard:
        // an over-aggressive verbose-eviction once stubbed all but the latest,
        // forcing an endless re-fetch loop when comparing languages).
        let original = vec![
            user_text("SYSTEM"),
            assistant_tool_use("tu1", "get_xfa", json!({})),
            result_text("tu1", &"x".repeat(400)),
            assistant_tool_use("tu2", "get_xfa", json!({})),
            result_text("tu2", &"y".repeat(400)),
        ];
        let mut h = original.clone();
        evict_stale_history(&mut h);
        assert_eq!(h, original);
    }

    #[test]
    fn calibration_ema_moves_toward_observed_and_clamps() {
        // Save + restore the shared factor so this test can't perturb others.
        let saved = CFG_TOKEN_CALIBRATION_MILLI.load(Ordering::Relaxed);
        CFG_TOKEN_CALIBRATION_MILLI.store(1000, Ordering::Relaxed);

        // Real is 2× the estimate → factor moves from 1.0 toward 2.0 (EMA, so
        // it lands between), and stays within the clamp band.
        record_token_calibration(2000, 1000);
        let k = token_calibration();
        assert!(k > 1.0 && k < 2.0, "EMA should land between 1.0 and 2.0, got {k}");
        // Zero inputs are ignored (no divide-by-zero, no change).
        let before = CFG_TOKEN_CALIBRATION_MILLI.load(Ordering::Relaxed);
        record_token_calibration(0, 1000);
        record_token_calibration(1000, 0);
        assert_eq!(CFG_TOKEN_CALIBRATION_MILLI.load(Ordering::Relaxed), before);

        CFG_TOKEN_CALIBRATION_MILLI.store(saved, Ordering::Relaxed);
    }

    #[test]
    fn estimate_counts_image_by_vision_cost_not_base64_length() {
        // Undecodable base64 must not be counted at ~100k tokens (byte/4); it
        // falls back to the flat per-image figure so it can't drag calibration
        // down and cause later text turns to under-evict.
        let huge = "A".repeat(400_000);
        let est = estimate_tokens(&result_image("i1", &huge));
        assert!(
            est < IMAGE_TOKEN_FALLBACK + 1_000,
            "image over-counted ({est}) — should be ~{IMAGE_TOKEN_FALLBACK}"
        );
    }

    #[test]
    fn image_tokens_computed_from_real_dimensions() {
        use base64::Engine;
        // A real 300×300 PNG → 90_000 px / 750 = 120 vision tokens, regardless of
        // its (tiny, well-compressed) base64 length.
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(300, 300));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let b64 = base64::prelude::BASE64_STANDARD.encode(buf.get_ref());

        let est = estimate_tokens(&result_image("i1", &b64));
        assert!(
            (120..400).contains(&est),
            "expected ~120 computed image tokens + small wrapper, got {est}"
        );
    }

    #[test]
    fn evict_to_fit_stubs_single_oversized_recent_result() {
        // One turn-pair whose result alone exceeds the target. Stage 3 can't drop
        // it (the last pair is kept for pairing), so stage 4 must stub it in place.
        let big = "x".repeat(400_000);
        let mut h = vec![
            user_text("KICK"),
            assistant_tool_use("t1", "get_xfa", json!({})),
            result_text("t1", &big),
        ];
        evict_to_fit(&mut h, &[], None, 5_000);

        assert_eq!(h.len(), 3, "pairing preserved — nothing safe to drop");
        assert!(
            block_text(&h[2]).starts_with(ELIDED_MARKER),
            "oversized recent result should be stubbed by stage 4"
        );
    }

    #[test]
    fn context_window_heuristic_and_learning() {
        // Isolate the shared learned-window state.
        let saved = CFG_CONTEXT_WINDOW.load(Ordering::Relaxed);
        CFG_CONTEXT_WINDOW.store(0, Ordering::Relaxed);

        // Optimistic heuristic: modern large-context families → 1M (even without a
        // literal `[1m]` in the id, which real API model strings lack); Haiku/older
        // → 200K.
        assert_eq!(context_window_for("claude-opus-4-8[1m]"), 1_000_000);
        assert_eq!(context_window_for("claude-opus-4-8"), 1_000_000);
        assert_eq!(context_window_for("claude-sonnet-5"), 1_000_000);
        assert_eq!(context_window_for("claude-haiku-4-5-20251001"), 200_000);

        // A 400 teaches the real maximum; it's authoritative and only ratchets down.
        learn_context_window_from_error("prompt is too long: 1316205 tokens > 250000 maximum");
        assert_eq!(context_window_for("claude-opus-4-8"), 250_000);
        learn_context_window_from_error("prompt is too long: 900000 tokens > 500000 maximum");
        assert_eq!(context_window_for("claude-opus-4-8"), 250_000);

        CFG_CONTEXT_WINDOW.store(saved, Ordering::Relaxed);
    }

    #[test]
    fn evict_to_fit_keeps_everything_under_token_budget() {
        // ~250KB of history — well over the legacy 200KB byte gate — but far under
        // a 1M-window token budget. Nothing may be stubbed or dropped. Regression
        // guard against premature eviction that made the agent re-fetch its own
        // context and loop on re-inspection.
        let big = "x".repeat(250_000); // ~62K estimated tokens
        let original = vec![
            user_text("KICK"),
            assistant_tool_use("t1", "get_xfa", json!({})),
            result_text("t1", &big),
            assistant_tool_use("t2", "list_states", json!({})),
            result_text("t2", "small recent result"),
        ];
        let mut h = original.clone();
        evict_to_fit(&mut h, &[], None, 800_000);
        assert_eq!(h, original, "must not evict while under the token budget");
    }

    #[test]
    fn evict_to_fit_drops_oldest_pairs_under_tiny_target() {
        // Several big turn-pairs. A tiny target forces escalation all the way to
        // dropping the oldest (assistant, user) pairs, keeping the kickoff message
        // and the most recent pair, with tool_use↔tool_result pairing intact.
        let big = "x".repeat(50_000);
        let kick = user_text("KICK");
        let mut h = vec![
            kick.clone(),
            assistant_tool_use("t1", "get_xfa", json!({})),
            result_text("t1", &big),
            assistant_tool_use("t2", "get_xfa", json!({})),
            result_text("t2", &big),
            assistant_tool_use("t3", "get_xfa", json!({})),
            result_text("t3", &big),
            assistant_tool_use("t4", "get_xfa", json!({})),
            result_text("t4", &big),
        ];
        evict_to_fit(&mut h, &[], None, 5_000);

        // Kickoff preserved; dropped down toward the floor (kickoff + last pair).
        assert_eq!(h[0], kick);
        assert!(h.len() <= 3, "expected drop to floor, got {} msgs", h.len());
        // Head after the kickoff is an assistant turn — no orphaned tool_result.
        assert_eq!(h[1]["role"], "assistant");
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
