//! Discovery, parsing, and filtering of project skill files.
//!
//! Skills are markdown files with YAML frontmatter that encode project
//! conventions for rework agents. They are injected into the rework prompt
//! so agents follow project-specific patterns.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A parsed skill definition loaded from a markdown file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

// ---------------------------------------------------------------------------
// Discovery + parsing
// ---------------------------------------------------------------------------

/// Discover and parse all skill files in the given directory.
///
/// Looks for `*.md` files with YAML frontmatter delimited by `---`.
/// Files that fail to parse are skipped (the error is collected but does not
/// halt discovery of remaining files).
pub fn discover_skills(dir: &Path) -> Result<Vec<Skill>, Error> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let entries = std::fs::read_dir(dir)?;

    let mut skills = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        // Only process .md files.
        let is_md = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if !is_md {
            continue;
        }

        match parse_skill_file(&path) {
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
                    write!(result, "[{}]", items.join(",")).unwrap();
                }
                continue;
            }

            // Strip surrounding quotes if present.
            let val = value_part
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(value_part);
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
            },
            Skill {
                name: "python-conventions".into(),
                description: "Python rules".into(),
                tags: vec!["py".into()],
                body: "Use ruff.".into(),
                path: PathBuf::from("python.md"),
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
            },
            Skill {
                name: "testing".into(),
                description: "Test conventions".into(),
                tags: vec![],
                body: "Never weaken tests.".into(),
                path: PathBuf::from("testing.md"),
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
}
