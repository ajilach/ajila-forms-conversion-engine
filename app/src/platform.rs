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

// ── Smart edit (gh copilot) ───────────────────────────────────────────

/// Run `gh copilot` with a prompt and optional image attachments.
///
/// `prompt` is the user instruction.
/// `json_context` is the serialised structured nodes to include in the prompt.
/// `images` is a list of `(label, base64_png)` pairs from the plain render stage.
///
/// Returns `Ok(response_text)` on success.
#[cfg(not(target_arch = "wasm32"))]
pub async fn run_copilot_smart_edit(
    prompt: &str,
    json_context: &str,
    images: &[(String, String)],
    session_name: Option<&str>,
    resume_session: bool,
) -> Result<String, String> {
    use base64::Engine;
    use std::io::Write;

    let tmp_dir = std::env::temp_dir().join("blueprint-smart-edit");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("Failed to create temp dir: {e}"))?;

    // Write images to temp PNG files
    let mut image_paths: Vec<std::path::PathBuf> = Vec::new();
    for (label, b64) in images {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("Failed to decode image {label}: {e}"))?;
        let path = tmp_dir.join(format!("{label}.png"));
        let mut f = std::fs::File::create(&path)
            .map_err(|e| format!("Failed to create temp image: {e}"))?;
        f.write_all(&bytes)
            .map_err(|e| format!("Failed to write temp image: {e}"))?;
        image_paths.push(path);
    }

    // Build the full prompt
    let full_prompt = format!(
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
    );

    // Resolve gh explicitly because packaged desktop apps often run with a
    // minimal PATH that does not include Homebrew locations.
    let gh_executable = resolve_gh_executable().ok_or_else(|| {
        format!(
            "Failed to locate GitHub CLI executable 'gh'. Install GitHub CLI and ensure it is available in PATH, or set GITHUB_CLI_PATH. Current PATH: {}",
            std::env::var("PATH").unwrap_or_else(|_| "<unset>".to_string())
        )
    })?;

    // Build command
    let mut cmd = tokio::process::Command::new(&gh_executable);
    cmd.arg("copilot").arg("--");

    if let Some(name) = session_name {
        if resume_session {
            cmd.arg(format!("--resume={name}"));
        } else {
            cmd.arg("--name").arg(name);
        }
    }

    cmd.arg("-p")
        .arg(&full_prompt)
        .arg("--output-format")
        .arg("text")
        .arg("--allow-all-tools");

    for img_path in &image_paths {
        cmd.arg("--attachment").arg(img_path);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("Failed to run gh copilot: {e}"))?;

    // Clean up temp files (best effort)
    if let Err(e) = std::fs::remove_dir_all(&tmp_dir) {
        eprintln!("Warning: failed to clean up smart-edit temp dir: {e}");
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh copilot failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let trimmed = stdout.trim();
    let lower = trimmed.to_ascii_lowercase();

    if trimmed.is_empty() {
        return Err(format!(
            "gh copilot returned no content. stderr: {}",
            stderr.trim()
        ));
    }

    let looks_like_cli_help = (lower.contains("usage:") && lower.contains("gh copilot"))
        || lower.contains("github cli")
        || lower.contains("unknown command")
        || lower.contains("authentication required");

    if looks_like_cli_help {
        return Err(format!(
            "gh copilot returned CLI/help output instead of model content. stdout: {}",
            trimmed
        ));
    }

    Ok(stdout)
}

#[cfg(not(target_arch = "wasm32"))]
fn resolve_gh_executable() -> Option<std::path::PathBuf> {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    fn executable_name() -> &'static str {
        #[cfg(target_os = "windows")]
        {
            "gh.exe"
        }
        #[cfg(not(target_os = "windows"))]
        {
            "gh"
        }
    }

    fn has_file(path: &Path) -> bool {
        path.is_file()
    }

    fn find_in_path(path_var: Option<&OsStr>, executable: &str) -> Option<PathBuf> {
        let path_var = path_var?;
        for dir in std::env::split_paths(path_var) {
            let candidate = dir.join(executable);
            if has_file(&candidate) {
                return Some(candidate);
            }
        }
        None
    }

    let executable = executable_name();

    if let Some(path_override) = std::env::var_os("GITHUB_CLI_PATH") {
        let override_path = PathBuf::from(path_override);
        if has_file(&override_path) {
            return Some(override_path);
        }
    }

    if let Some(path_hit) = find_in_path(std::env::var_os("PATH").as_deref(), executable) {
        return Some(path_hit);
    }

    #[cfg(target_os = "macos")]
    {
        for candidate in [
            "/opt/homebrew/bin/gh",
            "/usr/local/bin/gh",
            "/opt/local/bin/gh",
            "/usr/bin/gh",
        ] {
            let candidate = PathBuf::from(candidate);
            if has_file(&candidate) {
                return Some(candidate);
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        for candidate in ["/usr/local/bin/gh", "/usr/bin/gh", "/snap/bin/gh"] {
            let candidate = PathBuf::from(candidate);
            if has_file(&candidate) {
                return Some(candidate);
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let candidate = PathBuf::from(local_app_data)
                .join("GitHubCLI")
                .join("bin")
                .join("gh.exe");
            if has_file(&candidate) {
                return Some(candidate);
            }
        }
    }

    None
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::resolve_gh_executable;

    #[test]
    fn resolves_gh_from_path() {
        use std::fs;

        let tmp_root =
            std::env::temp_dir().join(format!("blueprint-gh-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_root);
        fs::create_dir_all(&tmp_root).expect("create tmp root");

        #[cfg(target_os = "windows")]
        let gh_name = "gh.exe";
        #[cfg(not(target_os = "windows"))]
        let gh_name = "gh";

        let gh_path = tmp_root.join(gh_name);
        fs::write(&gh_path, b"#!/bin/sh\n").expect("write fake gh");

        let old_path = std::env::var_os("PATH");
        let old_override = std::env::var_os("GITHUB_CLI_PATH");

        unsafe {
            std::env::set_var("GITHUB_CLI_PATH", "");
            std::env::set_var("PATH", &tmp_root);
        }

        let resolved = resolve_gh_executable();

        match old_path {
            Some(v) => unsafe { std::env::set_var("PATH", v) },
            None => unsafe { std::env::remove_var("PATH") },
        }
        match old_override {
            Some(v) => unsafe { std::env::set_var("GITHUB_CLI_PATH", v) },
            None => unsafe { std::env::remove_var("GITHUB_CLI_PATH") },
        }

        assert_eq!(resolved, Some(gh_path));

        let _ = fs::remove_dir_all(&tmp_root);
    }
}

#[cfg(target_arch = "wasm32")]
pub async fn run_copilot_smart_edit(
    _prompt: &str,
    _json_context: &str,
    _images: &[(String, String)],
    _session_name: Option<&str>,
    _resume_session: bool,
) -> Result<String, String> {
    Err("Smart Edit is only supported in the desktop app. The web version cannot invoke local CLI tools.".to_string())
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
