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

/// LLM provider used for Smart Edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LlmProvider {
    /// OpenAI (ChatGPT) models.
    #[serde(rename = "openai")]
    OpenAi,
    /// Anthropic (Claude) models.
    #[serde(rename = "anthropic")]
    Anthropic,
}

impl Default for LlmProvider {
    fn default() -> Self {
        Self::OpenAi
    }
}

/// Application settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub always_on_top: bool,
    pub live_preview_port: u16,
    /// Which LLM provider the AI features (Smart Edit, AI processing) talk to.
    pub provider: LlmProvider,
    /// OpenAI API key used for AI features. Stored in the settings file on disk.
    pub openai_api_key: String,
    /// OpenAI model used for AI features (e.g. "gpt-4o", "gpt-4.1").
    pub openai_model: String,
    /// Anthropic API key used for AI features. Stored in the settings file on disk.
    pub anthropic_api_key: String,
    /// Anthropic model used for AI features (e.g. "claude-opus-4-8").
    pub anthropic_model: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            always_on_top: false,
            live_preview_port: 3718,
            provider: LlmProvider::default(),
            openai_api_key: String::new(),
            openai_model: "gpt-4.1".to_string(),
            anthropic_api_key: String::new(),
            anthropic_model: "claude-opus-4-8".to_string(),
        }
    }
}

impl AppSettings {
    /// The API key for the currently selected provider.
    pub fn active_api_key(&self) -> &str {
        match self.provider {
            LlmProvider::OpenAi => &self.openai_api_key,
            LlmProvider::Anthropic => &self.anthropic_api_key,
        }
    }

    /// The model identifier for the currently selected provider.
    pub fn active_model(&self) -> &str {
        match self.provider {
            LlmProvider::OpenAi => &self.openai_model,
            LlmProvider::Anthropic => &self.anthropic_model,
        }
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(imported) = Self::load_legacy_toml() {
                imported.save();
                return imported;
            }
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
    #[cfg(not(target_arch = "wasm32"))]
    fn load_legacy_toml() -> Option<Self> {
        let path = Self::legacy_settings_path();
        let contents = std::fs::read_to_string(path).ok()?;
        toml::from_str(&contents).ok()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn legacy_settings_path() -> std::path::PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".config")
        });
        base.join("blueprint").join("settings.toml")
    }
}
