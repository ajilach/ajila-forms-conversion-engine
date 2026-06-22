//! Local model inference via mistral.rs (Metal GPU on macOS).
//!
//! Supports downloading models from HuggingFace and running them locally.
//! The loaded model is cached in a global `Mutex` and reused across turns.

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use futures_util::StreamExt;
use tokio::sync::Mutex as TokioMutex;

use crate::platform::{ToolCall, TurnOutput};

// ── Catalog ──────────────────────────────────────────────────────────────────

pub struct LocalModelSpec {
    pub name: &'static str,
    pub hf_repo: &'static str,
}

// Non-FP8 (bf16) variants: the FP8 vision kernels crash on Metal (GPU address
// fault on the first image request), so we use full-precision weights. The 32B
// bf16 (~64 GB) is omitted as it exceeds typical unified-memory budgets.
pub const AVAILABLE_MODELS: &[LocalModelSpec] = &[LocalModelSpec {
    name: "Qwen3-VL-8B-Instruct",
    hf_repo: "Qwen/Qwen3-VL-8B-Instruct",
}];

// ── Storage helpers ───────────────────────────────────────────────────────────

pub fn models_dir() -> PathBuf {
    let base =
        dirs::config_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"));
    base.join("blueprint").join("models")
}

pub fn model_dir(name: &str) -> PathBuf {
    models_dir().join(name)
}

/// Sentinel file written only after every model file has been fully downloaded.
/// Its presence is what distinguishes a complete model from a partial download.
const COMPLETE_MARKER: &str = ".download_complete";

pub fn is_downloaded(name: &str) -> bool {
    model_dir(name).join(COMPLETE_MARKER).exists()
}

pub fn downloaded_models() -> Vec<String> {
    AVAILABLE_MODELS
        .iter()
        .filter(|m| is_downloaded(m.name))
        .map(|m| m.name.to_string())
        .collect()
}

/// Delete a downloaded model's files from disk. Also evicts it from the in-memory
/// cache if it happens to be the currently loaded model.
pub async fn delete_model(name: &str) -> Result<(), String> {
    {
        let mut guard = MODEL.lock().await;
        if guard.as_ref().map(|(n, _)| n == name).unwrap_or(false) {
            *guard = None;
        }
    }

    let dir = model_dir(name);
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ── Download ──────────────────────────────────────────────────────────────────

/// Download all model files for `name` from HuggingFace, reporting progress in
/// [0, 1] via `on_progress`. Already-downloaded files are skipped.
pub async fn download_model(
    name: &str,
    mut on_progress: impl FnMut(f32) + 'static,
) -> Result<(), String> {
    let spec = AVAILABLE_MODELS
        .iter()
        .find(|m| m.name == name)
        .ok_or_else(|| format!("Unknown model: {name}"))?;

    let dir = model_dir(name);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3600))
        .build()
        .map_err(|e| e.to_string())?;

    // `?blobs=true` is required for the API to include per-file `size`; without it
    // the siblings carry only `rfilename` and we'd have no basis for a progress bar.
    let api_url = format!("https://huggingface.co/api/models/{}?blobs=true", spec.hf_repo);
    let meta: serde_json::Value = client
        .get(&api_url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let siblings = meta["siblings"]
        .as_array()
        .ok_or("No siblings in model metadata")?;

    let files: Vec<(String, u64)> = siblings
        .iter()
        .filter_map(|s| {
            let rfilename = s["rfilename"].as_str()?.to_string();
            let size = s["size"].as_u64().unwrap_or(0);
            Some((rfilename, size))
        })
        .collect();

    let total_bytes: u64 = files.iter().map(|(_, s)| s).sum();
    let total_files = files.len().max(1);
    let mut downloaded_bytes: u64 = 0;
    let mut completed_files: usize = 0;

    // Report something immediately so the UI shows a bar / disables the button
    // before the first byte arrives. Falls back to file-count progress if the
    // API didn't supply sizes.
    let report = |downloaded_bytes: u64, completed_files: usize, cb: &mut dyn FnMut(f32)| {
        let frac = if total_bytes > 0 {
            downloaded_bytes as f32 / total_bytes as f32
        } else {
            completed_files as f32 / total_files as f32
        };
        cb(frac.clamp(0.0, 1.0));
    };
    report(0, 0, &mut on_progress);

    for (filename, file_size) in &files {
        let dest = dir.join(filename);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        // A file is only treated as already-downloaded if its size matches the
        // expected size — otherwise a partial leftover would be skipped as "done".
        if dest.exists() {
            let on_disk = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
            if *file_size == 0 || on_disk == *file_size {
                downloaded_bytes += file_size;
                completed_files += 1;
                report(downloaded_bytes, completed_files, &mut on_progress);
                continue;
            }
            let _ = std::fs::remove_file(&dest);
        }

        let url = format!(
            "https://huggingface.co/{}/resolve/main/{}",
            spec.hf_repo, filename
        );
        let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("Failed to download {filename}: HTTP {}", resp.status()));
        }

        // Download to a temp path and rename on success, so an interrupted download
        // never leaves a partial file at the final path.
        let tmp = PathBuf::from(format!("{}.part", dest.display()));
        let mut file = tokio::fs::File::create(&tmp)
            .await
            .map_err(|e| e.to_string())?;
        let mut stream = resp.bytes_stream();

        use tokio::io::AsyncWriteExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| e.to_string())?;
            file.write_all(&chunk).await.map_err(|e| e.to_string())?;
            downloaded_bytes += chunk.len() as u64;
            report(downloaded_bytes, completed_files, &mut on_progress);
        }
        file.flush().await.map_err(|e| e.to_string())?;
        drop(file);
        tokio::fs::rename(&tmp, &dest)
            .await
            .map_err(|e| e.to_string())?;
        completed_files += 1;
        report(downloaded_bytes, completed_files, &mut on_progress);
    }

    // Mark the download complete so `is_downloaded` reports it.
    std::fs::write(dir.join(COMPLETE_MARKER), name).map_err(|e| e.to_string())?;
    on_progress(1.0);
    Ok(())
}

// ── Model cache ───────────────────────────────────────────────────────────────

static MODEL: TokioMutex<Option<(String, Arc<mistralrs::Model>)>> = TokioMutex::const_new(None);

async fn get_or_load_model(name: &str) -> Result<Arc<mistralrs::Model>, String> {
    let mut guard = MODEL.lock().await;
    if let Some((loaded_name, handle)) = guard.as_ref() {
        if loaded_name == name {
            return Ok(Arc::clone(handle));
        }
    }

    // Require a fully-downloaded local copy. Falling back to the HuggingFace repo
    // here would silently trigger a multi-GB network download that looks identical
    // to a hang, so we fail with a clear, actionable message instead.
    if !is_downloaded(name) {
        return Err(format!(
            "Model \"{name}\" is not downloaded. Open Settings → Local Model and download it first."
        ));
    }
    let model_id = model_dir(name).to_string_lossy().to_string();

    // Log the device mistral.rs will resolve to, so it's unambiguous from the
    // console whether inference runs on the Metal GPU or fell back to CPU.
    match mistralrs::best_device(false) {
        Ok(dev) => eprintln!("local_inference: loading \"{name}\" on device {dev:?}"),
        Err(e) => eprintln!("local_inference: device probe failed: {e}"),
    }

    // `max_num_seqs` defaults to 32, which makes mistral.rs reserve KV-cache
    // capacity for 32 concurrent sequences. For a large model on a memory-tight
    // Metal device that reservation alone can blow past the GPU working-set limit
    // and trigger a command-buffer failure mid-inference. We only ever run one
    // sequence at a time, so cap it at 1. `with_logging` surfaces the real
    // upstream error if a GPU command buffer still fails.
    let handle = mistralrs::MultimodalModelBuilder::new(&model_id)
        .with_max_num_seqs(1)
        .with_logging()
        .build()
        .await
        .map_err(|e| e.to_string())?;

    let handle = Arc::new(handle);
    *guard = Some((name.to_string(), Arc::clone(&handle)));
    Ok(handle)
}

// ── Format conversion (Anthropic → mistral.rs RequestBuilder) ─────────────────

/// Longest edge (px) any image is scaled down to before being handed to the
/// vision model. Qwen3-VL uses dynamic-resolution vision, so a full-resolution
/// page render expands into a huge number of vision tokens and the forward-pass
/// activations can exceed the Metal GPU budget (→ command-buffer failure). This
/// matches the model's planned `max_image_shape` and bounds that spike.
const MAX_IMAGE_EDGE: u32 = 1024;

/// Scale `img` down so its longest edge is at most [`MAX_IMAGE_EDGE`], preserving
/// aspect ratio. Images already within bounds are returned unchanged.
fn downscale_image(img: image::DynamicImage) -> image::DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w.max(h) <= MAX_IMAGE_EDGE {
        return img;
    }
    img.resize(
        MAX_IMAGE_EDGE,
        MAX_IMAGE_EDGE,
        image::imageops::FilterType::Lanczos3,
    )
}

fn anthropic_tool_to_mistralrs(tool: &serde_json::Value) -> mistralrs::Tool {
    mistralrs::Tool {
        tp: mistralrs::ToolType::Function,
        function: mistralrs::Function {
            name: tool["name"].as_str().unwrap_or_default().to_string(),
            description: tool["description"].as_str().map(|s| s.to_string()),
            parameters: tool["input_schema"]
                .as_object()
                .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        },
    }
}

/// Build a `RequestBuilder` from an Anthropic-format history and tool list.
fn history_to_request(
    history: &[serde_json::Value],
    tools: &[serde_json::Value],
    max_tokens: u32,
) -> Result<mistralrs::RequestBuilder, String> {
    let mut req = mistralrs::RequestBuilder::new().set_sampler_max_len(max_tokens as usize);

    if !tools.is_empty() {
        req = req.set_tools(tools.iter().map(anthropic_tool_to_mistralrs).collect());
    }

    for msg in history {
        let role = msg["role"].as_str().unwrap_or("user");
        let content = &msg["content"];

        match role {
            "assistant" => {
                let (text, tool_calls) = if let Some(blocks) = content.as_array() {
                    let text = blocks
                        .iter()
                        .filter_map(|b| {
                            if b["type"] == "text" {
                                b["text"].as_str().map(str::to_string)
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("");

                    let tool_calls: Vec<mistralrs::ToolCallResponse> = blocks
                        .iter()
                        .filter(|b| b["type"] == "tool_use")
                        .enumerate()
                        .map(|(i, b)| mistralrs::ToolCallResponse {
                            index: i,
                            id: b["id"].as_str().unwrap_or("").to_string(),
                            tp: mistralrs::ToolCallType::Function,
                            function: mistralrs::CalledFunction {
                                name: b["name"].as_str().unwrap_or("").to_string(),
                                arguments: serde_json::to_string(&b["input"]).unwrap_or_default(),
                            },
                        })
                        .collect();
                    (text, tool_calls)
                } else if let Some(text) = content.as_str() {
                    (text.to_string(), vec![])
                } else {
                    (String::new(), vec![])
                };

                if tool_calls.is_empty() {
                    req = req.add_message(mistralrs::TextMessageRole::Assistant, &text);
                } else {
                    req = req.add_message_with_tool_call(
                        mistralrs::TextMessageRole::Assistant,
                        &text,
                        tool_calls,
                    );
                }
            }
            "user" => {
                if let Some(blocks) = content.as_array() {
                    // Tool results must become separate "tool" role messages.
                    for block in blocks {
                        if block["type"] == "tool_result" {
                            let tool_call_id = block["tool_use_id"].as_str().unwrap_or("");
                            let result_text = block["content"]
                                .as_array()
                                .and_then(|arr| {
                                    arr.iter()
                                        .find(|b| b["type"] == "text")
                                        .and_then(|b| b["text"].as_str())
                                })
                                .or_else(|| block["content"].as_str())
                                .unwrap_or("");
                            req = req.add_tool_message(result_text, tool_call_id);
                        }
                    }

                    // Collect remaining text and images into one user message.
                    let mut text_parts: Vec<String> = Vec::new();
                    let mut images: Vec<image::DynamicImage> = Vec::new();

                    for block in blocks {
                        match block["type"].as_str() {
                            Some("text") => {
                                if let Some(t) = block["text"].as_str() {
                                    text_parts.push(t.to_string());
                                }
                            }
                            Some("image") => {
                                if let Some(b64) = block["source"]["data"].as_str() {
                                    let bytes = base64::engine::general_purpose::STANDARD
                                        .decode(b64)
                                        .map_err(|e| e.to_string())?;
                                    let img = image::load_from_memory(&bytes)
                                        .map_err(|e| e.to_string())?;
                                    images.push(downscale_image(img));
                                }
                            }
                            _ => {}
                        }
                    }

                    let combined_text = text_parts.join("\n");
                    if !combined_text.is_empty() || !images.is_empty() {
                        if images.is_empty() {
                            req = req.add_message(mistralrs::TextMessageRole::User, &combined_text);
                        } else {
                            req = req.add_image_message(
                                mistralrs::TextMessageRole::User,
                                &combined_text,
                                images,
                            );
                        }
                    }
                } else if let Some(text) = content.as_str() {
                    req = req.add_message(mistralrs::TextMessageRole::User, text);
                }
            }
            _ => {}
        }
    }

    Ok(req)
}

// ── Inference entry-points ────────────────────────────────────────────────────

/// Send one streaming turn to the local model with optional tool definitions.
/// Mirrors [`crate::platform::anthropic_stream_turn`].
///
/// mistral.rs runs its inference engine on its own dedicated OS thread (see
/// `MistralRs`'s `engine_handler`), so `send_chat_request` here is just a channel
/// send + await — it does no heavy CPU work on this thread and does not block the
/// UI during generation. Only the one-time model load (inside `get_or_load_model`)
/// is heavy, and it briefly pauses the UI on first use.
pub async fn local_stream_turn(
    history: &mut Vec<serde_json::Value>,
    tools: &[serde_json::Value],
    model_name: &str,
    max_tokens: u32,
) -> Result<TurnOutput, String> {
    let model = get_or_load_model(model_name).await?;
    let request = history_to_request(history, tools, max_tokens)?;

    let chat_resp = model
        .send_chat_request(request)
        .await
        .map_err(|e| e.to_string())?;

    let mut text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut stop_reason: Option<String> = None;

    if let Some(choice) = chat_resp.choices.into_iter().next() {
        text = choice.message.content.unwrap_or_default();
        stop_reason = Some(choice.finish_reason);
        if let Some(calls) = choice.message.tool_calls {
            for c in calls {
                let input =
                    serde_json::from_str(&c.function.arguments).unwrap_or(serde_json::Value::Null);
                tool_calls.push(ToolCall {
                    id: c.id,
                    name: c.function.name,
                    input,
                });
            }
        }
    }

    // Normalise stop reason to Anthropic convention used by drive_agent.
    if !tool_calls.is_empty() && stop_reason.as_deref() != Some("tool_use") {
        stop_reason = Some("tool_use".to_string());
    }

    // Append assistant turn to history in Anthropic format.
    let assistant_content: serde_json::Value = if tool_calls.is_empty() {
        serde_json::Value::String(text.clone())
    } else {
        let mut blocks: Vec<serde_json::Value> = Vec::new();
        if !text.is_empty() {
            blocks.push(serde_json::json!({"type": "text", "text": text}));
        }
        for tc in &tool_calls {
            blocks.push(serde_json::json!({
                "type": "tool_use",
                "id": tc.id,
                "name": tc.name,
                "input": tc.input
            }));
        }
        serde_json::Value::Array(blocks)
    };
    history.push(serde_json::json!({"role": "assistant", "content": assistant_content}));

    Ok(TurnOutput {
        text,
        tool_calls,
        stop_reason,
    })
}

/// Send one plain chat turn to the local model (no tools, text only).
/// Mirrors [`crate::platform::anthropic_chat_turn`].
pub async fn local_chat_turn(
    history: &mut Vec<serde_json::Value>,
    user_text: &str,
    images: &[(String, String)],
    model_name: &str,
    max_tokens: u32,
) -> Result<String, String> {
    // Build user message in Anthropic format, then run a stream turn with no tools.
    let mut content: Vec<serde_json::Value> =
        vec![serde_json::json!({"type": "text", "text": user_text})];
    for (_label, b64) in images {
        let media_type = if b64.starts_with("/9j/") {
            "image/jpeg"
        } else {
            "image/png"
        };
        content.push(serde_json::json!({
            "type": "image",
            "source": {"type": "base64", "media_type": media_type, "data": b64}
        }));
    }
    history.push(serde_json::json!({"role": "user", "content": content}));

    let turn = local_stream_turn(history, &[], model_name, max_tokens).await?;
    Ok(turn.text)
}
