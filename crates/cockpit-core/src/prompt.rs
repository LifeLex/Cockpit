//! Deterministic prompt assembly per `SPEC.md` §9.
//!
//! Builds rework prompts in a fixed section order so the same inputs always
//! produce the same output. No timestamps, no randomness, no system-dependent
//! content.

use std::fmt::Write;

use sha2::{Digest, Sha256};

use crate::model::{Anchor, Artifact, Comment, PlanDoc};

/// The scope-guard text, verbatim from `SPEC.md` §9.
///
/// The test-weakening clause is the highest-ROI line in the prompt.
const SCOPE_GUARD: &str = "\
Address only the comments above. \
Don't refactor unrelated code. \
Don't weaken or delete tests. \
If a comment is wrong or impossible, stop and say so.";

/// Input bundle for rework prompt assembly.
///
/// Collects references to the data needed to build a rework prompt,
/// avoiding a long parameter list.
pub struct ReworkInput<'a> {
    /// Intent: project-level summary or issue acceptance criteria.
    pub intent: &'a str,
    /// The approved plan, if this is a diff-gate rework (absent for plan-gate).
    pub approved_plan: Option<&'a PlanDoc>,
    /// The current artifact being reviewed.
    pub artifact: &'a Artifact,
    /// Gathered comments for this review cycle.
    pub comments: &'a [Comment],
}

/// Assembled prompt ready for dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledPrompt {
    /// The full prompt text.
    pub text: String,
    /// SHA-256 hex digest of `text`, for dedup/audit.
    pub hash: String,
}

/// Assemble a deterministic rework prompt per `SPEC.md` §9.
///
/// Sections appear in fixed order:
/// 1. Intent
/// 2. Approved plan (diff gate only; omitted for plan gate)
/// 3. Current artifact
/// 4. Comments with rendered anchors
/// 5. Scope guard
///
/// Same inputs always produce the same output.
pub fn assemble_rework(input: &ReworkInput<'_>) -> AssembledPrompt {
    // INVARIANT: all writeln!/write! calls target a String, whose fmt::Write
    // impl is infallible — unwrap() cannot panic.
    let mut text = String::new();

    // §1 — Intent
    writeln!(text, "## Intent\n").unwrap();
    writeln!(text, "{}\n", input.intent).unwrap();

    // §2 — Approved plan (diff gate only)
    if let Some(plan) = input.approved_plan {
        writeln!(text, "## Approved Plan\n").unwrap();
        writeln!(text, "{}\n", plan.raw).unwrap();
    }

    // §3 — Current artifact
    writeln!(text, "## Current Artifact\n").unwrap();
    match input.artifact {
        Artifact::Plan(doc) => writeln!(text, "{}\n", doc.raw).unwrap(),
        Artifact::Diff(data) => writeln!(text, "{}\n", data.raw).unwrap(),
    }

    // §4 — Comments with anchors
    writeln!(text, "## Comments\n").unwrap();
    if input.comments.is_empty() {
        writeln!(text, "No comments.\n").unwrap();
    } else {
        let plan_doc = match input.artifact {
            Artifact::Plan(doc) => Some(doc),
            Artifact::Diff(_) => None,
        };
        for (i, comment) in input.comments.iter().enumerate() {
            let anchor = render_anchor(&comment.anchor, plan_doc);
            writeln!(text, "{}. [{}] {}\n", i + 1, anchor, comment.body).unwrap();
        }
    }

    // §5 — Scope guard
    writeln!(text, "## Scope Guard\n").unwrap();
    writeln!(text, "{}\n", SCOPE_GUARD).unwrap();

    let hash = sha256_hex(&text);

    AssembledPrompt { text, hash }
}

/// Render an anchor as a human-readable location string.
///
/// When the artifact is a plan, `PlanStep` anchors are enriched with the
/// step title if available.
pub fn render_anchor(anchor: &Anchor, plan_doc: Option<&PlanDoc>) -> String {
    match anchor {
        Anchor::PlanStep(idx) => {
            if let Some(doc) = plan_doc {
                if let Some(step) = doc.steps.get(*idx) {
                    return format!("plan step {}: {}", idx, step.title);
                }
            }
            format!("plan step {idx}")
        }
        Anchor::PlanFile(path) => format!("plan file: {}", path.display()),
        Anchor::DiffLine { path, range } => {
            format!("{}:{}-{}", path.display(), range.0, range.1)
        }
    }
}

/// Compute the SHA-256 hex digest of the given text.
fn sha256_hex(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let result = hasher.finalize();
    // INVARIANT: write! into String is infallible.
    result.iter().fold(String::with_capacity(64), |mut acc, b| {
        write!(acc, "{b:02x}").unwrap();
        acc
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::{CommentId, CommentOrigin, DiffData, PlanStep};

    fn sample_plan_doc() -> PlanDoc {
        PlanDoc {
            summary: "Implement the widget system".into(),
            steps: vec![
                PlanStep {
                    index: 0,
                    title: "Define widget trait".into(),
                    description: "Create the base Widget trait".into(),
                },
                PlanStep {
                    index: 1,
                    title: "Add button widget".into(),
                    description: "Concrete button implementation".into(),
                },
            ],
            files: vec![
                PathBuf::from("src/widget.rs"),
                PathBuf::from("src/button.rs"),
            ],
            risks: vec!["breaking change to public API".into()],
            raw: "# Widget Plan\n\n1. Define widget trait\n2. Add button widget".into(),
        }
    }

    fn sample_diff_comments() -> Vec<Comment> {
        vec![
            Comment {
                id: CommentId::new("c-1"),
                anchor: Anchor::DiffLine {
                    path: PathBuf::from("src/main.rs"),
                    range: (10, 15),
                },
                body: "This function needs error handling".into(),
                origin: CommentOrigin::Local,
            },
            Comment {
                id: CommentId::new("c-2"),
                anchor: Anchor::DiffLine {
                    path: PathBuf::from("src/lib.rs"),
                    range: (42, 42),
                },
                body: "Missing doc comment on public method".into(),
                origin: CommentOrigin::Local,
            },
        ]
    }

    fn sample_plan_comments() -> Vec<Comment> {
        vec![
            Comment {
                id: CommentId::new("c-1"),
                anchor: Anchor::PlanStep(0),
                body: "This step is too vague, add detail".into(),
                origin: CommentOrigin::Local,
            },
            Comment {
                id: CommentId::new("c-2"),
                anchor: Anchor::PlanFile(PathBuf::from("src/widget.rs")),
                body: "Consider splitting into two files".into(),
                origin: CommentOrigin::Local,
            },
        ]
    }

    #[test]
    fn rework_diff_gate_golden() {
        let plan = sample_plan_doc();
        let diff = Artifact::Diff(DiffData {
            raw: "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -10,6 +10,8 @@\n+fn new_function() {}".into(),
        });
        let comments = sample_diff_comments();

        let input = ReworkInput {
            intent: "Implement the widget system for the dashboard",
            approved_plan: Some(&plan),
            artifact: &diff,
            comments: &comments,
        };

        let result = assemble_rework(&input);

        let expected = include_str!("../tests/golden/rework_diff_gate.txt");
        assert_eq!(result.text, expected, "diff-gate golden file mismatch");
    }

    #[test]
    fn rework_plan_gate_golden() {
        let plan_artifact = Artifact::Plan(sample_plan_doc());
        let comments = sample_plan_comments();

        let input = ReworkInput {
            intent: "Build a reusable widget framework",
            approved_plan: None,
            artifact: &plan_artifact,
            comments: &comments,
        };

        let result = assemble_rework(&input);

        let expected = include_str!("../tests/golden/rework_plan_gate.txt");
        assert_eq!(result.text, expected, "plan-gate golden file mismatch");
    }

    #[test]
    fn rework_no_comments_golden() {
        let diff = Artifact::Diff(DiffData {
            raw: "diff --git a/README.md b/README.md".into(),
        });

        let input = ReworkInput {
            intent: "Update documentation",
            approved_plan: None,
            artifact: &diff,
            comments: &[],
        };

        let result = assemble_rework(&input);

        let expected = include_str!("../tests/golden/rework_no_comments.txt");
        assert_eq!(result.text, expected, "no-comments golden file mismatch");
    }

    #[test]
    fn hash_is_deterministic() {
        let diff = Artifact::Diff(DiffData {
            raw: "some diff".into(),
        });
        let comments = sample_diff_comments();

        let input = ReworkInput {
            intent: "test",
            approved_plan: None,
            artifact: &diff,
            comments: &comments,
        };

        let a = assemble_rework(&input);
        let b = assemble_rework(&input);

        assert_eq!(a.hash, b.hash, "same inputs must produce same hash");
        assert_eq!(a.hash.len(), 64, "SHA-256 hex digest is 64 chars");
        assert!(
            a.hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hash must be hex"
        );
    }

    #[test]
    fn hash_changes_with_content() {
        let diff = Artifact::Diff(DiffData {
            raw: "diff a".into(),
        });

        let a = assemble_rework(&ReworkInput {
            intent: "intent a",
            approved_plan: None,
            artifact: &diff,
            comments: &[],
        });

        let b = assemble_rework(&ReworkInput {
            intent: "intent b",
            approved_plan: None,
            artifact: &diff,
            comments: &[],
        });

        assert_ne!(
            a.hash, b.hash,
            "different inputs must produce different hashes"
        );
    }

    #[test]
    fn render_anchor_diff_line() {
        let anchor = Anchor::DiffLine {
            path: PathBuf::from("src/main.rs"),
            range: (10, 15),
        };
        assert_eq!(render_anchor(&anchor, None), "src/main.rs:10-15");
    }

    #[test]
    fn render_anchor_plan_step_with_title() {
        let doc = sample_plan_doc();
        let anchor = Anchor::PlanStep(0);
        assert_eq!(
            render_anchor(&anchor, Some(&doc)),
            "plan step 0: Define widget trait"
        );
    }

    #[test]
    fn render_anchor_plan_step_without_doc() {
        let anchor = Anchor::PlanStep(3);
        assert_eq!(render_anchor(&anchor, None), "plan step 3");
    }

    #[test]
    fn render_anchor_plan_file() {
        let anchor = Anchor::PlanFile(PathBuf::from("src/widget.rs"));
        assert_eq!(render_anchor(&anchor, None), "plan file: src/widget.rs");
    }
}
