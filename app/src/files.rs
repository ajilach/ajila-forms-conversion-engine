//! Desktop file handoff: save an artefact to the user's Downloads folder and
//! show it to them, either revealed in the file manager or opened directly.

use std::path::{Path, PathBuf};

/// Where an artefact called `filename` goes. The one place that decides.
pub fn downloads_path(filename: &str) -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Could not determine the home directory.")?;
    Ok(home.join("Downloads").join(filename))
}

/// Write `data` to `~/Downloads/<filename>` and return where it landed.
pub fn save_to_downloads(filename: &str, data: &[u8]) -> Result<PathBuf, String> {
    let path = downloads_path(filename)?;
    std::fs::write(&path, data).map_err(|e| format!("Could not save {}: {e}", path.display()))?;
    Ok(path)
}

/// Save an artefact and reveal it in the file manager.
///
/// A file manager that refuses to open is not worth failing over — the file the
/// user asked for is on disk either way — so only the save can fail here.
pub fn download_file(data: &[u8], filename: &str) -> Result<PathBuf, String> {
    let path = save_to_downloads(filename, data)?;
    let _ = open_path(&path, Reveal::InFolder);
    Ok(path)
}

/// Save an HTML rendering and open it in the browser.
///
/// Unlike a download, opening *is* the point of a preview, so a failure to
/// launch the browser is reported rather than swallowed.
pub fn show_html_preview(html: &str, filename: &str) -> Result<PathBuf, String> {
    let path = save_to_downloads(filename, html.as_bytes())?;
    open_path(&path, Reveal::Open)?;
    Ok(path)
}

/// Whether to hand the path itself to the desktop, or point the file manager at
/// it inside its folder.
#[derive(Clone, Copy)]
enum Reveal {
    Open,
    InFolder,
}

/// Hand a path to the desktop environment.
fn open_path(path: &Path, how: Reveal) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let spawned = {
        let mut cmd = std::process::Command::new("open");
        if matches!(how, Reveal::InFolder) {
            cmd.arg("-R");
        }
        cmd.arg(path).spawn()
    };

    #[cfg(target_os = "linux")]
    let spawned = {
        // xdg-open cannot select a file, so revealing means opening its folder.
        let target = match how {
            Reveal::Open => path,
            Reveal::InFolder => path.parent().unwrap_or(path),
        };
        std::process::Command::new("xdg-open").arg(target).spawn()
    };

    #[cfg(target_os = "windows")]
    let spawned = match how {
        Reveal::Open => std::process::Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .spawn(),
        Reveal::InFolder => std::process::Command::new("explorer")
            .args(["/select,", &path.to_string_lossy()])
            .spawn(),
    };

    spawned
        .map(|_| ())
        .map_err(|e| format!("Saved to {}, but could not open it: {e}", path.display()))
}

/// Reveal an existing file in the platform's file manager.
pub fn reveal_in_file_explorer(path: &Path) {
    let _ = open_path(path, Reveal::InFolder);
}
