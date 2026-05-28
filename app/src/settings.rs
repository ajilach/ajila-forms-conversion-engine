//! Persistent application settings stored as TOML.
//!
//! Settings file location:
//! - macOS: ~/Library/Application Support/blueprint/settings.toml
//! - Linux: ~/.config/blueprint/settings.toml
//! - Windows: %APPDATA%/blueprint/settings.toml
//! - WASM: not persisted (defaults only)

use serde::{Deserialize, Serialize};

/// Application settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub always_on_top: bool,
    pub live_preview_port: u16,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            always_on_top: false,
            live_preview_port: 3718,
        }
    }
}

impl AppSettings {
    /// Load settings from disk, falling back to defaults on any error.
    pub fn load() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = Self::settings_path();
            match std::fs::read_to_string(&path) {
                Ok(contents) => toml::from_str(&contents).unwrap_or_default(),
                Err(_) => Self::default(),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self::default()
        }
    }

    /// Save settings to disk.
    pub fn save(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = Self::settings_path();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(contents) = toml::to_string_pretty(self) {
                let _ = std::fs::write(&path, contents);
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn settings_path() -> std::path::PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".config")
        });
        base.join("blueprint").join("settings.toml")
    }
}
