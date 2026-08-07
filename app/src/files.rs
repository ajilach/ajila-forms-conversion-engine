//! Desktop file handoff: save an artefact to the user's Downloads folder and
//! show it to them, either revealed in the file manager or opened directly.

use std::path::{Path, PathBuf};

/// Write `data` to `~/Downloads/<filename>` and return where it landed.
///
/// The only place that decides where the app puts a file the user asked for.
pub fn save_to_downloads(filename: &str, data: &[u8]) -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Could not determine the home directory.")?;
    let path = home.join("Downloads").join(filename);
    std::fs::write(&path, data).map_err(|e| format!("Could not save {}: {e}", path.display()))?;
    Ok(path)
}

/// Save an artefact and reveal it in the file manager.
pub fn download_file(data: &[u8], filename: &str) -> Result<PathBuf, String> {
    let path = save_to_downloads(filename, data)?;
    open_path(&path, Reveal::InFolder);
    Ok(path)
}

/// Save an HTML rendering and open it in the browser.
pub fn show_html_preview(html: &str, filename: &str) -> Result<PathBuf, String> {
    let path = save_to_downloads(filename, html.as_bytes())?;
    open_path(&path, Reveal::Open);
    Ok(path)
}

/// Whether to hand the path itself to the desktop, or point the file manager at
/// it inside its folder.
enum Reveal {
    Open,
    InFolder,
}

/// Hand a path to the desktop environment. Best-effort: if the platform's
/// opener is missing there is nothing useful to report, the file is saved either
/// way.
fn open_path(path: &Path, how: Reveal) {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        if matches!(how, Reveal::InFolder) {
            cmd.arg("-R");
        }
        let _ = cmd.arg(path).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        // xdg-open cannot select a file, so reveal means "open the folder".
        let target = match how {
            Reveal::Open => Some(path),
            Reveal::InFolder => path.parent(),
        };
        if let Some(target) = target {
            let _ = std::process::Command::new("xdg-open").arg(target).spawn();
        }
    }
    #[cfg(target_os = "windows")]
    {
        let _ = match how {
            Reveal::Open => std::process::Command::new("cmd")
                .args(["/C", "start", "", &path.to_string_lossy()])
                .spawn(),
            Reveal::InFolder => std::process::Command::new("explorer")
                .args(["/select,", &path.to_string_lossy()])
                .spawn(),
        };
    }
}

/// Reveal an existing file in the platform's file manager.
pub fn reveal_in_file_explorer(path: &Path) {
    open_path(path, Reveal::InFolder);
}
