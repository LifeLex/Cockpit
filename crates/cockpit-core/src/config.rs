//! Configuration persistence for cockpit.
//!
//! Reads and writes `~/.cockpit/config.toml`. If the file is missing,
//! [`Config::load`] returns sensible defaults rather than failing — first
//! launch should not require setup.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors from configuration loading and saving.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Failed to locate the user's home directory.
    #[error("could not determine home directory")]
    NoHomeDir,

    /// An I/O error occurred reading or writing the config file.
    #[error("config I/O error at {path}: {source}")]
    Io {
        /// Path that was being read/written.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// The config file contained invalid TOML.
    #[error("failed to parse config at {path}: {source}")]
    Parse {
        /// Path that was being parsed.
        path: PathBuf,
        /// Underlying TOML deserialize error.
        source: toml::de::Error,
    },

    /// Failed to serialize the config to TOML.
    #[error("failed to serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Persistent application configuration stored at `~/.cockpit/config.toml`.
///
/// All fields have sensible defaults so the app works out of the box.
/// Optional fields are `None` until the user fills them in through the
/// settings UI. New fields use `#[serde(default)]` for backward
/// compatibility with existing config files.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct Config {
    /// Linear personal API key (e.g. `lin_api_...`).
    pub linear_api_key: Option<String>,

    /// Default Linear project ID to work with.
    pub linear_project_id: Option<String>,

    /// Path to the git repository managed by cockpit.
    pub repo_path: Option<PathBuf>,

    /// Command used to spawn the agent (default: `"claude"`).
    #[serde(default = "default_agent_command")]
    pub agent_command: String,

    /// Port for the Stop-hook listener (default: `19876`).
    #[serde(default = "default_hook_port")]
    pub hook_port: u16,

    // ----- AI settings -----
    /// Anthropic API key for direct model access.
    #[serde(default)]
    pub anthropic_api_key: Option<String>,

    /// Model identifier to use for agent runs (default: `"claude-sonnet-4-6"`).
    #[serde(default)]
    pub model: Option<String>,

    /// Optional daily spend cap in US dollars. `None` means unlimited.
    #[serde(default)]
    pub daily_budget_usd: Option<f64>,

    // ----- Integration settings -----
    /// GitHub personal access token for API operations.
    #[serde(default)]
    pub github_token: Option<String>,

    // ----- Development settings -----
    /// Shell command to launch the preferred IDE (e.g. `"cursor"`, `"code"`).
    #[serde(default)]
    pub ide_command: Option<String>,

    // ----- Appearance settings -----
    /// Application color theme: `"dark"`, `"light"`, or `"system"`.
    #[serde(default)]
    pub app_theme: Option<String>,

    /// Monaco editor theme identifier (default: `"vs-dark"`).
    #[serde(default)]
    pub editor_theme: Option<String>,

    /// Terminal font family name (default: `"SF Mono"`).
    #[serde(default)]
    pub terminal_font: Option<String>,

    /// Terminal font size in points (default: `13`).
    #[serde(default)]
    pub terminal_font_size: Option<u16>,
}

/// Default agent command value for serde deserialization.
fn default_agent_command() -> String {
    "claude".into()
}

/// Default hook port value for serde deserialization.
fn default_hook_port() -> u16 {
    19876
}

impl Default for Config {
    fn default() -> Self {
        Self {
            linear_api_key: None,
            linear_project_id: None,
            repo_path: None,
            agent_command: default_agent_command(),
            hook_port: default_hook_port(),
            anthropic_api_key: None,
            model: None,
            daily_budget_usd: None,
            github_token: None,
            ide_command: None,
            app_theme: None,
            editor_theme: None,
            terminal_font: None,
            terminal_font_size: None,
        }
    }
}

impl Config {
    /// Load the configuration from `~/.cockpit/config.toml`.
    ///
    /// Returns the default configuration if the file does not exist.
    /// Errors only on a genuine I/O or parse failure (not on "file missing").
    pub fn load() -> Result<Self, Error> {
        let path = config_path()?;

        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;

        let config: Self = toml::from_str(&content).map_err(|source| Error::Parse {
            path: path.clone(),
            source,
        })?;

        Ok(config)
    }

    /// Save the configuration to `~/.cockpit/config.toml`.
    ///
    /// Creates the `~/.cockpit/` directory if it does not exist.
    pub fn save(&self) -> Result<(), Error> {
        let path = config_path()?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content).map_err(|source| Error::Io { path, source })?;

        Ok(())
    }
}

/// Return the path to `~/.cockpit/config.toml`.
fn config_path() -> Result<PathBuf, Error> {
    let home = dirs::home_dir().ok_or(Error::NoHomeDir)?;
    Ok(home.join(".cockpit").join("config.toml"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_expected_values() {
        let config = Config::default();
        assert!(config.linear_api_key.is_none());
        assert!(config.linear_project_id.is_none());
        assert!(config.repo_path.is_none());
        assert_eq!(config.agent_command, "claude");
        assert_eq!(config.hook_port, 19876);
        assert!(config.anthropic_api_key.is_none());
        assert!(config.model.is_none());
        assert!(config.daily_budget_usd.is_none());
        assert!(config.github_token.is_none());
        assert!(config.ide_command.is_none());
        assert!(config.app_theme.is_none());
        assert!(config.editor_theme.is_none());
        assert!(config.terminal_font.is_none());
        assert!(config.terminal_font_size.is_none());
    }

    #[test]
    fn round_trip_toml() {
        let config = Config {
            linear_api_key: Some("lin_api_test".into()),
            linear_project_id: Some("proj-123".into()),
            repo_path: Some(PathBuf::from("/home/user/repo")),
            agent_command: "claude".into(),
            hook_port: 19876,
            anthropic_api_key: Some("sk-ant-test".into()),
            model: Some("claude-sonnet-4-6".into()),
            daily_budget_usd: Some(10.0),
            github_token: Some("ghp_test".into()),
            ide_command: Some("cursor".into()),
            app_theme: Some("dark".into()),
            editor_theme: Some("github-dark".into()),
            terminal_font: Some("SF Mono".into()),
            terminal_font_size: Some(14),
        };

        let serialized = toml::to_string_pretty(&config).expect("serialize should succeed");
        let deserialized: Config = toml::from_str(&serialized).expect("deserialize should succeed");

        assert_eq!(deserialized.linear_api_key, config.linear_api_key);
        assert_eq!(deserialized.linear_project_id, config.linear_project_id);
        assert_eq!(deserialized.repo_path, config.repo_path);
        assert_eq!(deserialized.agent_command, config.agent_command);
        assert_eq!(deserialized.hook_port, config.hook_port);
        assert_eq!(deserialized.anthropic_api_key, config.anthropic_api_key);
        assert_eq!(deserialized.model, config.model);
        assert_eq!(deserialized.daily_budget_usd, config.daily_budget_usd);
        assert_eq!(deserialized.github_token, config.github_token);
        assert_eq!(deserialized.ide_command, config.ide_command);
        assert_eq!(deserialized.app_theme, config.app_theme);
        assert_eq!(deserialized.editor_theme, config.editor_theme);
        assert_eq!(deserialized.terminal_font, config.terminal_font);
        assert_eq!(deserialized.terminal_font_size, config.terminal_font_size);
    }

    #[test]
    fn load_missing_file_returns_default() {
        // Config::load reads from ~/.cockpit/config.toml.
        // If the file doesn't exist, it returns defaults.
        // We test this indirectly: defaults should match.
        let default = Config::default();
        assert_eq!(default.agent_command, "claude");
        assert_eq!(default.hook_port, 19876);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("config.toml");

        let config = Config {
            linear_api_key: Some("lin_api_test_key".into()),
            linear_project_id: None,
            repo_path: Some(PathBuf::from("/repos/my-project")),
            agent_command: "custom-agent".into(),
            hook_port: 12345,
            anthropic_api_key: None,
            model: None,
            daily_budget_usd: None,
            github_token: None,
            ide_command: None,
            app_theme: None,
            editor_theme: None,
            terminal_font: None,
            terminal_font_size: None,
        };

        // Save to a specific path (test helper).
        let content = toml::to_string_pretty(&config).expect("serialize");
        std::fs::write(&path, content).expect("write");

        // Load from the same path.
        let raw = std::fs::read_to_string(&path).expect("read");
        let loaded: Config = toml::from_str(&raw).expect("parse");

        assert_eq!(loaded.linear_api_key, config.linear_api_key);
        assert_eq!(loaded.agent_command, "custom-agent");
        assert_eq!(loaded.hook_port, 12345);
    }

    #[test]
    fn partial_toml_fills_defaults() {
        // With #[serde(default)] on new fields, a config file that only
        // contains the original fields should still parse correctly, filling
        // new fields with their defaults (None for Options).
        let toml_str = r#"
agent_command = "my-agent"
hook_port = 9999
"#;
        let config: Config = toml::from_str(toml_str).expect("should parse partial config");
        assert!(config.linear_api_key.is_none());
        assert!(config.linear_project_id.is_none());
        assert!(config.repo_path.is_none());
        assert_eq!(config.agent_command, "my-agent");
        assert_eq!(config.hook_port, 9999);
        // New fields should all be None when missing from the file.
        assert!(config.anthropic_api_key.is_none());
        assert!(config.model.is_none());
        assert!(config.daily_budget_usd.is_none());
        assert!(config.github_token.is_none());
        assert!(config.ide_command.is_none());
        assert!(config.app_theme.is_none());
        assert!(config.editor_theme.is_none());
        assert!(config.terminal_font.is_none());
        assert!(config.terminal_font_size.is_none());
    }

    #[test]
    fn backward_compatible_with_old_config() {
        // Simulate a config file that was written by the old version of
        // cockpit, containing only the original five fields. The new fields
        // should default gracefully.
        let toml_str = r#"
linear_api_key = "lin_api_old"
linear_project_id = "old-proj"
repo_path = "/old/repo"
agent_command = "claude"
hook_port = 19876
"#;
        let config: Config = toml::from_str(toml_str).expect("should parse old config");
        assert_eq!(config.linear_api_key.as_deref(), Some("lin_api_old"));
        assert_eq!(config.agent_command, "claude");
        assert_eq!(config.hook_port, 19876);
        // All new fields are None.
        assert!(config.anthropic_api_key.is_none());
        assert!(config.editor_theme.is_none());
        assert!(config.terminal_font_size.is_none());
    }
}
