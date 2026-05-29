//! Platform-aware helpers: async sleep, file download, HTML preview, file explorer.

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

// ── Smart edit (OpenAI API) ───────────────────────────────────────────

/// Call the OpenAI chat completions API with a prompt and optional image attachments.
///
/// `prompt` is the user instruction (already includes schema description and output format).
/// `json_context` is the serialised structured nodes; pass an empty string for follow-up calls.
/// `images` is a list of `(label, base64_png)` pairs from the plain render stage.
/// `api_key` is the OpenAI API key.
///
/// Returns `Ok(response_text)` on success.
#[cfg(not(target_arch = "wasm32"))]
pub async fn run_copilot_smart_edit(
    prompt: &str,
    json_context: &str,
    images: &[(String, String)],
    api_key: &str,
) -> Result<String, String> {
    if api_key.is_empty() {
        return Err(
            "OpenAI API key is not configured. Open Settings and paste your API key.".to_string(),
        );
    }

    let text = if json_context.is_empty() {
        prompt.to_string()
    } else {
        format!(
            "{prompt}\n\n\
             The structured JSON representation of the selected form nodes is included below. \
             The attached PNG images show the rendered form pages for visual reference.\n\n\
             BEGIN STRUCTURED NODES JSON\n\
             {json_context}\n\
             END STRUCTURED NODES JSON\n\n\
             Return ONLY a valid JSON object with exactly two keys: \
             \"nodes\" (the replacement Vec<StructuredNode> array) and \
             \"changes\" (an array of objects, each with \"id\" (integer) and \"description\" (string), \
             describing each logical change you made). \
             No surrounding prose, no markdown fences, no trailing notes."
        )
    };

    let mut content: Vec<serde_json::Value> =
        vec![serde_json::json!({"type": "text", "text": text})];

    for (_label, b64) in images {
        content.push(serde_json::json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:image/png;base64,{b64}"),
                "detail": "high"
            }
        }));
    }

    let request_body = serde_json::json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": content}]
    });

    let client = reqwest::Client::new();
    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&request_body)
        .send()
        .await
        .map_err(|e| format!("Failed to call OpenAI API: {e}"))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("Failed to read OpenAI response: {e}"))?;

    if !status.is_success() {
        return Err(format!("OpenAI API returned HTTP {status}: {body}"));
    }

    let parsed: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("Failed to parse OpenAI response: {e}"))?;

    let content_text = parsed["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| format!("Unexpected OpenAI response structure: {body}"))?;

    Ok(content_text.to_string())
}

#[cfg(target_arch = "wasm32")]
pub async fn run_copilot_smart_edit(
    _prompt: &str,
    _json_context: &str,
    _images: &[(String, String)],
    _api_key: &str,
) -> Result<String, String> {
    Err("Smart Edit is only supported in the desktop app. The web version cannot call the OpenAI API directly.".to_string())
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
