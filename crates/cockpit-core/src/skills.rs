//! Discovery, parsing, and filtering of project skill files.
//!
//! Skills are markdown files with YAML frontmatter that encode project
//! conventions for rework agents. They are injected into the rework prompt
//! so agents follow project-specific patterns.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::config;

/// Filename for a skill's markdown definition inside its directory.
const SKILL_FILE: &str = "SKILL.md";
/// Filename for a skill's provenance sidecar inside its directory.
const META_FILE: &str = ".meta.json";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A parsed skill definition loaded from a markdown file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct Skill {
    /// Short identifier from frontmatter.
    pub name: String,
    /// One-line description from frontmatter.
    pub description: String,
    /// Optional tags for filtering by relevance (e.g. `["rust", "testing"]`).
    pub tags: Vec<String>,
    /// The full markdown body (after frontmatter).
    pub body: String,
    /// Source file path.
    pub path: PathBuf,
    /// Provenance of the skill (local vs. GitHub), read from the `.meta.json`
    /// sidecar. Defaults to [`SkillSource::Local`] when no sidecar exists so
    /// hand-authored skills surface a Local badge in the UI.
    pub source: SkillSource,
}

/// Where a locally-installed skill came from.
///
/// Recorded in each skill's `.meta.json` sidecar so sync can decide whether a
/// remote update should overwrite a local edit and so the UI can label origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
#[serde(tag = "kind")]
pub enum SkillSource {
    /// Authored locally; never overwritten by GitHub sync.
    Local,
    /// Installed/synced from a GitHub repository via the `gh` CLI.
    GitHub {
        /// Repository owner (user or org).
        owner: String,
        /// Repository name.
        repo: String,
        /// The blob SHA of the `SKILL.md` last fetched, for idempotency.
        sha: String,
    },
}

/// Provenance sidecar written next to each skill's `SKILL.md`.
///
/// Serialized to `.meta.json`. Kept minimal and stable so it round-trips
/// cleanly and older files stay readable.
//
// No `#[derive(TS)]`: this is an on-disk sidecar, never sent to the frontend
// (the UI reads provenance via `Skill.source`), so a binding would be orphan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillMeta {
    /// Where this skill came from.
    pub source: SkillSource,
}

/// Outcome counts from a [`sync_from_github`] run.
///
/// Serializable so the CLI and Tauri surfaces can report progress without
/// re-deriving the numbers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct SyncReport {
    /// Skills newly written (no prior local copy).
    pub installed: usize,
    /// Skills whose SHA changed and were overwritten.
    pub updated: usize,
    /// Skills skipped because the stored SHA already matched (idempotent).
    pub skipped: usize,
}

/// YAML frontmatter deserialized from the top of a skill file.
#[derive(Debug, Deserialize)]
struct Frontmatter {
    /// Short identifier.
    name: String,
    /// One-line description.
    description: String,
    /// Optional tags; defaults to empty.
    #[serde(default)]
    tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors from skill operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Failed to read the skills directory or a skill file.
    #[error("failed to read skills directory: {0}")]
    ReadDir(#[from] std::io::Error),

    /// Failed to parse YAML frontmatter in a skill file.
    #[error("failed to parse skill frontmatter in {path}: {reason}")]
    ParseFrontmatter {
        /// Path of the file that failed to parse.
        path: PathBuf,
        /// Human-readable explanation.
        reason: String,
    },

    /// Could not resolve the cockpit skills directory.
    #[error("could not resolve skills directory: {0}")]
    Config(#[from] config::Error),

    /// The `gh` CLI could not be run (missing binary, etc.).
    #[error("failed to run `gh`: {0}")]
    GhSpawn(std::io::Error),

    /// The `gh` CLI exited non-zero (e.g. auth or network failure).
    #[error("`gh {args}` failed: {stderr}")]
    GhFailed {
        /// The gh subcommand/args that were run, for diagnostics.
        args: String,
        /// Captured stderr from the failed invocation.
        stderr: String,
    },

    /// A `gh api` JSON response could not be parsed as expected.
    #[error("failed to parse `gh api` response: {0}")]
    GhParse(String),

    /// Failed to write a skill or its sidecar to disk.
    #[error("failed to write skill {name}: {source}")]
    Write {
        /// Skill name being written.
        name: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },
}

// ---------------------------------------------------------------------------
// Discovery + parsing
// ---------------------------------------------------------------------------

/// Return the on-disk skills directory (`<cockpit_home>/skills`).
///
/// This is the canonical install location: each installed skill lives at
/// `skills_dir()/<name>/SKILL.md` with a `.meta.json` provenance sidecar.
pub fn skills_dir() -> Result<PathBuf, Error> {
    Ok(config::cockpit_home()?.join("skills"))
}

/// Discover and parse all installed skills under [`skills_dir`].
///
/// A convenience wrapper over [`discover_skills`] that reads the default
/// location. Callers on the review path should use this so skills actually
/// reach the prompt; a discovery failure is theirs to treat as non-fatal
/// (Invariant §0.1 — never block the loop on skills).
pub fn discover_installed_skills() -> Result<Vec<Skill>, Error> {
    let dir = skills_dir()?;
    let mut skills = discover_skills(&dir)?;
    // Overlay each skill's real provenance from its `.meta.json` sidecar so the
    // UI can badge Local vs. GitHub. A missing/unreadable sidecar leaves the
    // `Local` default from `parse_skill_file` (never a hard error, §0.1).
    for skill in &mut skills {
        if let Ok(Some(meta)) = read_meta(&skill.name) {
            skill.source = meta.source;
        }
    }
    Ok(skills)
}

/// Discover and parse all skills in the given directory.
///
/// Supports two layouts, so a plain fixture directory and the installed
/// `<name>/SKILL.md` layout both work:
/// - flat `*.md` files directly under `dir`, and
/// - subdirectories each containing a `SKILL.md`.
///
/// Skills with YAML frontmatter delimited by `---` are parsed; entries that
/// fail to parse are skipped (they do not halt discovery of the rest). Results
/// are sorted by name for deterministic ordering.
pub fn discover_skills(dir: &Path) -> Result<Vec<Skill>, Error> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let entries = std::fs::read_dir(dir)?;

    let mut skills = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        let candidate = if path.is_dir() {
            // Installed layout: <name>/SKILL.md.
            let skill_file = path.join(SKILL_FILE);
            if skill_file.is_file() {
                Some(skill_file)
            } else {
                None
            }
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            // Flat layout: a bare *.md file (used by fixtures/tests).
            Some(path)
        } else {
            None
        };

        let Some(candidate) = candidate else { continue };

        match parse_skill_file(&candidate) {
            Ok(skill) => skills.push(skill),
            Err(_e) => {
                // Skip files that fail to parse; callers can audit the
                // directory separately if needed.
                continue;
            }
        }
    }

    // Sort by name for deterministic ordering.
    skills.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(skills)
}

/// Parse a single skill file into a [`Skill`].
fn parse_skill_file(path: &Path) -> Result<Skill, Error> {
    let content = std::fs::read_to_string(path).map_err(|e| Error::ParseFrontmatter {
        path: path.to_path_buf(),
        reason: format!("could not read file: {e}"),
    })?;

    let (frontmatter, body) =
        split_frontmatter(&content).ok_or_else(|| Error::ParseFrontmatter {
            path: path.to_path_buf(),
            reason: "missing YAML frontmatter delimiters (---)".into(),
        })?;

    let fm: Frontmatter =
        serde_json::from_str(&yaml_to_json_minimal(&frontmatter)).map_err(|e| {
            Error::ParseFrontmatter {
                path: path.to_path_buf(),
                reason: format!("invalid frontmatter: {e}"),
            }
        })?;

    Ok(Skill {
        name: fm.name,
        description: fm.description,
        tags: fm.tags,
        body: body.trim().to_owned(),
        path: path.to_path_buf(),
        // Default provenance; `discover_installed_skills` overlays the real
        // source from the `.meta.json` sidecar where one exists.
        source: SkillSource::Local,
    })
}

/// Split content into (frontmatter, body) by finding `---` delimiters.
///
/// Expects the file to start with `---`, followed by YAML, followed by `---`,
/// then the body.
fn split_frontmatter(content: &str) -> Option<(String, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }

    // Skip the opening "---" line.
    let after_open = &trimmed[3..];
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);

    // Find the closing "---".
    let close_idx = after_open.find("\n---")?;
    let frontmatter = after_open[..close_idx].to_owned();
    let body = &after_open[close_idx + 4..]; // skip "\n---"
    let body = body.strip_prefix('\n').unwrap_or(body);

    Some((frontmatter, body.to_owned()))
}

/// Minimal YAML-to-JSON conversion for simple flat frontmatter.
///
/// Handles `key: value` pairs and `key:` followed by `- item` list entries.
/// This avoids pulling in a full YAML parser dependency for the simple
/// frontmatter format used by skill files.
fn yaml_to_json_minimal(yaml: &str) -> String {
    let mut result = String::from("{");
    let mut first_key = true;
    let lines: Vec<&str> = yaml.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() || line.starts_with('#') {
            i += 1;
            continue;
        }

        if let Some(colon_pos) = line.find(':') {
            let key = line[..colon_pos].trim();
            let value_part = line[colon_pos + 1..].trim();

            if !first_key {
                result.push(',');
            }
            first_key = false;

            // INVARIANT: write! into String is infallible.
            write!(result, "\"{key}\":").unwrap();

            if value_part.is_empty() {
                // Could be a list — check next lines for "- item" entries.
                let mut items: Vec<String> = Vec::new();
                i += 1;
                while i < lines.len() {
                    let next = lines[i].trim();
                    if let Some(item) = next.strip_prefix("- ") {
                        items.push(format!("\"{}\"", escape_json(item.trim())));
                        i += 1;
                    } else {
                        break;
                    }
                }
                if items.is_empty() {
                    result.push_str("\"\"");
                } else {
                    // INVARIANT: write! into String is infallible.
                    write!(result, "[{}]", items.join(",")).unwrap();
                }
                continue;
            }

            // Strip surrounding quotes if present.
            let val = value_part
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(value_part);
            // INVARIANT: write! into String is infallible.
            write!(result, "\"{}\"", escape_json(val)).unwrap();
        }

        i += 1;
    }

    result.push('}');
    result
}

/// Escape special characters for JSON string values.
fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

// ---------------------------------------------------------------------------
// Meta sidecar (provenance)
// ---------------------------------------------------------------------------

/// Return the directory that holds a named skill (`skills_dir()/<name>`).
fn skill_dir(name: &str) -> Result<PathBuf, Error> {
    Ok(skills_dir()?.join(name))
}

/// Read a skill's `.meta.json` sidecar, if present and parseable.
///
/// Returns `None` when the sidecar is missing or cannot be parsed — callers
/// treat a missing/invalid sidecar as "unknown provenance" (e.g. sync will
/// then reinstall), never as a hard error.
pub fn read_meta(name: &str) -> Result<Option<SkillMeta>, Error> {
    let path = skill_dir(name)?.join(META_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&raw).ok())
}

/// Write a skill's `.meta.json` sidecar, creating its directory if needed.
pub fn write_meta(name: &str, meta: &SkillMeta) -> Result<(), Error> {
    let dir = skill_dir(name)?;
    std::fs::create_dir_all(&dir).map_err(|source| Error::Write {
        name: name.to_owned(),
        source,
    })?;
    // INVARIANT: SkillMeta is a plain struct with String/enum fields; serde_json
    // serialization is infallible for it.
    let json = serde_json::to_string_pretty(meta).map_err(|e| Error::GhParse(e.to_string()))?;
    std::fs::write(dir.join(META_FILE), json).map_err(|source| Error::Write {
        name: name.to_owned(),
        source,
    })
}

// ---------------------------------------------------------------------------
// Install
// ---------------------------------------------------------------------------

/// Install (or overwrite) a skill on disk.
///
/// Writes `skills_dir()/<name>/SKILL.md` with the given contents and a
/// `.meta.json` sidecar recording `source`. Creates the skill directory if it
/// does not exist. Returns the path to the written `SKILL.md`.
pub fn install_skill(name: &str, contents: &str, source: SkillSource) -> Result<PathBuf, Error> {
    let dir = skill_dir(name)?;
    std::fs::create_dir_all(&dir).map_err(|src| Error::Write {
        name: name.to_owned(),
        source: src,
    })?;

    let skill_path = dir.join(SKILL_FILE);
    std::fs::write(&skill_path, contents).map_err(|src| Error::Write {
        name: name.to_owned(),
        source: src,
    })?;

    write_meta(name, &SkillMeta { source })?;
    Ok(skill_path)
}

/// Delete an installed skill directory (SKILL.md + sidecar) by name.
///
/// A no-op if the directory does not exist. This is an explicit user action.
pub fn delete_skill(name: &str) -> Result<(), Error> {
    let dir = skill_dir(name)?;
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|source| Error::Write {
            name: name.to_owned(),
            source,
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// GitHub sync (via the `gh` CLI)
// ---------------------------------------------------------------------------

/// One entry from a `gh api .../contents/<path>` directory listing.
///
/// Only the fields sync needs are modeled; unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct GhContentEntry {
    /// Entry base name (e.g. `SKILL.md` or a skill directory name).
    name: String,
    /// `"file"` or `"dir"`.
    #[serde(rename = "type")]
    kind: String,
    /// Git blob SHA (present for files); used for idempotency.
    #[serde(default)]
    sha: String,
    /// Repo-relative path of the entry.
    path: String,
}

/// Sync skills from a GitHub repository into [`skills_dir`] via the `gh` CLI.
///
/// Uses the user's existing `gh auth` (never a PAT — consistent with removing
/// `github_token`). Lists `path` in `owner/repo@branch`; for every entry that
/// is a skill directory containing a `SKILL.md`, fetches that file and installs
/// it, skipping any whose stored `.meta.json` SHA already matches the remote
/// blob SHA (idempotency). A skill previously marked [`SkillSource::Local`] is
/// left untouched so sync never clobbers a local edit.
///
/// Returns a [`SyncReport`] with installed/updated/skipped counts.
pub async fn sync_from_github(
    owner: &str,
    repo: &str,
    branch: &str,
    path: &str,
) -> Result<SyncReport, Error> {
    let listing = gh_list_contents(owner, repo, branch, path).await?;
    let mut report = SyncReport::default();

    for entry in listing {
        if entry.kind != "dir" {
            continue;
        }
        let name = entry.name.clone();
        let skill_md_path = format!("{}/{SKILL_FILE}", entry.path);

        // Fetch the SKILL.md metadata (for its SHA) then contents.
        let Some(file) = gh_file_entry(owner, repo, branch, &skill_md_path).await? else {
            // No SKILL.md in this directory — not a skill; skip.
            continue;
        };

        match classify_sync(&name, &file.sha)? {
            SyncAction::Skip => report.skipped += 1,
            SyncAction::LocalKeep => report.skipped += 1,
            action @ (SyncAction::Install | SyncAction::Update) => {
                let contents = gh_fetch_file(owner, repo, branch, &skill_md_path).await?;
                install_skill(
                    &name,
                    &contents,
                    SkillSource::GitHub {
                        owner: owner.to_owned(),
                        repo: repo.to_owned(),
                        sha: file.sha.clone(),
                    },
                )?;
                match action {
                    SyncAction::Install => report.installed += 1,
                    SyncAction::Update => report.updated += 1,
                    // Unreachable: outer match already narrowed the variants.
                    SyncAction::Skip | SyncAction::LocalKeep => {}
                }
            }
        }
    }

    Ok(report)
}

/// What to do with a single remote skill during sync.
#[derive(Debug, PartialEq, Eq)]
enum SyncAction {
    /// No local copy — write it fresh.
    Install,
    /// Local copy exists with a different GitHub SHA — overwrite.
    Update,
    /// Stored GitHub SHA already matches — idempotent skip.
    Skip,
    /// Local-origin skill — never overwritten by sync.
    LocalKeep,
}

/// Decide the sync action for `name` given the remote blob `sha`.
///
/// Reads the local `.meta.json` (if any) and compares provenance/SHA. This is
/// the pure idempotency core, tested directly against on-disk fixtures.
fn classify_sync(name: &str, remote_sha: &str) -> Result<SyncAction, Error> {
    match read_meta(name)? {
        None => {
            // Directory may exist without a sidecar (hand-authored). If a
            // SKILL.md is present treat it as a local skill to keep; otherwise
            // it's a fresh install.
            let has_local = skill_dir(name)?.join(SKILL_FILE).is_file();
            Ok(if has_local {
                SyncAction::LocalKeep
            } else {
                SyncAction::Install
            })
        }
        Some(meta) => match meta.source {
            SkillSource::Local => Ok(SyncAction::LocalKeep),
            SkillSource::GitHub { sha, .. } => Ok(if sha == remote_sha {
                SyncAction::Skip
            } else {
                SyncAction::Update
            }),
        },
    }
}

/// List a directory's contents in a repo via `gh api`.
async fn gh_list_contents(
    owner: &str,
    repo: &str,
    branch: &str,
    path: &str,
) -> Result<Vec<GhContentEntry>, Error> {
    let endpoint = format!("repos/{owner}/{repo}/contents/{path}?ref={branch}");
    let stdout = run_gh(&["api", &endpoint]).await?;
    serde_json::from_str(&stdout).map_err(|e| Error::GhParse(e.to_string()))
}

/// Fetch a single file's content-listing entry (for its SHA) via `gh api`.
///
/// Returns `None` when the path does not exist (a 404 from `gh`).
async fn gh_file_entry(
    owner: &str,
    repo: &str,
    branch: &str,
    path: &str,
) -> Result<Option<GhContentEntry>, Error> {
    let endpoint = format!("repos/{owner}/{repo}/contents/{path}?ref={branch}");
    match run_gh(&["api", &endpoint]).await {
        Ok(stdout) => serde_json::from_str(&stdout)
            .map(Some)
            .map_err(|e| Error::GhParse(e.to_string())),
        // A missing file is expected (directory without SKILL.md); treat as None.
        Err(Error::GhFailed { stderr, .. }) if stderr.contains("404") => Ok(None),
        Err(e) => Err(e),
    }
}

/// Fetch a file's raw contents via `gh api` with the raw media type.
async fn gh_fetch_file(owner: &str, repo: &str, branch: &str, path: &str) -> Result<String, Error> {
    let endpoint = format!("repos/{owner}/{repo}/contents/{path}?ref={branch}");
    run_gh(&["api", "-H", "Accept: application/vnd.github.raw", &endpoint]).await
}

/// Run `gh <args>` and return stdout, mapping failures to typed errors.
async fn run_gh(args: &[&str]) -> Result<String, Error> {
    let output = tokio::process::Command::new("gh")
        .args(args)
        .output()
        .await
        .map_err(Error::GhSpawn)?;

    if !output.status.success() {
        return Err(Error::GhFailed {
            args: args.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// ---------------------------------------------------------------------------
// Diff parsing
// ---------------------------------------------------------------------------

/// Extract the set of changed file paths from a unified diff.
///
/// Parses `+++ b/<path>` headers (the post-image path) and falls back to the
/// `diff --git a/<path> b/<path>` line's `b/` path so callers can drive
/// [`filter_relevant`] from a review's raw diff. `/dev/null` (deletions) is
/// skipped. Paths are returned de-duplicated in first-seen order.
pub fn changed_files_from_diff(diff: &str) -> Vec<String> {
    let mut files = Vec::new();

    for line in diff.lines() {
        let path = if let Some(rest) = line.strip_prefix("+++ ") {
            strip_diff_prefix(rest.trim())
        } else if let Some(rest) = line.strip_prefix("diff --git ") {
            // Format: `a/<path> b/<path>`; take the b-side.
            rest.split_whitespace()
                .nth(1)
                .and_then(|b| b.strip_prefix("b/").map(str::to_owned))
        } else {
            None
        };

        if let Some(path) = path {
            if path != "/dev/null" && !files.contains(&path) {
                files.push(path);
            }
        }
    }

    files
}

/// Strip a leading `a/` or `b/` marker from a diff path, if present.
fn strip_diff_prefix(path: &str) -> Option<String> {
    if path == "/dev/null" {
        return Some(path.to_owned());
    }
    Some(
        path.strip_prefix("a/")
            .or_else(|| path.strip_prefix("b/"))
            .unwrap_or(path)
            .to_owned(),
    )
}

// ---------------------------------------------------------------------------
// Filtering
// ---------------------------------------------------------------------------

/// Filter skills by relevance to a set of changed file paths.
///
/// A skill is relevant if:
/// - It has no tags (universal convention), or
/// - Any of its tags match a file extension or directory name in the changed files.
pub fn filter_relevant<'a>(skills: &'a [Skill], changed_files: &[&str]) -> Vec<&'a Skill> {
    skills
        .iter()
        .filter(|skill| {
            // Skills with no tags are universal conventions — always include.
            if skill.tags.is_empty() {
                return true;
            }

            // Check if any tag matches an extension or path segment.
            skill.tags.iter().any(|tag| {
                let tag_lower = tag.to_lowercase();
                changed_files.iter().any(|file| {
                    let path = Path::new(file);

                    // Match against file extension (e.g. tag "rust" matches ".rs" if
                    // we also check the extension directly).
                    let ext_match = path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case(&tag_lower));

                    // Match against path segments (directory names, file stems).
                    let segment_match = path
                        .components()
                        .any(|c| c.as_os_str().eq_ignore_ascii_case(tag_lower.as_str()));

                    ext_match || segment_match
                })
            })
        })
        .collect()
}

/// Discover installed skills relevant to a diff, tolerating any failure.
///
/// Convenience for the review path: reads [`skills_dir`], parses the changed
/// files out of `diff`, filters to the relevant subset, and returns owned
/// clones ready to hand to a prompt input. On **any** discovery error it
/// returns an empty vec rather than propagating — skills must never block the
/// loop (Invariant §0.1). Pass `""` for a diff to get only universal
/// (untagged) skills.
pub fn relevant_for_diff(diff: &str) -> Vec<Skill> {
    let all = match discover_installed_skills() {
        Ok(all) => all,
        Err(_) => return Vec::new(),
    };
    let changed = changed_files_from_diff(diff);
    let changed_refs: Vec<&str> = changed.iter().map(String::as_str).collect();
    filter_relevant(&all, &changed_refs)
        .into_iter()
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Prompt formatting
// ---------------------------------------------------------------------------

/// Format skills into a prompt section for injection into the rework prompt.
///
/// Produces a `## Project Conventions` header followed by one subsection
/// per skill. Returns an empty string if no skills are provided.
pub fn format_for_prompt(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    // INVARIANT: all write!/writeln! target a String, whose fmt::Write is infallible.
    let mut out = String::new();
    writeln!(out, "## Project Conventions\n").unwrap();

    for skill in skills {
        writeln!(out, "### {}\n", skill.name).unwrap();
        writeln!(out, "{}\n", skill.body).unwrap();
    }

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- split_frontmatter ------------------------------------------------

    #[test]
    fn split_frontmatter_basic() {
        let content = "---\nname: test\n---\nBody content here.";
        let (fm, body) = split_frontmatter(content).expect("should parse");
        assert_eq!(fm, "name: test");
        assert_eq!(body, "Body content here.");
    }

    #[test]
    fn split_frontmatter_missing_opener() {
        let content = "name: test\n---\nBody";
        assert!(split_frontmatter(content).is_none());
    }

    #[test]
    fn split_frontmatter_missing_closer() {
        let content = "---\nname: test\nBody without closer";
        assert!(split_frontmatter(content).is_none());
    }

    // -- yaml_to_json_minimal ---------------------------------------------

    #[test]
    fn yaml_simple_kv() {
        let yaml = "name: my-skill\ndescription: A test skill";
        let json = yaml_to_json_minimal(yaml);
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(v["name"], "my-skill");
        assert_eq!(v["description"], "A test skill");
    }

    #[test]
    fn yaml_list() {
        let yaml = "name: test\ntags:\n- rust\n- testing";
        let json = yaml_to_json_minimal(yaml);
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(v["tags"], serde_json::json!(["rust", "testing"]));
    }

    #[test]
    fn yaml_empty_list_becomes_empty_string() {
        let yaml = "name: test\ndescription:";
        let json = yaml_to_json_minimal(yaml);
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(v["description"], "");
    }

    // -- discover_skills --------------------------------------------------

    #[test]
    fn discover_skills_from_dir() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Write two valid skill files.
        std::fs::write(
            dir.path().join("alpha.md"),
            "---\nname: alpha\ndescription: First skill\ntags:\n- rust\n---\nAlpha body.",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("beta.md"),
            "---\nname: beta\ndescription: Second skill\n---\nBeta body.",
        )
        .unwrap();

        // Write a non-md file (should be ignored).
        std::fs::write(dir.path().join("readme.txt"), "not a skill").unwrap();

        let skills = discover_skills(dir.path()).expect("should discover");
        assert_eq!(skills.len(), 2);

        // Sorted by name.
        assert_eq!(skills[0].name, "alpha");
        assert_eq!(skills[0].description, "First skill");
        assert_eq!(skills[0].tags, vec!["rust"]);
        assert_eq!(skills[0].body, "Alpha body.");

        assert_eq!(skills[1].name, "beta");
        assert_eq!(skills[1].description, "Second skill");
        assert!(skills[1].tags.is_empty());
        assert_eq!(skills[1].body, "Beta body.");
    }

    #[test]
    fn discover_skills_skips_bad_files() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Valid skill.
        std::fs::write(
            dir.path().join("good.md"),
            "---\nname: good\ndescription: works\n---\nContent.",
        )
        .unwrap();

        // Invalid skill (no frontmatter).
        std::fs::write(dir.path().join("bad.md"), "No frontmatter here.").unwrap();

        let skills = discover_skills(dir.path()).expect("should not fail");
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "good");
    }

    #[test]
    fn discover_skills_nonexistent_dir() {
        let path = Path::new("/nonexistent/skills/dir");
        let skills = discover_skills(path).expect("should return empty");
        assert!(skills.is_empty());
    }

    // -- filter_relevant --------------------------------------------------

    #[test]
    fn filter_universal_skills_always_included() {
        let skills = vec![Skill {
            name: "universal".into(),
            description: "always applies".into(),
            tags: vec![],
            body: "Universal rule.".into(),
            path: PathBuf::from("universal.md"),
            source: SkillSource::Local,
        }];

        let result = filter_relevant(&skills, &["anything.py"]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "universal");
    }

    #[test]
    fn filter_by_extension() {
        let skills = vec![
            Skill {
                name: "rust-conventions".into(),
                description: "Rust rules".into(),
                tags: vec!["rs".into()],
                body: "Use thiserror.".into(),
                path: PathBuf::from("rust.md"),
                source: SkillSource::Local,
            },
            Skill {
                name: "python-conventions".into(),
                description: "Python rules".into(),
                tags: vec!["py".into()],
                body: "Use ruff.".into(),
                path: PathBuf::from("python.md"),
                source: SkillSource::Local,
            },
        ];

        let result = filter_relevant(&skills, &["src/main.rs", "src/lib.rs"]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "rust-conventions");
    }

    #[test]
    fn filter_by_directory_name() {
        let skills = vec![Skill {
            name: "test-conventions".into(),
            description: "Testing rules".into(),
            tags: vec!["tests".into()],
            body: "Write integration tests.".into(),
            path: PathBuf::from("testing.md"),
            source: SkillSource::Local,
        }];

        let result = filter_relevant(&skills, &["tests/integration.rs"]);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn filter_excludes_irrelevant() {
        let skills = vec![Skill {
            name: "go-conventions".into(),
            description: "Go rules".into(),
            tags: vec!["go".into()],
            body: "Use gofmt.".into(),
            path: PathBuf::from("go.md"),
            source: SkillSource::Local,
        }];

        let result = filter_relevant(&skills, &["src/main.rs"]);
        assert!(result.is_empty());
    }

    // -- format_for_prompt ------------------------------------------------

    #[test]
    fn format_empty_returns_empty() {
        assert!(format_for_prompt(&[]).is_empty());
    }

    #[test]
    fn format_single_skill() {
        let skills = vec![Skill {
            name: "naming".into(),
            description: "Naming conventions".into(),
            tags: vec![],
            body: "Use snake_case for functions.".into(),
            path: PathBuf::from("naming.md"),
            source: SkillSource::Local,
        }];

        let result = format_for_prompt(&skills);
        assert!(result.contains("## Project Conventions"));
        assert!(result.contains("### naming"));
        assert!(result.contains("Use snake_case for functions."));
    }

    #[test]
    fn format_multiple_skills() {
        let skills = vec![
            Skill {
                name: "errors".into(),
                description: "Error handling".into(),
                tags: vec![],
                body: "Use thiserror in core.".into(),
                path: PathBuf::from("errors.md"),
                source: SkillSource::Local,
            },
            Skill {
                name: "testing".into(),
                description: "Test conventions".into(),
                tags: vec![],
                body: "Never weaken tests.".into(),
                path: PathBuf::from("testing.md"),
                source: SkillSource::Local,
            },
        ];

        let result = format_for_prompt(&skills);
        assert!(result.contains("### errors"));
        assert!(result.contains("### testing"));

        // Verify ordering is preserved.
        let errors_pos = result.find("### errors").expect("should contain errors");
        let testing_pos = result.find("### testing").expect("should contain testing");
        assert!(
            errors_pos < testing_pos,
            "skills should appear in input order"
        );
    }

    // -- changed_files_from_diff ------------------------------------------

    #[test]
    fn changed_files_parses_unified_diff() {
        let diff = "\
diff --git a/src/main.rs b/src/main.rs
index e69de29..0d1d7fc 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -0,0 +1,2 @@
+fn main() {}
diff --git a/tests/foo.py b/tests/foo.py
--- a/tests/foo.py
+++ b/tests/foo.py
@@ -1 +1 @@
-x
+y";
        let files = changed_files_from_diff(diff);
        assert_eq!(files, vec!["src/main.rs", "tests/foo.py"]);
    }

    #[test]
    fn changed_files_skips_dev_null_deletion() {
        let diff = "\
diff --git a/gone.rs b/gone.rs
deleted file mode 100644
--- a/gone.rs
+++ /dev/null";
        let files = changed_files_from_diff(diff);
        // The `diff --git` b-side still names gone.rs; the +++ /dev/null is skipped.
        assert_eq!(files, vec!["gone.rs"]);
    }

    #[test]
    fn changed_files_empty_diff_is_empty() {
        assert!(changed_files_from_diff("").is_empty());
    }

    // -- install / meta round-trip ----------------------------------------

    #[test]
    fn install_writes_skill_and_meta() {
        let home = tempfile::tempdir().expect("tempdir");
        temp_env::with_var("COCKPIT_HOME", Some(home.path()), || {
            let path = install_skill(
                "errors",
                "---\nname: errors\ndescription: Error rules\n---\nUse thiserror.",
                SkillSource::Local,
            )
            .expect("install");

            assert!(path.ends_with("skills/errors/SKILL.md"));
            assert!(path.is_file());

            let meta = read_meta("errors").expect("read meta").expect("some meta");
            assert_eq!(meta.source, SkillSource::Local);

            // Discovery via the default location picks up the installed skill
            // and surfaces its provenance from the sidecar.
            let skills = discover_installed_skills().expect("discover");
            assert_eq!(skills.len(), 1);
            assert_eq!(skills[0].name, "errors");
            assert_eq!(skills[0].body, "Use thiserror.");
            assert_eq!(skills[0].source, SkillSource::Local);
        });
    }

    #[test]
    fn discovery_surfaces_github_source_from_sidecar() {
        let home = tempfile::tempdir().expect("tempdir");
        temp_env::with_var("COCKPIT_HOME", Some(home.path()), || {
            let source = SkillSource::GitHub {
                owner: "acme".into(),
                repo: "skills".into(),
                sha: "deadbeef".into(),
            };
            install_skill(
                "gh-skill",
                "---\nname: gh-skill\ndescription: d\n---\nbody",
                source.clone(),
            )
            .expect("install");

            let skills = discover_installed_skills().expect("discover");
            assert_eq!(skills.len(), 1);
            assert_eq!(
                skills[0].source, source,
                "GitHub provenance must be surfaced from the .meta.json sidecar"
            );
        });
    }

    #[test]
    fn discovery_defaults_source_to_local_without_sidecar() {
        let home = tempfile::tempdir().expect("tempdir");
        temp_env::with_var("COCKPIT_HOME", Some(home.path()), || {
            // Hand-authored skill directory with no .meta.json sidecar.
            let dir = skill_dir("hand").expect("dir");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(SKILL_FILE),
                "---\nname: hand\ndescription: d\n---\nb",
            )
            .unwrap();

            let skills = discover_installed_skills().expect("discover");
            assert_eq!(skills.len(), 1);
            assert_eq!(
                skills[0].source,
                SkillSource::Local,
                "a sidecar-less skill defaults to Local provenance"
            );
        });
    }

    #[test]
    fn delete_removes_installed_skill() {
        let home = tempfile::tempdir().expect("tempdir");
        temp_env::with_var("COCKPIT_HOME", Some(home.path()), || {
            install_skill(
                "gone",
                "---\nname: gone\ndescription: d\n---\nbody",
                SkillSource::Local,
            )
            .expect("install");
            assert_eq!(discover_installed_skills().expect("discover").len(), 1);

            delete_skill("gone").expect("delete");
            assert!(discover_installed_skills().expect("discover").is_empty());

            // Deleting a missing skill is a no-op.
            delete_skill("gone").expect("delete missing is ok");
        });
    }

    // -- sync idempotency (classify_sync) ---------------------------------

    #[test]
    fn classify_sync_install_when_absent() {
        let home = tempfile::tempdir().expect("tempdir");
        temp_env::with_var("COCKPIT_HOME", Some(home.path()), || {
            assert_eq!(
                classify_sync("new", "sha1").expect("classify"),
                SyncAction::Install
            );
        });
    }

    #[test]
    fn classify_sync_skips_matching_sha() {
        let home = tempfile::tempdir().expect("tempdir");
        temp_env::with_var("COCKPIT_HOME", Some(home.path()), || {
            install_skill(
                "gh-skill",
                "---\nname: gh-skill\ndescription: d\n---\nbody",
                SkillSource::GitHub {
                    owner: "o".into(),
                    repo: "r".into(),
                    sha: "abc123".into(),
                },
            )
            .expect("install");

            assert_eq!(
                classify_sync("gh-skill", "abc123").expect("classify"),
                SyncAction::Skip,
                "matching SHA must be idempotent skip"
            );
            assert_eq!(
                classify_sync("gh-skill", "def456").expect("classify"),
                SyncAction::Update,
                "changed SHA must update"
            );
        });
    }

    #[test]
    fn classify_sync_keeps_local_authored() {
        let home = tempfile::tempdir().expect("tempdir");
        temp_env::with_var("COCKPIT_HOME", Some(home.path()), || {
            install_skill(
                "mine",
                "---\nname: mine\ndescription: d\n---\nbody",
                SkillSource::Local,
            )
            .expect("install");
            assert_eq!(
                classify_sync("mine", "any-sha").expect("classify"),
                SyncAction::LocalKeep,
                "local-authored skills are never overwritten by sync"
            );
        });
    }

    #[test]
    fn classify_sync_keeps_sidecarless_local() {
        let home = tempfile::tempdir().expect("tempdir");
        temp_env::with_var("COCKPIT_HOME", Some(home.path()), || {
            // Hand-authored SKILL.md with no .meta.json sidecar.
            let dir = skill_dir("hand").expect("dir");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(SKILL_FILE),
                "---\nname: hand\ndescription: d\n---\nb",
            )
            .unwrap();

            assert_eq!(
                classify_sync("hand", "sha").expect("classify"),
                SyncAction::LocalKeep
            );
        });
    }
}
