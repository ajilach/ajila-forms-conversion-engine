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
    reveal_in_file_explorer(&path);
    Ok(path)
}

/// Reveal an existing file in the platform's file manager.
///
/// Best-effort: a file manager that refuses to open is not worth reporting, the
/// file is on disk either way.
pub fn reveal_in_file_explorer(path: &Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();

    // xdg-open cannot select a file, so revealing means opening its folder.
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();

    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer")
        .args(["/select,", &path.to_string_lossy()])
        .spawn();
}
