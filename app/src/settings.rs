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

/// Default cap on Reviewer → Author-fix rounds in the conversion pipeline.
pub const DEFAULT_MAX_REVIEW_ROUNDS: usize = 3;

/// Render operator-configured extra instructions as a prompt section, or an
/// empty string when none are set. Appended after the built-in guidance so the
/// hard constraints still take precedence.
pub fn extra_instructions_block(instructions: &str) -> String {
    let trimmed = instructions.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    format!(
        "\n\n--- ADDITIONAL USER INSTRUCTIONS ---\n\
         The operator configured the following extra instructions. Follow them \
         wherever they do not conflict with the hard constraints above:\n{trimmed}"
    )
}

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
    /// Maximum Reviewer → Author-fix rounds in the conversion pipeline before
    /// finalizing with whatever is built. Missing/0 is normalized to
    /// [`DEFAULT_MAX_REVIEW_ROUNDS`] in [`AppSettings::load`].
    #[serde(default)]
    pub max_review_rounds: usize,
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
    /// History-eviction tuning for the agent / smart-edit token usage. See
    /// [`crate::platform::configure_eviction`]. Trailing messages kept verbatim
    /// (even → whole turn-pairs). Missing/0 is normalized to the default in
    /// [`AppSettings::load`].
    pub evict_keep_recent_messages: usize,
    /// Tool-result text longer than this (chars) is elided once stale.
    pub evict_text_over_chars: usize,
    /// Tool-use input longer than this (chars) is elided once stale.
    pub evict_input_over_chars: usize,
    /// Eviction is a no-op until the serialized history exceeds this many bytes.
    pub evict_trigger_bytes: usize,
    /// Extra operator instructions appended to the autonomous conversion agent's
    /// system prompt. Empty = none.
    #[serde(default)]
    pub agent_instructions: String,
    /// Extra operator instructions appended to the structured Smart Edit prompt.
    /// Empty = none.
    #[serde(default)]
    pub smart_edit_instructions: String,
    /// Extra operator instructions appended to the AEM Smart Edit prompt.
    /// Empty = none.
    #[serde(default)]
    pub aem_smart_edit_instructions: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            always_on_top: false,
            live_preview_port: 3718,
            anthropic_api_key: String::new(),
            anthropic_model: "claude-opus-4-8".to_string(),
            max_review_rounds: DEFAULT_MAX_REVIEW_ROUNDS,
            aem_host: "http://localhost:4502".to_string(),
            aem_username: "admin".to_string(),
            aem_password: "admin".to_string(),
            legacy_agent_ui: false,
            evict_keep_recent_messages: crate::platform::DEFAULT_KEEP_RECENT_MESSAGES,
            evict_text_over_chars: crate::platform::DEFAULT_ELIDE_TEXT_OVER_CHARS,
            evict_input_over_chars: crate::platform::DEFAULT_ELIDE_INPUT_OVER_CHARS,
            evict_trigger_bytes: crate::platform::DEFAULT_EVICT_TRIGGER_BYTES,
            agent_instructions: String::new(),
            smart_edit_instructions: String::new(),
            aem_smart_edit_instructions: String::new(),
        }
    }
}

impl AppSettings {
    /// Push runtime tuning (currently history eviction) into the platform layer.
    /// Call at startup and whenever settings change.
    pub fn apply_runtime_config(&self) {
        crate::platform::configure_eviction(
            self.evict_keep_recent_messages,
            self.evict_text_over_chars,
            self.evict_input_over_chars,
            self.evict_trigger_bytes,
        );
    }

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
    /// Coerce missing/zero eviction values to their real defaults. Guards against
    /// configs saved before these fields had sensible defaults (where a `0` would
    /// otherwise show in the UI and read as "off").
    fn normalize_eviction(&mut self) {
        let d = Self::default();
        if self.evict_keep_recent_messages == 0 {
            self.evict_keep_recent_messages = d.evict_keep_recent_messages;
        }
        if self.evict_text_over_chars == 0 {
            self.evict_text_over_chars = d.evict_text_over_chars;
        }
        if self.evict_input_over_chars == 0 {
            self.evict_input_over_chars = d.evict_input_over_chars;
        }
        if self.evict_trigger_bytes == 0 {
            self.evict_trigger_bytes = d.evict_trigger_bytes;
        }
        if self.max_review_rounds == 0 {
            self.max_review_rounds = d.max_review_rounds;
        }
    }

    /// Load settings from the database, falling back to defaults on any error.
    pub fn load() -> Self {
        if let Some(json) = crate::db::get_setting(SETTINGS_KEY)
            && let Ok(mut settings) = serde_json::from_str::<AppSettings>(&json)
        {
            settings.normalize_eviction();
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
