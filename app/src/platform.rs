//! Platform-aware helpers: async sleep, file download, HTML preview, file explorer.

use crate::settings::LlmProvider;

/// Platform-agnostic async sleep.
#[allow(dead_code)]
pub async fn async_sleep_ms(ms: u32) {
    #[cfg(target_arch = "wasm32")]
    gloo_timers::future::TimeoutFuture::new(ms).await;
    #[cfg(not(target_arch = "wasm32"))]
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

// ── File download / preview helpers ──────────────────────────────────

#[cfg(target_arch = "wasm32")]
pub fn download_file(data: &[u8], filename: &str, mime_type: &str) {
    use js_sys::{Array, Uint8Array};
    use wasm_bindgen::JsCast;
    use web_sys::{Blob, BlobPropertyBag, HtmlAnchorElement, Url};

    let uint8_array = Uint8Array::from(data);
    let array = Array::new();
    array.push(&uint8_array.buffer());

    let mut options = BlobPropertyBag::new();
    options.set_type(mime_type);

    let blob = Blob::new_with_buffer_source_sequence_and_options(&array, &options).unwrap();
    let url = Url::create_object_url_with_blob(&blob).unwrap();

    let window = web_sys::window().unwrap();
    let document = window.document().unwrap();
    let a: HtmlAnchorElement = document.create_element("a").unwrap().dyn_into().unwrap();
    a.set_href(&url);
    a.set_download(filename);
    a.click();

    let _ = Url::revoke_object_url(&url);
}

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(target_arch = "wasm32")]
pub fn show_html_preview(html: String, _filename: &str) {
    use js_sys::{Array, Uint8Array};
    use web_sys::{Blob, BlobPropertyBag, Url};

    let uint8_array = Uint8Array::from(html.as_bytes());
    let array = Array::new();
    array.push(&uint8_array.buffer());

    let mut options = BlobPropertyBag::new();
    options.set_type("text/html");

    let blob = Blob::new_with_buffer_source_sequence_and_options(&array, &options).unwrap();
    let url = Url::create_object_url_with_blob(&blob).unwrap();

    let window = web_sys::window().unwrap();
    let _ = window.open_with_url_and_target(&url, "_blank");
}

#[cfg(not(target_arch = "wasm32"))]
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

/// Send one turn of a multi-turn chat to the configured LLM provider and
/// return the assistant's reply.
///
/// Dispatches to the OpenAI or Anthropic implementation based on `provider`.
/// Because a single smart-edit session always uses one provider, `history`
/// only ever holds messages in that provider's wire format.
/// `images` are `(label, base64_png)` pairs; `pdfs` are `(filename, raw_bytes)`
/// pairs attached as document inputs. `max_tokens` bounds the reply length
/// (only the Anthropic API requires it; the OpenAI path lets the model decide).
pub async fn chat_turn(
    provider: LlmProvider,
    history: &mut Vec<serde_json::Value>,
    user_text: &str,
    images: &[(String, String)],
    pdfs: &[(String, Vec<u8>)],
    api_key: &str,
    model: &str,
    max_tokens: u32,
) -> Result<String, String> {
    match provider {
        LlmProvider::OpenAi => {
            openai_chat_turn(history, user_text, images, pdfs, api_key, model, max_tokens).await
        }
        LlmProvider::Anthropic => {
            anthropic_chat_turn(history, user_text, images, pdfs, api_key, model, max_tokens).await
        }
    }
}

/// List the available model identifiers for the given provider.
pub async fn list_models(provider: LlmProvider, api_key: &str) -> Result<Vec<String>, String> {
    match provider {
        LlmProvider::OpenAi => openai_list_models(api_key).await,
        LlmProvider::Anthropic => anthropic_list_models(api_key).await,
    }
}

/// Send one turn of a multi-turn chat to the OpenAI API and return the assistant's reply.
///
/// `history` is the ongoing conversation; this function appends both the new user message
/// and the assistant reply so subsequent calls automatically continue the thread.
/// `user_text` is the user message text for this turn.
/// `images` is a list of `(label, base64_png)` pairs; pass an empty slice for follow-up turns.
/// `pdfs` is a list of `(filename, raw_bytes)` pairs attached as `file` inputs.
/// `api_key` is the OpenAI API key.
/// `model` is the OpenAI model identifier (e.g. "gpt-4o").
/// `max_tokens` is accepted for signature parity but not sent — the OpenAI path
/// lets the model use its default completion limit.
#[cfg(not(target_arch = "wasm32"))]
pub async fn openai_chat_turn(
    history: &mut Vec<serde_json::Value>,
    user_text: &str,
    images: &[(String, String)],
    pdfs: &[(String, Vec<u8>)],
    api_key: &str,
    model: &str,
    _max_tokens: u32,
) -> Result<String, String> {
    use async_openai::{Client, config::OpenAIConfig};
    use base64::Engine;
    use futures_util::StreamExt;

    if api_key.is_empty() {
        return Err(
            "OpenAI API key is not configured. Open Settings and paste your API key.".to_string(),
        );
    }

    let mut content: Vec<serde_json::Value> =
        vec![serde_json::json!({"type": "text", "text": user_text})];

    for (_label, b64) in images {
        let media_type = image_media_type(b64);
        content.push(serde_json::json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:{media_type};base64,{b64}"),
                "detail": "high"
            }
        }));
    }

    for (filename, bytes) in pdfs {
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        content.push(serde_json::json!({
            "type": "file",
            "file": {
                "filename": filename,
                "file_data": format!("data:application/pdf;base64,{b64}")
            }
        }));
    }

    history.push(serde_json::json!({"role": "user", "content": content}));

    let config = OpenAIConfig::new().with_api_key(api_key);
    let client = Client::with_config(config);

    let request = serde_json::json!({
        "model": model,
        "messages": history,
        "response_format": { "type": "json_object" },
        "stream": true,
    });

    // Stream the reply and accumulate the full text. Streaming avoids the
    // request timeout on long generations (e.g. whole-document AI processing).
    let mut stream = client
        .chat()
        .create_stream_byot::<serde_json::Value, serde_json::Value>(request)
        .await
        .map_err(|e| format!("OpenAI API error: {e}"))?;

    let mut response_text = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("OpenAI API error: {e}"))?;
        if let Some(delta) = chunk["choices"][0]["delta"]["content"].as_str() {
            response_text.push_str(delta);
        }
    }

    history.push(serde_json::json!({"role": "assistant", "content": response_text}));

    Ok(response_text)
}

#[cfg(target_arch = "wasm32")]
pub async fn openai_chat_turn(
    _history: &mut Vec<serde_json::Value>,
    _user_text: &str,
    _images: &[(String, String)],
    _pdfs: &[(String, Vec<u8>)],
    _api_key: &str,
    _model: &str,
    _max_tokens: u32,
) -> Result<String, String> {
    Err("AI features are only supported in the desktop app. The web version cannot call the OpenAI API directly.".to_string())
}

// ── List available OpenAI models ─────────────────────────────────────

/// Fetch the list of available model IDs from the OpenAI API,
/// filtered to chat-capable models and sorted alphabetically.
#[cfg(not(target_arch = "wasm32"))]
pub async fn openai_list_models(api_key: &str) -> Result<Vec<String>, String> {
    use async_openai::{Client, config::OpenAIConfig};

    if api_key.is_empty() {
        return Err("OpenAI API key is not configured.".to_string());
    }

    let config = OpenAIConfig::new().with_api_key(api_key);
    let client = Client::with_config(config);

    let response = client
        .models()
        .list()
        .await
        .map_err(|e| format!("Failed to list models: {e}"))?;

    let mut ids: Vec<String> = response
        .data
        .into_iter()
        .map(|m| m.id)
        .filter(|id| is_chat_model(id))
        .collect();

    ids.sort();
    Ok(ids)
}

/// Returns `true` only for models known to support the chat completions endpoint.
fn is_chat_model(id: &str) -> bool {
    // Allowlist of chat-capable model families that support image_url (vision).
    // Excludes models without vision: gpt-3.5-turbo, gpt-4 (non-turbo),
    // o1-mini, o1-preview, o3-mini.
    const CHAT_MODELS: &[&str] = &[
        "chatgpt-4o-latest",
        "gpt-4-turbo",
        "gpt-4o",
        "gpt-4o-mini",
        "gpt-4.1",
        "gpt-4.1-mini",
        "gpt-4.1-nano",
        "gpt-4.5-preview",
        "o1",
        "o3",
        "o3-pro",
        "o4-mini",
    ];

    CHAT_MODELS.iter().any(|base| {
        // Exact match or dated snapshot (e.g. "gpt-4o-2024-11-20").
        id == *base || id.starts_with(&format!("{base}-2"))
    })
}

#[cfg(target_arch = "wasm32")]
pub async fn openai_list_models(_api_key: &str) -> Result<Vec<String>, String> {
    Err("Listing models is only supported in the desktop app.".to_string())
}

/// Detect the image media type from a base64 payload by its leading bytes.
/// PNG base64 begins with `iVBOR` (`\x89PNG`); JPEG with `/9j/` (`\xff\xd8\xff`).
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
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
/// Mirrors [`openai_chat_turn`] but builds Anthropic-shaped content blocks and
/// appends them to `history`, so the same conversation thread continues across
/// repair and follow-up calls within a smart-edit session.
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(target_arch = "wasm32")]
pub async fn anthropic_chat_turn(
    _history: &mut Vec<serde_json::Value>,
    _user_text: &str,
    _images: &[(String, String)],
    _pdfs: &[(String, Vec<u8>)],
    _api_key: &str,
    _model: &str,
    _max_tokens: u32,
) -> Result<String, String> {
    Err("AI features are only supported in the desktop app. The web version cannot call the Anthropic API directly.".to_string())
}

/// Fetch the list of available model IDs from the Anthropic API, sorted
/// alphabetically. All Claude models support the chat + vision endpoint, so no
/// filtering is applied.
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(target_arch = "wasm32")]
pub async fn anthropic_list_models(_api_key: &str) -> Result<Vec<String>, String> {
    Err("Listing models is only supported in the desktop app.".to_string())
}

// ── File explorer reveal ─────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
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
