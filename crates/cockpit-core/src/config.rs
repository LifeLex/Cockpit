//! Configuration persistence for cockpit.
//!
//! Reads and writes `~/.cockpit/config.toml`. If the file is missing,
//! [`Config::load`] returns sensible defaults rather than failing — first
//! launch should not require setup.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::AgentMode;

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
// AgentPrompts
// ---------------------------------------------------------------------------

/// Per-[`AgentMode`] custom prompt fragment overrides.
///
/// Each field holds a user-authored preamble that is injected **verbatim**
/// into the deterministic prompt assembly for that mode (never paraphrased).
/// `None` means "no override": assembly falls back to the builtin intent for
/// that mode. A separate struct (rather than a map) keeps serde/TOML/ts-rs
/// output stable and self-documenting.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct AgentPrompts {
    /// Override for [`AgentMode::Implement`].
    #[serde(default)]
    pub implement: Option<String>,
    /// Override for [`AgentMode::Plan`].
    #[serde(default)]
    pub plan: Option<String>,
    /// Override for [`AgentMode::Fix`].
    #[serde(default)]
    pub fix: Option<String>,
    /// Override for [`AgentMode::Restack`].
    #[serde(default)]
    pub restack: Option<String>,
    /// Override for [`AgentMode::Review`].
    #[serde(default)]
    pub review: Option<String>,
}

impl AgentPrompts {
    /// Return the stored override for `mode`, if any.
    ///
    /// An empty override string is treated as "no override" (`None`) so a
    /// blank editor field falls back to the builtin rather than injecting an
    /// empty preamble.
    pub fn for_mode(&self, mode: AgentMode) -> Option<&str> {
        let value = match mode {
            AgentMode::Implement => self.implement.as_deref(),
            AgentMode::Plan => self.plan.as_deref(),
            AgentMode::Fix => self.fix.as_deref(),
            AgentMode::Restack => self.restack.as_deref(),
            AgentMode::Review => self.review.as_deref(),
        };
        value.filter(|s| !s.trim().is_empty())
    }

    /// Set (or clear) the override for `mode`.
    ///
    /// An empty or whitespace-only `text` clears the override (stores `None`),
    /// which resets the mode back to its builtin default.
    pub fn set_mode(&mut self, mode: AgentMode, text: Option<String>) {
        let value = text.filter(|s| !s.trim().is_empty());
        match mode {
            AgentMode::Implement => self.implement = value,
            AgentMode::Plan => self.plan = value,
            AgentMode::Fix => self.fix = value,
            AgentMode::Restack => self.restack = value,
            AgentMode::Review => self.review = value,
        }
    }
}

// ---------------------------------------------------------------------------
// SkillsGithub
// ---------------------------------------------------------------------------

/// GitHub source configuration for syncing installable skills.
///
/// Points at a directory in a repo that holds one skill per subdirectory
/// (`<name>/SKILL.md`). Authentication is the user's existing `gh auth` — there
/// is deliberately no token field here (see PROGRAM_PLAN #3/#8).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct SkillsGithub {
    /// Repository owner (user or org).
    pub owner: String,
    /// Repository name.
    pub repo: String,
    /// Branch to sync from (e.g. `"main"`).
    pub branch: String,
    /// Repo-relative path to the directory that holds the skills.
    pub path: String,
    /// Whether cockpit should sync on relevant triggers automatically.
    #[serde(default)]
    pub auto_sync: bool,
}

// ---------------------------------------------------------------------------
// LspServers
// ---------------------------------------------------------------------------

/// Language-server command configuration for the Monaco LSP bridge.
///
/// Cockpit does not bundle language servers. The user installs them (for
/// example `npm i -g pyright typescript-language-server`) and cockpit resolves
/// each command via the login-shell `PATH` at spawn time. These fields let a
/// user point at a non-default binary or a wrapper script; when a field is
/// `None`, the built-in default command name is used and looked up on `PATH`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct LspServers {
    /// Whether the Monaco LSP bridge is active at all (default: `true`).
    ///
    /// When `false`, cockpit never starts a language-server bridge and the
    /// diff editor falls back to plain syntax highlighting only.
    #[serde(default = "default_lsp_enabled")]
    pub enabled: bool,

    /// Override for the Python language-server command
    /// (default: `pyright-langserver`).
    ///
    /// The command is invoked with `--stdio`. `None` uses the default name,
    /// resolved on `PATH`.
    #[serde(default)]
    pub pyright_command: Option<String>,

    /// Override for the TypeScript/JavaScript language-server command
    /// (default: `typescript-language-server`).
    ///
    /// The command is invoked with `--stdio`. `None` uses the default name,
    /// resolved on `PATH`.
    #[serde(default)]
    pub typescript_command: Option<String>,
}

/// Default value for [`LspServers::enabled`].
fn default_lsp_enabled() -> bool {
    true
}

impl Default for LspServers {
    fn default() -> Self {
        Self {
            enabled: default_lsp_enabled(),
            pyright_command: None,
            typescript_command: None,
        }
    }
}

impl LspServers {
    /// Resolve the configured command for `language`, falling back to the
    /// built-in default when no override is set.
    ///
    /// Returns `None` for a language cockpit has no language server for.
    pub fn command_for(&self, language: LspLanguage) -> String {
        match language {
            LspLanguage::Python => self
                .pyright_command
                .clone()
                .unwrap_or_else(|| "pyright-langserver".to_owned()),
            LspLanguage::TypeScript => self
                .typescript_command
                .clone()
                .unwrap_or_else(|| "typescript-language-server".to_owned()),
        }
    }
}

/// A language for which cockpit runs a Monaco language-server bridge.
///
/// Kept as a small closed enum (not an open string) so the set of supported
/// servers is explicit and exhaustively matched at every use site.
//
// No `#[derive(TS)]`: the frontend keys off Monaco's raw `languageId` strings,
// not this Rust-side enum, so exporting a binding would emit an orphan `.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LspLanguage {
    /// Python, served by `pyright-langserver`.
    Python,
    /// TypeScript and JavaScript, served by `typescript-language-server`.
    TypeScript,
}

impl LspLanguage {
    /// Parse a Monaco `languageId` into a supported [`LspLanguage`].
    ///
    /// Returns `None` for languages without a configured server (the caller
    /// then skips the bridge for that model).
    pub fn from_language_id(language_id: &str) -> Option<Self> {
        match language_id {
            "python" => Some(Self::Python),
            "typescript" | "javascript" | "typescriptreact" | "javascriptreact" => {
                Some(Self::TypeScript)
            }
            _ => None,
        }
    }
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

    /// Maximum number of implementer agents to run concurrently during a
    /// plan-approval fan-out (default: `3`).
    ///
    /// Bounds resource use when a project's frontier is large: `spawn_batch`
    /// spawns at most this many agents at once and waits for a slot before
    /// starting the next.
    #[serde(default = "default_max_parallel_agents")]
    pub max_parallel_agents: u16,

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

    // ----- Agent prompt customization -----
    /// Per-[`AgentMode`] custom prompt fragment overrides.
    ///
    /// Each present override is injected verbatim into the corresponding
    /// mode's prompt; absent modes fall back to the builtin intent.
    #[serde(default)]
    pub agent_prompts: AgentPrompts,

    // ----- Skills sync -----
    /// Optional GitHub source for installable skills.
    ///
    /// `None` means no remote source is configured; skills are then local-only.
    #[serde(default)]
    pub skills_github: Option<SkillsGithub>,

    // ----- Language servers (Monaco LSP bridge) -----
    /// Language-server command configuration for the Monaco LSP bridge.
    ///
    /// Controls whether the bridge runs and which binaries back each language.
    #[serde(default)]
    pub lsp_servers: LspServers,

    // ----- Notifications -----
    /// Seconds between background board polls for review-request changes.
    ///
    /// `None` or `0` disables background board polling. The UI default is 90.
    #[serde(default)]
    pub notify_poll_secs: Option<u16>,
}

/// Default agent command value for serde deserialization.
fn default_agent_command() -> String {
    "claude".into()
}

/// Default hook port value for serde deserialization.
fn default_hook_port() -> u16 {
    19876
}

/// Default maximum number of concurrent implementer agents.
fn default_max_parallel_agents() -> u16 {
    3
}

impl Default for Config {
    fn default() -> Self {
        Self {
            linear_api_key: None,
            linear_project_id: None,
            repo_path: None,
            agent_command: default_agent_command(),
            hook_port: default_hook_port(),
            max_parallel_agents: default_max_parallel_agents(),
            ide_command: None,
            app_theme: None,
            editor_theme: None,
            agent_prompts: AgentPrompts::default(),
            skills_github: None,
            lsp_servers: LspServers::default(),
            notify_poll_secs: None,
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

/// Return the cockpit home directory (`$HOME/.cockpit`).
///
/// If the `COCKPIT_HOME` environment variable is set, its value is used
/// verbatim instead. This override exists so tests can isolate their
/// on-disk state; production callers should leave it unset and get the
/// default under the user's home directory.
///
/// Returns [`Error::NoHomeDir`] if neither the override nor the home
/// directory can be resolved.
pub fn cockpit_home() -> Result<PathBuf, Error> {
    // An explicitly-empty override would make every derived path relative to
    // the CWD (which for a bundled app could be `/`), so treat empty as unset.
    if let Some(override_dir) = std::env::var_os("COCKPIT_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(override_dir));
    }
    let home = dirs::home_dir().ok_or(Error::NoHomeDir)?;
    Ok(home.join(".cockpit"))
}

/// Return the directory that holds agent worktrees (`<cockpit_home>/worktrees`).
///
/// Worktrees live outside the managed repository so a bundled app never
/// writes scratch state into the user's checkout.
pub fn worktrees_dir() -> Result<PathBuf, Error> {
    Ok(cockpit_home()?.join("worktrees"))
}

/// Return the directory that holds agent run logs (`<cockpit_home>/logs`).
///
/// Logs are centralized here rather than inside each worktree so they
/// survive worktree cleanup and are easy to locate.
pub fn logs_dir() -> Result<PathBuf, Error> {
    Ok(cockpit_home()?.join("logs"))
}

/// Return the directory that holds planner-written plan documents
/// (`<cockpit_home>/plans`).
///
/// Each project's finished plan is a markdown file under this directory,
/// written by the planner agent and parsed back into [`crate::model::PlanDoc`].
pub fn plans_dir() -> Result<PathBuf, Error> {
    Ok(cockpit_home()?.join("plans"))
}

/// Return the path to the markdown plan file for `project_id`.
///
/// The convention is `<cockpit_home>/plans/<slug>.md`, where `<slug>` is the
/// project id with filesystem-hostile characters replaced by `-` so an
/// arbitrary Linear project id or ad-hoc name yields a safe single filename.
pub fn plan_file_path(project_id: &str) -> Result<PathBuf, Error> {
    let slug = path_slug(project_id, "plan");
    Ok(plans_dir()?.join(format!("{slug}.md")))
}

/// Return the directory that holds advisory reviewer findings files
/// (`<cockpit_home>/findings`).
///
/// Findings are the JSON arrays written by the read-only pre-pass reviewer
/// ([`crate::model::AgentMode::Review`]); each PR's findings are one file under
/// this directory, parsed back with [`crate::findings::parse_findings`]. Like
/// [`plans_dir`], this only resolves the path — it does not create the
/// directory.
pub fn findings_dir() -> Result<PathBuf, Error> {
    Ok(cockpit_home()?.join("findings"))
}

/// Return the path to the findings JSON file for the PR identified by `pr`.
///
/// The convention is `<cockpit_home>/findings/<slug>.json`, where `<slug>` is
/// `pr` with filesystem-hostile characters replaced by `-` (the same
/// sanitization [`plan_file_path`] applies), so an arbitrary PR reference such
/// as `owner/repo#42` yields a safe single filename. Like [`plan_file_path`],
/// this only resolves the path and does not create the directory.
pub fn findings_file_path(pr: &str) -> Result<PathBuf, Error> {
    let slug = path_slug(pr, "findings");
    Ok(findings_dir()?.join(format!("{slug}.json")))
}

/// Turn an arbitrary id into a single filesystem-safe filename stem.
///
/// Every non-alphanumeric character becomes `-`. An id that reduces to only
/// separators (or is empty) falls back to `fallback`, so we never emit an empty
/// or dotfile-only name. Shared by [`plan_file_path`] and [`findings_file_path`]
/// so both use identical sanitization.
fn path_slug(id: &str, fallback: &str) -> String {
    let slug: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if slug.trim_matches('-').is_empty() {
        fallback.to_owned()
    } else {
        slug
    }
}

/// Return the path to `<cockpit_home>/config.toml`.
fn config_path() -> Result<PathBuf, Error> {
    Ok(cockpit_home()?.join("config.toml"))
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
        assert_eq!(config.max_parallel_agents, 3);
        assert!(config.ide_command.is_none());
        assert!(config.app_theme.is_none());
        assert!(config.editor_theme.is_none());
        assert!(config.notify_poll_secs.is_none());
    }

    #[test]
    fn round_trip_toml() {
        let config = Config {
            linear_api_key: Some("lin_api_test".into()),
            linear_project_id: Some("proj-123".into()),
            repo_path: Some(PathBuf::from("/home/user/repo")),
            agent_command: "claude".into(),
            hook_port: 19876,
            max_parallel_agents: 3,
            ide_command: Some("cursor".into()),
            app_theme: Some("dark".into()),
            editor_theme: Some("github-dark".into()),
            agent_prompts: AgentPrompts {
                implement: Some("custom implement".into()),
                plan: None,
                fix: Some("custom fix".into()),
                restack: None,
                review: Some("custom review".into()),
            },
            skills_github: Some(SkillsGithub {
                owner: "acme".into(),
                repo: "skills".into(),
                branch: "main".into(),
                path: "skills".into(),
                auto_sync: true,
            }),
            lsp_servers: LspServers {
                enabled: true,
                pyright_command: Some("pyright-langserver".into()),
                typescript_command: None,
            },
            notify_poll_secs: Some(90),
        };

        let serialized = toml::to_string_pretty(&config).expect("serialize should succeed");
        let deserialized: Config = toml::from_str(&serialized).expect("deserialize should succeed");

        assert_eq!(deserialized.linear_api_key, config.linear_api_key);
        assert_eq!(deserialized.linear_project_id, config.linear_project_id);
        assert_eq!(deserialized.repo_path, config.repo_path);
        assert_eq!(deserialized.agent_command, config.agent_command);
        assert_eq!(deserialized.hook_port, config.hook_port);
        assert_eq!(deserialized.max_parallel_agents, config.max_parallel_agents);
        assert_eq!(deserialized.ide_command, config.ide_command);
        assert_eq!(deserialized.app_theme, config.app_theme);
        assert_eq!(deserialized.editor_theme, config.editor_theme);
        assert_eq!(
            deserialized.agent_prompts.implement,
            config.agent_prompts.implement
        );
        assert_eq!(deserialized.agent_prompts.plan, config.agent_prompts.plan);
        assert_eq!(deserialized.agent_prompts.fix, config.agent_prompts.fix);
        assert_eq!(
            deserialized.agent_prompts.restack,
            config.agent_prompts.restack
        );
        assert_eq!(
            deserialized.agent_prompts.review,
            config.agent_prompts.review
        );
        assert_eq!(deserialized.skills_github, config.skills_github);
        assert_eq!(deserialized.lsp_servers, config.lsp_servers);
        assert_eq!(deserialized.notify_poll_secs, config.notify_poll_secs);
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
            max_parallel_agents: 3,
            ide_command: None,
            app_theme: None,
            editor_theme: None,
            agent_prompts: AgentPrompts::default(),
            skills_github: None,
            lsp_servers: LspServers::default(),
            notify_poll_secs: None,
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
        // max_parallel_agents defaults to 3 when absent from the file.
        assert_eq!(config.max_parallel_agents, 3);
        // New fields should all be None when missing from the file.
        assert!(config.ide_command.is_none());
        assert!(config.app_theme.is_none());
        assert!(config.editor_theme.is_none());
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
        // New fields default when absent.
        assert!(config.editor_theme.is_none());
        // agent_prompts must default cleanly when absent (migration).
        assert!(config.agent_prompts.for_mode(AgentMode::Fix).is_none());
        assert!(config.agent_prompts.for_mode(AgentMode::Plan).is_none());
        assert!(
            config
                .agent_prompts
                .for_mode(AgentMode::Implement)
                .is_none()
        );
        assert!(config.agent_prompts.for_mode(AgentMode::Restack).is_none());
        assert!(config.agent_prompts.for_mode(AgentMode::Review).is_none());
        // skills_github must default to None when absent (migration).
        assert!(config.skills_github.is_none());
        // notify_poll_secs must default to None when absent (migration).
        assert!(config.notify_poll_secs.is_none());
    }

    #[test]
    fn config_ignores_removed_fields() {
        // A config file written by an older cockpit that carried the now-removed
        // AI/integration/terminal fields must still load: serde ignores unknown
        // fields (no `deny_unknown_fields`), so the removed keys are dropped
        // silently and the surviving fields parse as usual.
        let toml_str = r#"
linear_api_key = "lin_api_old"
agent_command = "claude"
hook_port = 19876
anthropic_api_key = "sk-ant-removed"
model = "claude-sonnet-4-6"
daily_budget_usd = 10.0
github_token = "ghp_removed"
terminal_font = "SF Mono"
terminal_font_size = 14
"#;
        let config: Config =
            toml::from_str(toml_str).expect("removed fields must not break loading");
        assert_eq!(config.linear_api_key.as_deref(), Some("lin_api_old"));
        assert_eq!(config.agent_command, "claude");
        assert_eq!(config.hook_port, 19876);
    }

    #[test]
    fn skills_github_round_trips_toml() {
        let cfg = SkillsGithub {
            owner: "acme".into(),
            repo: "conventions".into(),
            branch: "main".into(),
            path: "skills".into(),
            auto_sync: true,
        };
        let serialized = toml::to_string_pretty(&cfg).expect("serialize");
        let deserialized: SkillsGithub = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(deserialized, cfg);
    }

    #[test]
    fn skills_github_absent_auto_sync_defaults_false() {
        // A config file specifying the source but omitting auto_sync should
        // default auto_sync to false rather than fail to parse.
        let toml_str = r#"
[skills_github]
owner = "acme"
repo = "conventions"
branch = "main"
path = "skills"
"#;
        let config: Config = toml::from_str(toml_str).expect("should parse");
        let gh = config.skills_github.expect("skills_github present");
        assert_eq!(gh.owner, "acme");
        assert!(!gh.auto_sync);
    }

    #[test]
    fn agent_prompts_for_mode_returns_override() {
        let prompts = AgentPrompts {
            implement: Some("build it".into()),
            plan: None,
            fix: Some("fix it".into()),
            restack: None,
            review: Some("review it".into()),
        };
        assert_eq!(prompts.for_mode(AgentMode::Implement), Some("build it"));
        assert_eq!(prompts.for_mode(AgentMode::Fix), Some("fix it"));
        assert_eq!(prompts.for_mode(AgentMode::Plan), None);
        assert_eq!(prompts.for_mode(AgentMode::Restack), None);
        assert_eq!(prompts.for_mode(AgentMode::Review), Some("review it"));
    }

    #[test]
    fn agent_prompts_blank_override_is_none() {
        let prompts = AgentPrompts {
            implement: Some("   ".into()),
            plan: Some(String::new()),
            fix: None,
            restack: None,
            review: None,
        };
        // Whitespace-only and empty overrides fall back to builtin (None).
        assert_eq!(prompts.for_mode(AgentMode::Implement), None);
        assert_eq!(prompts.for_mode(AgentMode::Plan), None);
    }

    #[test]
    fn agent_prompts_set_mode_sets_and_clears() {
        let mut prompts = AgentPrompts::default();
        prompts.set_mode(AgentMode::Fix, Some("do the fix".into()));
        assert_eq!(prompts.for_mode(AgentMode::Fix), Some("do the fix"));

        // Empty text clears the override (reset to default).
        prompts.set_mode(AgentMode::Fix, Some(String::new()));
        assert_eq!(prompts.for_mode(AgentMode::Fix), None);

        prompts.set_mode(AgentMode::Plan, Some("plan text".into()));
        prompts.set_mode(AgentMode::Plan, None);
        assert_eq!(prompts.for_mode(AgentMode::Plan), None);
    }

    #[test]
    fn plan_file_path_uses_plans_dir_and_slug() {
        temp_env::with_var("COCKPIT_HOME", Some("/tmp/cockpit-home-test"), || {
            let path = plan_file_path("proj-1").expect("should resolve");
            assert_eq!(
                path,
                PathBuf::from("/tmp/cockpit-home-test/plans/proj-1.md")
            );
        });
    }

    #[test]
    fn plan_file_path_sanitizes_hostile_ids() {
        temp_env::with_var("COCKPIT_HOME", Some("/tmp/cockpit-home-test"), || {
            // Slashes and spaces become '-' so the id yields one safe filename.
            let path = plan_file_path("acme/team plan").expect("should resolve");
            assert_eq!(
                path,
                PathBuf::from("/tmp/cockpit-home-test/plans/acme-team-plan.md")
            );
        });
    }

    #[test]
    fn plan_file_path_falls_back_for_empty_slug() {
        temp_env::with_var("COCKPIT_HOME", Some("/tmp/cockpit-home-test"), || {
            let path = plan_file_path("///").expect("should resolve");
            assert_eq!(path, PathBuf::from("/tmp/cockpit-home-test/plans/plan.md"));
        });
    }

    #[test]
    fn findings_file_path_uses_findings_dir_and_slug() {
        temp_env::with_var("COCKPIT_HOME", Some("/tmp/cockpit-home-test"), || {
            let path = findings_file_path("PR-1").expect("should resolve");
            assert_eq!(
                path,
                PathBuf::from("/tmp/cockpit-home-test/findings/PR-1.json")
            );
        });
    }

    #[test]
    fn findings_file_path_sanitizes_hostile_ids() {
        temp_env::with_var("COCKPIT_HOME", Some("/tmp/cockpit-home-test"), || {
            // A `owner/repo#42` PR ref must collapse to one safe filename.
            let path = findings_file_path("owner/repo#42").expect("should resolve");
            assert_eq!(
                path,
                PathBuf::from("/tmp/cockpit-home-test/findings/owner-repo-42.json")
            );
        });
    }

    #[test]
    fn findings_file_path_falls_back_for_empty_slug() {
        temp_env::with_var("COCKPIT_HOME", Some("/tmp/cockpit-home-test"), || {
            let path = findings_file_path("###").expect("should resolve");
            assert_eq!(
                path,
                PathBuf::from("/tmp/cockpit-home-test/findings/findings.json")
            );
        });
    }

    #[test]
    fn written_plan_file_parses_into_doc() {
        // End-to-end for the ingestion convention: a planner-written markdown
        // file at the resolved plan path parses into a PlanDoc with the
        // expected steps and files.
        let dir = tempfile::tempdir().expect("temp dir");
        temp_env::with_var("COCKPIT_HOME", Some(dir.path().as_os_str()), || {
            let path = plan_file_path("proj-1").expect("resolve path");
            std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");

            let raw = "\
# Plan: Ingested plan

## Steps

1. First step
   Do the first thing.

2. Second step
   Do the second thing.

## Files

- src/a.rs
- src/b.rs

## Risks

- A migration is needed
";
            std::fs::write(&path, raw).expect("write plan file");

            let read = std::fs::read_to_string(&path).expect("read plan file");
            let doc = crate::plan_parser::parse(&read).expect("parse plan file");

            assert_eq!(doc.summary, "Ingested plan");
            assert_eq!(doc.steps.len(), 2);
            assert_eq!(doc.steps[0].title, "First step");
            assert_eq!(doc.steps[1].title, "Second step");
            assert_eq!(
                doc.files,
                vec![PathBuf::from("src/a.rs"), PathBuf::from("src/b.rs")]
            );
            assert_eq!(doc.risks, vec!["A migration is needed".to_owned()]);
        });
    }

    #[test]
    fn lsp_servers_default_enabled_with_no_overrides() {
        let lsp = LspServers::default();
        assert!(lsp.enabled);
        assert!(lsp.pyright_command.is_none());
        assert!(lsp.typescript_command.is_none());
    }

    #[test]
    fn lsp_command_for_falls_back_to_defaults() {
        let lsp = LspServers::default();
        assert_eq!(lsp.command_for(LspLanguage::Python), "pyright-langserver");
        assert_eq!(
            lsp.command_for(LspLanguage::TypeScript),
            "typescript-language-server"
        );
    }

    #[test]
    fn lsp_command_for_honors_overrides() {
        let lsp = LspServers {
            enabled: true,
            pyright_command: Some("/opt/pyright".into()),
            typescript_command: Some("tsserver-wrap".into()),
        };
        assert_eq!(lsp.command_for(LspLanguage::Python), "/opt/pyright");
        assert_eq!(lsp.command_for(LspLanguage::TypeScript), "tsserver-wrap");
    }

    #[test]
    fn lsp_language_from_language_id() {
        assert_eq!(
            LspLanguage::from_language_id("python"),
            Some(LspLanguage::Python)
        );
        assert_eq!(
            LspLanguage::from_language_id("typescript"),
            Some(LspLanguage::TypeScript)
        );
        assert_eq!(
            LspLanguage::from_language_id("javascript"),
            Some(LspLanguage::TypeScript)
        );
        assert_eq!(LspLanguage::from_language_id("rust"), None);
        assert_eq!(LspLanguage::from_language_id("plaintext"), None);
    }

    #[test]
    fn lsp_servers_absent_defaults_enabled() {
        // A config file without an [lsp_servers] table must still parse and
        // default to enabled=true with no command overrides (migration).
        let toml_str = r#"
agent_command = "claude"
hook_port = 19876
"#;
        let config: Config = toml::from_str(toml_str).expect("should parse");
        assert!(config.lsp_servers.enabled);
        assert!(config.lsp_servers.pyright_command.is_none());
        assert!(config.lsp_servers.typescript_command.is_none());
    }

    #[test]
    fn lsp_servers_round_trips_toml() {
        let lsp = LspServers {
            enabled: false,
            pyright_command: Some("pyright-langserver".into()),
            typescript_command: Some("typescript-language-server".into()),
        };
        let serialized = toml::to_string_pretty(&lsp).expect("serialize");
        let deserialized: LspServers = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(deserialized, lsp);
    }

    #[test]
    fn agent_prompts_round_trips_toml() {
        let prompts = AgentPrompts {
            implement: Some("impl preamble".into()),
            plan: Some("plan preamble".into()),
            fix: None,
            restack: Some("restack preamble".into()),
            review: Some("review preamble".into()),
        };
        let serialized = toml::to_string_pretty(&prompts).expect("serialize");
        let deserialized: AgentPrompts = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(deserialized.implement, prompts.implement);
        assert_eq!(deserialized.plan, prompts.plan);
        assert_eq!(deserialized.fix, prompts.fix);
        assert_eq!(deserialized.restack, prompts.restack);
        assert_eq!(deserialized.review, prompts.review);
    }
}
