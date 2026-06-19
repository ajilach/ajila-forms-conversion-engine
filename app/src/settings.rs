//! Persistent application settings stored in the local SQLite database.
//!
//! Settings are serialized as JSON and stored under the `app` key in the
//! `settings` table of `<config_dir>/blueprint/history.db`. On first run the
//! legacy `settings.toml` file (if present) is imported once.
//!
//! On WASM the database is unavailable, so settings fall back to defaults.

use serde::{Deserialize, Serialize};

/// Key under which the serialized settings are stored.
const SETTINGS_KEY: &str = "app";

/// Application settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub always_on_top: bool,
    pub live_preview_port: u16,
    /// Anthropic API key used for AI features. Stored in the settings file on disk.
    pub anthropic_api_key: String,
    /// Anthropic model used for AI features (e.g. "claude-opus-4-8").
    pub anthropic_model: String,
    /// Base URL of the AEM author instance for package upload
    /// (e.g. "http://localhost:4502").
    pub aem_host: String,
    /// Username for AEM HTTP basic auth.
    pub aem_username: String,
    /// Password for AEM HTTP basic auth. Stored locally on disk.
    pub aem_password: String,
    /// When `true`, restore the previous stacked upload / progress / results
    /// layout (and normal, non-agent processing) instead of the simplified
    /// agent flow.
    #[serde(default)]
    pub legacy_agent_ui: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            always_on_top: false,
            live_preview_port: 3718,
            anthropic_api_key: String::new(),
            anthropic_model: "claude-opus-4-8".to_string(),
            aem_host: "http://localhost:4502".to_string(),
            aem_username: "admin".to_string(),
            aem_password: "admin".to_string(),
            legacy_agent_ui: false,
        }
    }
}

impl AppSettings {
    /// The Anthropic API key used for AI features.
    pub fn active_api_key(&self) -> &str {
        &self.anthropic_api_key
    }

    /// The Anthropic model identifier used for AI features.
    pub fn active_model(&self) -> &str {
        &self.anthropic_model
    }

    /// Build an AEM upload connection from the configured host/credentials,
    /// or `None` if host or username have not been set.
    pub fn aem_connection(&self) -> Option<blueprint::AemConnection> {
        let host = self.aem_host.trim();
        let username = self.aem_username.trim();
        if host.is_empty() || username.is_empty() {
            return None;
        }
        Some(blueprint::AemConnection {
            host: host.trim_end_matches('/').to_string(),
            username: username.to_string(),
            password: self.aem_password.clone(),
        })
    }
}

impl AppSettings {
    /// Load settings from the database, falling back to defaults on any error.
    pub fn load() -> Self {
        if let Some(json) = crate::db::get_setting(SETTINGS_KEY)
            && let Ok(settings) = serde_json::from_str::<AppSettings>(&json)
        {
            return settings;
        }

        // No settings in the database yet: try a one-time import from the
        // legacy TOML file, then persist it into the database.
        if let Some(imported) = Self::load_legacy_toml() {
            imported.save();
            return imported;
        }

        Self::default()
    }

    /// Save settings to the database.
    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string(self) {
            crate::db::set_setting(SETTINGS_KEY, &json);
        }
    }

    /// One-time import of the legacy `settings.toml` file, if it exists.
    fn load_legacy_toml() -> Option<Self> {
        let path = Self::legacy_settings_path();
        let contents = std::fs::read_to_string(path).ok()?;
        toml::from_str(&contents).ok()
    }

    fn legacy_settings_path() -> std::path::PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".config")
        });
        base.join("blueprint").join("settings.toml")
    }
}
