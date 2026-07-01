//! Structured plan-document parser and renderer.
//!
//! Pins the format the planner subagent must produce and provides a round-trip
//! pair: [`parse`] (markdown → [`PlanDoc`]) and [`render`] ([`PlanDoc`] →
//! markdown). This resolves the open item in `SPEC.md` §16 about plan-doc
//! parsing.
//!
//! # Pinned format
//!
//! ```markdown
//! # Plan: <summary>
//!
//! ## Steps
//!
//! 1. <step title>
//!    <step description — can be multi-line>
//!
//! 2. <step title>
//!    <step description>
//!
//! ## Files
//!
//! - path/to/file1.rs
//! - path/to/file2.rs
//!
//! ## Risks
//!
//! - Risk description 1
//! - Risk description 2
//! ```

use std::fmt::Write;
use std::path::{Path, PathBuf};

use crate::gate::Gated;
use crate::model::{PlanDoc, PlanStep, ProjectPlan};

/// Errors from parsing a structured plan document.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A required section header is missing from the input.
    #[error("missing required section: {0}")]
    MissingSection(String),
    /// A step could not be parsed.
    #[error("failed to parse step at line {line}: {reason}")]
    InvalidStep {
        /// One-based line number where the problem was detected.
        line: usize,
        /// Human-readable explanation.
        reason: String,
    },
    /// The input was empty or contained only whitespace.
    #[error("plan document is empty")]
    Empty,
}

/// Parse a structured plan document into a [`PlanDoc`].
///
/// The input must follow the pinned format:
/// - `# Plan: <summary>` header
/// - `## Steps` section with numbered steps
/// - `## Files` section with bullet list of paths
/// - `## Risks` section with bullet list of risk descriptions
///
/// The `## Risks` section may be present but empty (produces an empty vec).
pub fn parse(raw: &str) -> Result<PlanDoc, Error> {
    if raw.trim().is_empty() {
        return Err(Error::Empty);
    }

    let summary = extract_summary(raw)?;
    let sections = split_sections(raw);

    let steps_body = sections
        .get("steps")
        .ok_or_else(|| Error::MissingSection("## Steps".into()))?;
    let steps = parse_steps(steps_body)?;

    let files_body = sections
        .get("files")
        .ok_or_else(|| Error::MissingSection("## Files".into()))?;
    let files = parse_bullet_list(files_body)
        .into_iter()
        .map(PathBuf::from)
        .collect();

    let risks = sections
        .get("risks")
        .map(|body| parse_bullet_list(body))
        .unwrap_or_default();

    Ok(PlanDoc {
        summary,
        steps,
        files,
        risks,
        raw: raw.to_owned(),
    })
}

/// Serialize a [`PlanDoc`] back to the pinned format.
///
/// Round-trips with [`parse`] for testing.
pub fn render(doc: &PlanDoc) -> String {
    // INVARIANT: write!/writeln! into String is infallible.
    let mut out = String::new();

    writeln!(out, "# Plan: {}", doc.summary).unwrap();
    writeln!(out).unwrap();
    writeln!(out, "## Steps").unwrap();

    for step in &doc.steps {
        writeln!(out).unwrap();
        writeln!(out, "{}. {}", step.index + 1, step.title).unwrap();
        if !step.description.is_empty() {
            for line in step.description.lines() {
                writeln!(out, "   {line}").unwrap();
            }
        }
    }

    writeln!(out).unwrap();
    writeln!(out, "## Files").unwrap();
    writeln!(out).unwrap();
    for file in &doc.files {
        writeln!(out, "- {}", file.display()).unwrap();
    }

    writeln!(out).unwrap();
    writeln!(out, "## Risks").unwrap();
    if !doc.risks.is_empty() {
        writeln!(out).unwrap();
        for risk in &doc.risks {
            writeln!(out, "- {risk}").unwrap();
        }
    }

    out
}

/// Parse a plan anchor string into an [`Anchor`].
///
/// Accepted formats:
/// - `"step:N"` — produces `Anchor::PlanStep(N)` (zero-based index).
/// - `"file:path/to/file"` — produces `Anchor::PlanFile(path)`.
pub fn parse_plan_anchor(s: &str) -> Result<crate::model::Anchor, Error> {
    if let Some(rest) = s.strip_prefix("step:") {
        let idx: usize = rest.parse().map_err(|_| Error::InvalidStep {
            line: 0,
            reason: format!("invalid step index: {rest:?}"),
        })?;
        Ok(crate::model::Anchor::PlanStep(idx))
    } else if let Some(rest) = s.strip_prefix("file:") {
        let path = rest.trim();
        if path.is_empty() {
            return Err(Error::InvalidStep {
                line: 0,
                reason: "empty file path in anchor".into(),
            });
        }
        Ok(crate::model::Anchor::PlanFile(PathBuf::from(path)))
    } else {
        Err(Error::InvalidStep {
            line: 0,
            reason: format!(
                "unrecognized anchor format: {s:?}; expected \"step:N\" or \"file:path\""
            ),
        })
    }
}

/// Reconcile a plan after the planner agent completes.
///
/// Re-reads and re-parses the plan document from the given path, updates the
/// `ProjectPlan`'s `doc` field with the fresh parse, and transitions the gate
/// to `Reworked` (which also clears ephemeral comments per Invariant 4).
///
/// This is the plan-gate counterpart of the diff-gate's git reconciliation.
pub fn reconcile_plan(plan: &mut ProjectPlan, plan_path: &Path) -> Result<(), ReconcileError> {
    let raw = std::fs::read_to_string(plan_path).map_err(|source| ReconcileError::ReadFailed {
        path: plan_path.to_path_buf(),
        source,
    })?;
    let doc = parse(&raw).map_err(ReconcileError::ParseFailed)?;
    plan.doc = doc;
    plan.mark_reworked()
        .map_err(ReconcileError::TransitionFailed)?;
    Ok(())
}

/// Errors from plan reconciliation.
#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    /// The plan file could not be read from disk.
    #[error("failed to read plan file at {path}: {source}")]
    ReadFailed {
        /// Path that was being read.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// The plan file could not be parsed after the agent's edits.
    #[error("failed to parse updated plan: {0}")]
    ParseFailed(Error),

    /// The gate state transition failed (e.g. plan was not in Dispatched state).
    #[error("gate transition failed: {0}")]
    TransitionFailed(crate::gate::Error),
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Extract the summary from the `# Plan: <summary>` header line.
fn extract_summary(raw: &str) -> Result<String, Error> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# Plan:") {
            let summary = rest.trim().to_owned();
            if summary.is_empty() {
                return Err(Error::MissingSection("# Plan: <summary>".into()));
            }
            return Ok(summary);
        }
    }
    Err(Error::MissingSection("# Plan:".into()))
}

/// Split the document into named sections keyed by the lowercased `## Header`
/// name. The value is all text between that header and the next `## ` header
/// (or end of document).
fn split_sections(raw: &str) -> std::collections::HashMap<String, String> {
    let mut sections = std::collections::HashMap::new();
    let mut current_key: Option<String> = None;
    let mut current_body = String::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(header) = trimmed.strip_prefix("## ") {
            // Flush previous section.
            if let Some(key) = current_key.take() {
                sections.insert(key, std::mem::take(&mut current_body));
            }
            current_key = Some(header.trim().to_lowercase());
            current_body.clear();
        } else if current_key.is_some() {
            current_body.push_str(line);
            current_body.push('\n');
        }
    }

    // Flush last section.
    if let Some(key) = current_key {
        sections.insert(key, current_body);
    }

    sections
}

/// Parse numbered steps from the `## Steps` section body.
///
/// Each step starts with a line matching `N. <title>` (digit(s), dot, space).
/// Subsequent non-step lines are appended to the description until the next
/// step or end of section.
fn parse_steps(body: &str) -> Result<Vec<PlanStep>, Error> {
    let mut steps: Vec<PlanStep> = Vec::new();
    let mut current_title: Option<String> = None;
    let mut current_desc = String::new();
    let mut current_index: usize = 0;
    let mut step_start_line: usize = 0;

    for (offset, line) in body.lines().enumerate() {
        let trimmed = line.trim();

        if let Some((num, title)) = try_parse_step_line(trimmed) {
            // Flush previous step.
            if let Some(title_text) = current_title.take() {
                steps.push(PlanStep {
                    index: current_index,
                    title: title_text,
                    description: current_desc.trim().to_owned(),
                });
                current_desc.clear();
            }

            current_index = num.saturating_sub(1); // 1-based → 0-based
            current_title = Some(title);
            step_start_line = offset + 1; // 1-based line within the section
        } else if current_title.is_some() && !trimmed.is_empty() {
            if !current_desc.is_empty() {
                current_desc.push('\n');
            }
            current_desc.push_str(trimmed);
        }
    }

    // Flush last step.
    if let Some(title_text) = current_title.take() {
        steps.push(PlanStep {
            index: current_index,
            title: title_text,
            description: current_desc.trim().to_owned(),
        });
    }

    if steps.is_empty() {
        return Err(Error::InvalidStep {
            line: step_start_line,
            reason: "no numbered steps found in ## Steps section".into(),
        });
    }

    Ok(steps)
}

/// Try to parse a line as `N. <title>` where N is one or more digits.
///
/// Returns `(N, title)` on success.
fn try_parse_step_line(trimmed: &str) -> Option<(usize, String)> {
    let dot_pos = trimmed.find(". ")?;
    let num_str = &trimmed[..dot_pos];

    // Must be all digits.
    if num_str.is_empty() || !num_str.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let num: usize = num_str.parse().ok()?;
    let title = trimmed[dot_pos + 2..].trim().to_owned();

    if title.is_empty() {
        return None;
    }

    Some((num, title))
}

/// Parse a bullet list (`- item`) from a section body.
fn parse_bullet_list(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix("- ")
                .map(|rest| rest.trim().to_owned())
        })
        .filter(|s| !s.is_empty())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::Anchor;

    /// Canonical sample plan in the pinned format.
    fn sample_plan_raw() -> String {
        "\
# Plan: Implement widget system

## Steps

1. Define the Widget trait
   Create a base trait with render and update methods.

2. Add Button widget
   Concrete implementation for clickable buttons.

3. Wire up event dispatch
   Connect widget events to the application event loop.

## Files

- src/widget.rs
- src/button.rs
- src/events.rs

## Risks

- Breaking change to the public API
- New dependency on crossterm
"
        .to_owned()
    }

    fn sample_plan_doc() -> PlanDoc {
        PlanDoc {
            summary: "Implement widget system".into(),
            steps: vec![
                PlanStep {
                    index: 0,
                    title: "Define the Widget trait".into(),
                    description: "Create a base trait with render and update methods.".into(),
                },
                PlanStep {
                    index: 1,
                    title: "Add Button widget".into(),
                    description: "Concrete implementation for clickable buttons.".into(),
                },
                PlanStep {
                    index: 2,
                    title: "Wire up event dispatch".into(),
                    description: "Connect widget events to the application event loop.".into(),
                },
            ],
            files: vec![
                PathBuf::from("src/widget.rs"),
                PathBuf::from("src/button.rs"),
                PathBuf::from("src/events.rs"),
            ],
            risks: vec![
                "Breaking change to the public API".into(),
                "New dependency on crossterm".into(),
            ],
            raw: String::new(), // filled by parse
        }
    }

    // -- round_trip ----------------------------------------------------------

    #[test]
    fn round_trip() {
        let doc = sample_plan_doc();
        let rendered = render(&doc);
        let parsed = parse(&rendered).expect("round-trip parse failed");

        assert_eq!(parsed.summary, doc.summary);
        assert_eq!(parsed.steps.len(), doc.steps.len());
        for (a, b) in parsed.steps.iter().zip(doc.steps.iter()) {
            assert_eq!(a.index, b.index);
            assert_eq!(a.title, b.title);
            assert_eq!(a.description, b.description);
        }
        assert_eq!(parsed.files, doc.files);
        assert_eq!(parsed.risks, doc.risks);
    }

    // -- parse_minimal -------------------------------------------------------

    #[test]
    fn parse_minimal() {
        let raw = "\
# Plan: Fix a bug

## Steps

1. Patch the handler
   One-line fix.

## Files

- src/handler.rs

## Risks

- Regression in edge case
";
        let doc = parse(raw).expect("parse_minimal failed");

        assert_eq!(doc.summary, "Fix a bug");
        assert_eq!(doc.steps.len(), 1);
        assert_eq!(doc.steps[0].index, 0);
        assert_eq!(doc.steps[0].title, "Patch the handler");
        assert_eq!(doc.steps[0].description, "One-line fix.");
        assert_eq!(doc.files, vec![PathBuf::from("src/handler.rs")]);
        assert_eq!(doc.risks, vec!["Regression in edge case".to_owned()]);
        assert_eq!(doc.raw, raw);
    }

    // -- parse_multi_step ----------------------------------------------------

    #[test]
    fn parse_multi_step() {
        let raw = sample_plan_raw();
        let doc = parse(&raw).expect("parse_multi_step failed");

        assert_eq!(doc.steps.len(), 3);

        assert_eq!(doc.steps[0].title, "Define the Widget trait");
        assert_eq!(
            doc.steps[0].description,
            "Create a base trait with render and update methods."
        );

        assert_eq!(doc.steps[1].title, "Add Button widget");
        assert_eq!(
            doc.steps[1].description,
            "Concrete implementation for clickable buttons."
        );

        assert_eq!(doc.steps[2].title, "Wire up event dispatch");
        assert_eq!(
            doc.steps[2].description,
            "Connect widget events to the application event loop."
        );
    }

    // -- parse_many_files ----------------------------------------------------

    #[test]
    fn parse_many_files() {
        let raw = "\
# Plan: Broad refactor

## Steps

1. Touch everything
   Big change.

## Files

- src/a.rs
- src/b.rs
- src/c.rs
- src/d.rs
- src/e.rs
- src/f.rs

## Risks

- Lots of churn
";
        let doc = parse(raw).expect("parse_many_files failed");
        assert_eq!(doc.files.len(), 6);
        assert_eq!(doc.files[0], PathBuf::from("src/a.rs"));
        assert_eq!(doc.files[5], PathBuf::from("src/f.rs"));
    }

    // -- parse_no_risks ------------------------------------------------------

    #[test]
    fn parse_no_risks() {
        let raw = "\
# Plan: Safe change

## Steps

1. Trivial tweak
   No-op.

## Files

- src/lib.rs

## Risks

";
        let doc = parse(raw).expect("parse_no_risks failed");
        assert!(
            doc.risks.is_empty(),
            "empty risks section should yield empty vec"
        );
    }

    // -- missing_summary -----------------------------------------------------

    #[test]
    fn missing_summary() {
        let raw = "\
## Steps

1. Do something
   Details.

## Files

- src/lib.rs

## Risks

- None
";
        let err = parse(raw).unwrap_err();
        assert!(
            matches!(err, Error::MissingSection(ref s) if s.contains("Plan")),
            "expected MissingSection for # Plan:, got: {err}"
        );
    }

    // -- missing_steps -------------------------------------------------------

    #[test]
    fn missing_steps() {
        let raw = "\
# Plan: Something

## Files

- src/lib.rs

## Risks

- None
";
        let err = parse(raw).unwrap_err();
        assert!(
            matches!(err, Error::MissingSection(ref s) if s.contains("Steps")),
            "expected MissingSection for ## Steps, got: {err}"
        );
    }

    // -- empty_input ---------------------------------------------------------

    #[test]
    fn empty_input() {
        let err = parse("").unwrap_err();
        assert!(matches!(err, Error::Empty), "expected Empty, got: {err}");

        let err2 = parse("   \n  \n  ").unwrap_err();
        assert!(
            matches!(err2, Error::Empty),
            "whitespace-only should be Empty"
        );
    }

    // -- anchors_resolve -----------------------------------------------------

    #[test]
    fn anchors_resolve() {
        let raw = sample_plan_raw();
        let doc = parse(&raw).expect("parse failed");

        // PlanStep indices are sequential and match position.
        for (pos, step) in doc.steps.iter().enumerate() {
            assert_eq!(
                step.index, pos,
                "step index {} doesn't match position {pos}",
                step.index
            );
        }

        // PlanStep anchors resolve against the parsed doc.
        for (i, step) in doc.steps.iter().enumerate() {
            let anchor = Anchor::PlanStep(i);
            match anchor {
                Anchor::PlanStep(idx) => {
                    assert!(idx < doc.steps.len(), "anchor index out of range");
                    assert_eq!(doc.steps[idx].title, step.title);
                }
                _ => panic!("expected PlanStep anchor"),
            }
        }

        // PlanFile anchors are valid PathBufs.
        for file in &doc.files {
            let anchor = Anchor::PlanFile(file.clone());
            match anchor {
                Anchor::PlanFile(ref p) => {
                    assert!(
                        !p.as_os_str().is_empty(),
                        "file anchor path should be non-empty"
                    );
                }
                _ => panic!("expected PlanFile anchor"),
            }
        }
    }

    // -- extra_whitespace ----------------------------------------------------

    #[test]
    fn extra_whitespace() {
        let raw = "
  # Plan:   Whitespace test

   ## Steps

   1.   First step
      Some    description   with   spaces.

   2.   Second step
      Another     description.

   ## Files

   -   src/lib.rs
   -    src/main.rs

   ## Risks

   -   Some risk
";
        let doc = parse(raw).expect("extra_whitespace parse failed");

        assert_eq!(doc.summary, "Whitespace test");
        assert_eq!(doc.steps.len(), 2);
        assert_eq!(doc.steps[0].title, "First step");
        assert_eq!(
            doc.steps[0].description,
            "Some    description   with   spaces."
        );
        assert_eq!(doc.steps[1].title, "Second step");
        assert_eq!(
            doc.files,
            vec![PathBuf::from("src/lib.rs"), PathBuf::from("src/main.rs")]
        );
        assert_eq!(doc.risks, vec!["Some risk".to_owned()]);
    }

    // -- parse_plan_anchor ---------------------------------------------------

    #[test]
    fn parse_plan_anchor_step() {
        let anchor = parse_plan_anchor("step:0").expect("should parse step:0");
        assert_eq!(anchor, Anchor::PlanStep(0));
    }

    #[test]
    fn parse_plan_anchor_step_large() {
        let anchor = parse_plan_anchor("step:42").expect("should parse step:42");
        assert_eq!(anchor, Anchor::PlanStep(42));
    }

    #[test]
    fn parse_plan_anchor_file() {
        let anchor = parse_plan_anchor("file:src/lib.rs").expect("should parse file anchor");
        assert_eq!(anchor, Anchor::PlanFile(PathBuf::from("src/lib.rs")));
    }

    #[test]
    fn parse_plan_anchor_file_with_spaces() {
        let anchor = parse_plan_anchor("file: src/lib.rs ").expect("should trim file path");
        assert_eq!(anchor, Anchor::PlanFile(PathBuf::from("src/lib.rs")));
    }

    #[test]
    fn parse_plan_anchor_invalid_format() {
        let err = parse_plan_anchor("invalid").unwrap_err();
        assert!(
            matches!(err, Error::InvalidStep { .. }),
            "expected InvalidStep, got {err:?}"
        );
    }

    #[test]
    fn parse_plan_anchor_empty_file() {
        let err = parse_plan_anchor("file:").unwrap_err();
        assert!(
            matches!(err, Error::InvalidStep { .. }),
            "expected InvalidStep for empty file, got {err:?}"
        );
    }

    #[test]
    fn parse_plan_anchor_non_numeric_step() {
        let err = parse_plan_anchor("step:abc").unwrap_err();
        assert!(
            matches!(err, Error::InvalidStep { .. }),
            "expected InvalidStep for non-numeric step, got {err:?}"
        );
    }

    // -- reconcile_plan -----------------------------------------------------

    #[test]
    fn reconcile_plan_updates_doc() {
        use crate::model::{GateState, ProjectPlan, ProjectRef};

        let dir = tempfile::tempdir().expect("should create temp dir");
        let plan_path = dir.path().join("plan.md");

        let raw = "\
# Plan: Updated plan

## Steps

1. New step
   New description.

## Files

- src/new.rs

## Risks

- No risks
";
        std::fs::write(&plan_path, raw).expect("should write plan file");

        // Start with a plan in Dispatched state (ready for reconcile).
        let mut plan = ProjectPlan {
            project: ProjectRef::new("proj-1"),
            doc: PlanDoc {
                summary: "Old plan".into(),
                steps: vec![],
                files: vec![],
                risks: vec![],
                raw: String::new(),
            },
            gate_state: GateState::Dispatched,
            comments: vec![],
            agent: None,
            plan_path: None,
        };

        reconcile_plan(&mut plan, &plan_path).expect("reconcile should succeed");

        assert_eq!(plan.doc.summary, "Updated plan");
        assert_eq!(plan.doc.steps.len(), 1);
        assert_eq!(plan.doc.steps[0].title, "New step");
        assert_eq!(plan.doc.files, vec![PathBuf::from("src/new.rs")]);
        assert_eq!(plan.gate_state, GateState::Reworked);
    }

    #[test]
    fn reconcile_plan_wrong_state() {
        use crate::model::{GateState, ProjectPlan, ProjectRef};

        let dir = tempfile::tempdir().expect("should create temp dir");
        let plan_path = dir.path().join("plan.md");
        std::fs::write(
            &plan_path,
            "# Plan: X\n\n## Steps\n\n1. A\n   B\n\n## Files\n\n- f\n\n## Risks\n",
        )
        .expect("should write");

        // Plan is in InReview, not Dispatched — reconcile should fail.
        let mut plan = ProjectPlan {
            project: ProjectRef::new("proj-1"),
            doc: PlanDoc {
                summary: "Old".into(),
                steps: vec![],
                files: vec![],
                risks: vec![],
                raw: String::new(),
            },
            gate_state: GateState::InReview,
            comments: vec![],
            agent: None,
            plan_path: None,
        };

        let err = reconcile_plan(&mut plan, &plan_path).unwrap_err();
        assert!(
            matches!(err, ReconcileError::TransitionFailed(_)),
            "expected TransitionFailed, got {err:?}"
        );
    }

    #[test]
    fn reconcile_plan_missing_file() {
        use crate::model::{GateState, ProjectPlan, ProjectRef};

        let mut plan = ProjectPlan {
            project: ProjectRef::new("proj-1"),
            doc: PlanDoc {
                summary: "Old".into(),
                steps: vec![],
                files: vec![],
                risks: vec![],
                raw: String::new(),
            },
            gate_state: GateState::Dispatched,
            comments: vec![],
            agent: None,
            plan_path: None,
        };

        let err =
            reconcile_plan(&mut plan, std::path::Path::new("/nonexistent/plan.md")).unwrap_err();
        assert!(
            matches!(err, ReconcileError::ReadFailed { .. }),
            "expected ReadFailed, got {err:?}"
        );
    }

    // -- multi-line description preserved ------------------------------------

    #[test]
    fn multi_line_description() {
        let raw = "\
# Plan: Multi-line test

## Steps

1. Complex step
   Line one of the description.
   Line two of the description.
   Line three of the description.

## Files

- src/lib.rs

## Risks

- None
";
        let doc = parse(raw).expect("multi_line_description failed");
        assert_eq!(doc.steps.len(), 1);
        assert_eq!(
            doc.steps[0].description,
            "Line one of the description.\nLine two of the description.\nLine three of the description."
        );
    }
}
