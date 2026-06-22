//! Register the bundled `mcp` stdio server with Claude Desktop.
//!
//! The desktop app ships the standalone `mcp` binary as a sidecar next to its
//! own executable (see `[bundle].external_bin` in `Dioxus.toml`). This module
//! writes an entry into Claude Desktop's `claude_desktop_config.json` pointing
//! at that binary, so the form-conversion tools become available in Claude
//! Desktop. Both binaries share the same `<config_dir>/blueprint/history.db`,
//! so a conversion driven over MCP shows up in the app and vice versa.

use std::path::PathBuf;

/// Server key under `mcpServers` in `claude_desktop_config.json`.
const SERVER_KEY: &str = "blueprint";

/// Path to Claude Desktop's config file.
///
/// `dirs::config_dir()` already maps to the right per-OS base — `~/Library/
/// Application Support` (macOS), `%APPDATA%` (Windows), `~/.config` (Linux) —
/// each of which is where Claude Desktop keeps its config, so a single join
/// works on all three.
pub fn claude_config_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("Claude")
            .join("claude_desktop_config.json"),
    )
}

/// Path to the bundled `mcp` binary, resolved relative to the running app
/// executable.
///
/// In dev, `cargo build` puts `blueprint-app` and `mcp` side by side in
/// `target/<profile>/`. In a packaged build, dx's `external_bin` sidecar lands
/// next to the app executable. We return the first candidate that exists:
/// the plain name (`mcp` / `mcp.exe`) and, defensively, the target-triple
/// suffixed name dx derives the sidecar from.
pub fn mcp_binary_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;

    let exe_suffix = std::env::consts::EXE_SUFFIX; // "" on unix, ".exe" on windows
    let candidates = [
        format!("mcp{exe_suffix}"),
        format!("mcp-{}{exe_suffix}", current_target_triple()),
    ];
    candidates
        .iter()
        .map(|name| dir.join(name))
        .find(|p| p.exists())
        // Fall back to the plain sibling path even if it doesn't exist yet, so
        // `install()` can surface a clear "not found at <path>" error.
        .or_else(|| Some(dir.join(format!("mcp{exe_suffix}"))))
}

/// Best-effort host target triple, used only to probe the dx sidecar filename.
fn current_target_triple() -> String {
    format!(
        "{}-{}",
        std::env::consts::ARCH,
        match std::env::consts::OS {
            "macos" => "apple-darwin",
            "windows" => "pc-windows-msvc",
            "linux" => "unknown-linux-gnu",
            other => other,
        }
    )
}

/// True when the config already registers our server pointing at the current
/// bundled binary. A stale entry (different path, e.g. after the app moved)
/// reads as not-installed, so the UI offers to re-point it.
pub fn is_installed() -> bool {
    let (Some(cfg), Some(bin)) = (claude_config_path(), mcp_binary_path()) else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(&cfg) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    json["mcpServers"][SERVER_KEY]["command"].as_str() == Some(&bin.to_string_lossy())
}

/// Register the bundled `mcp` binary in Claude Desktop's config, preserving all
/// other servers and top-level keys. Creates the file and parent directory if
/// they don't exist. Claude Desktop must be restarted to pick up the change.
pub fn install() -> Result<(), String> {
    let cfg = claude_config_path().ok_or("Could not determine the Claude Desktop config path.")?;
    let bin = mcp_binary_path().ok_or("Could not locate the bundled mcp binary.")?;
    if !bin.exists() {
        return Err(format!(
            "Bundled mcp binary not found at {}.",
            bin.display()
        ));
    }

    // Load the existing config, or start fresh if absent. Refuse to proceed on
    // malformed JSON rather than clobber a file we can't safely merge into.
    let mut root: serde_json::Value = match std::fs::read_to_string(&cfg) {
        Ok(text) if !text.trim().is_empty() => serde_json::from_str(&text)
            .map_err(|e| format!("Existing Claude Desktop config is not valid JSON: {e}"))?,
        _ => serde_json::json!({}),
    };
    if !root.is_object() {
        root = serde_json::json!({});
    }

    // Ensure `mcpServers` is an object, then upsert our entry.
    let obj = root.as_object_mut().unwrap();
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    if !servers.is_object() {
        *servers = serde_json::json!({});
    }
    servers.as_object_mut().unwrap().insert(
        SERVER_KEY.to_string(),
        serde_json::json!({ "command": bin.to_string_lossy() }),
    );

    if let Some(parent) = cfg.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let out = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("Failed to serialize the config: {e}"))?;
    std::fs::write(&cfg, out).map_err(|e| format!("Failed to write {}: {e}", cfg.display()))?;
    Ok(())
}
