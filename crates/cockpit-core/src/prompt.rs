//! Deterministic prompt assembly per `SPEC.md` §9.
//!
//! Builds rework prompts in a fixed section order so the same inputs always
//! produce the same output. No timestamps, no randomness, no system-dependent
//! content.

use std::fmt::Write;

use sha2::{Digest, Sha256};

use crate::model::{AgentMode, Anchor, Artifact, Comment, PlanDoc};
use crate::skills::Skill;

/// The scope-guard text, verbatim from `SPEC.md` §9.
///
/// The test-weakening clause is the highest-ROI line in the prompt.
const SCOPE_GUARD: &str = "\
Address only the comments above. \
Don't refactor unrelated code. \
Don't weaken or delete tests. \
If a comment is wrong or impossible, stop and say so.";

/// The builtin scope-guard text (`SPEC.md` §9).
///
/// Exposed so callers (settings UI, tests) can show or reset against the
/// canonical default without duplicating the string.
pub fn scope_guard() -> &'static str {
    SCOPE_GUARD
}

/// The builtin per-[`AgentMode`] intent/instruction fragment.
///
/// This is the default preamble text for each mode — the placeholder the
/// settings editor shows and the value a "reset to default" restores. It is
/// **not** the full assembled prompt; it is the mode-specific instruction that
/// a custom preamble augments (custom text is injected in addition to, and
/// ahead of, the assembly's own sections — see [`assemble_rework`]).
pub fn builtin_intent(mode: AgentMode) -> &'static str {
    match mode {
        AgentMode::Plan => PLAN_INSTRUCTION,
        AgentMode::Implement => IMPLEMENT_INSTRUCTION,
        AgentMode::Fix => SCOPE_GUARD,
        AgentMode::Restack => RESTACK_INSTRUCTION,
    }
}

/// The builtin implementer instruction, verbatim.
///
/// Guides the initial-implementation spawn toward a committed, pushed PR.
const IMPLEMENT_INSTRUCTION: &str = "\
Create the initial implementation for the issue above. \
Follow the approved plan and repository conventions. \
Commit your work and push the branch. \
Don't weaken or delete tests.";

/// The builtin restack / conflict-resolution instruction, verbatim.
///
/// Guides the conflict-resolver spawn dispatched when a rebase fails.
const RESTACK_INSTRUCTION: &str = "\
Resolve the rebase conflicts in this worktree. \
Preserve the intent of both sides. \
Don't refactor unrelated code and don't weaken or delete tests. \
If a conflict is ambiguous, stop and say so.";

/// Input bundle for rework prompt assembly.
///
/// Collects references to the data needed to build a rework prompt,
/// avoiding a long parameter list.
pub struct ReworkInput<'a> {
    /// Intent: project-level summary or issue acceptance criteria.
    pub intent: &'a str,
    /// Optional user-authored preamble, injected **verbatim** in a fixed
    /// position (right after the Intent section, before the rest of the
    /// assembly). `None` or an empty/whitespace-only string is omitted, so the
    /// prompt is byte-identical to the builtin-only output. This is the
    /// per-[`AgentMode`] override seam (see [`crate::config::AgentPrompts`]).
    pub custom_preamble: Option<&'a str>,
    /// The approved plan, if this is a diff-gate rework (absent for plan-gate).
    pub approved_plan: Option<&'a PlanDoc>,
    /// The current artifact being reviewed.
    pub artifact: &'a Artifact,
    /// Gathered comments for this review cycle.
    pub comments: &'a [Comment],
    /// Optional CI failure logs to include, emitted verbatim in a fixed
    /// position (after Comments, before the Scope Guard). `None` or an
    /// empty/whitespace-only string is omitted, so the prompt is byte-identical
    /// to output without CI. Populated only by the explicit "Fix CI" action.
    pub ci_failures: Option<&'a str>,
    /// Pre-filtered project skills to inject as conventions.
    ///
    /// Pass an empty slice to omit the conventions section. Callers are
    /// responsible for calling [`crate::skills::discover_skills`] and
    /// [`crate::skills::filter_relevant`] before constructing this input.
    pub skills: &'a [Skill],
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
///    (then the custom preamble, verbatim; omitted when None/empty)
/// 2. Approved plan (diff gate only; omitted for plan gate)
/// 3. Current artifact
/// 4. Comments with rendered anchors
/// 5. Scope guard
///
/// Same inputs always produce the same output. When `custom_preamble` is
/// `None` or empty the output is byte-identical to the builtin-only prompt.
pub fn assemble_rework(input: &ReworkInput<'_>) -> AssembledPrompt {
    // INVARIANT: all writeln!/write! calls target a String, whose fmt::Write
    // impl is infallible — unwrap() cannot panic.
    let mut text = String::new();

    // §1 — Intent
    writeln!(text, "## Intent\n").unwrap();
    writeln!(text, "{}\n", input.intent).unwrap();

    // §1b — Custom preamble (verbatim override). Fixed position: right after
    // Intent, before the rest. Empty/None is omitted so output is identical to
    // the builtin-only prompt.
    write_custom_preamble(&mut text, input.custom_preamble);

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

    // §4b — CI failures (verbatim). Fixed position: after Comments, before the
    // Scope Guard. Empty/None is omitted so output is byte-identical to a
    // prompt assembled without CI logs.
    write_ci_failures(&mut text, input.ci_failures);

    // §5 — Scope guard
    writeln!(text, "## Scope Guard\n").unwrap();
    writeln!(text, "{}\n", SCOPE_GUARD).unwrap();

    // §6 — Project conventions (skills)
    let conventions = crate::skills::format_for_prompt(input.skills);
    if !conventions.is_empty() {
        text.push_str(&conventions);
    }

    let hash = sha256_hex(&text);

    AssembledPrompt { text, hash }
}

/// Write a CI-failures section verbatim, if present and non-empty.
///
/// The heading is fixed so the section is stable and recognizable; the body is
/// the failed-CI log tail exactly as supplied. A `None` or whitespace-only value
/// writes nothing, keeping the output byte-identical to a CI-free prompt.
fn write_ci_failures(text: &mut String, ci_failures: Option<&str>) {
    if let Some(body) = ci_failures {
        if !body.trim().is_empty() {
            // INVARIANT: writeln! into String is infallible.
            writeln!(text, "## CI Failures\n").unwrap();
            writeln!(text, "{body}\n").unwrap();
        }
    }
}

/// The plan-generation instruction, verbatim.
///
/// Names the required output shape so the planner produces a document the
/// [`crate::plan_parser`] can parse deterministically.
const PLAN_INSTRUCTION: &str = "\
Produce a project plan. Name the files to touch, the order of the steps, \
and the risks (migrations, new dependencies, breaking changes). \
If the goal is ambiguous or under-specified, stop and say so.";

/// Input bundle for initial plan-generation prompt assembly.
///
/// Used when a project's plan needs generating for the first time (the
/// planner-fill spawn). This is distinct from [`ReworkInput`]: there are no
/// comments to address yet — the agent is producing the plan from scratch.
pub struct PlanInput<'a> {
    /// Project intent: the overall goal the plan must achieve.
    pub intent: &'a str,
    /// Optional user-authored preamble, injected **verbatim** right after the
    /// Intent section. `None` or empty is omitted (identical to builtin-only).
    /// This is the [`AgentMode::Plan`] override seam.
    pub custom_preamble: Option<&'a str>,
    /// Issue list with optional dependency notes, one entry per issue.
    ///
    /// Each string is rendered verbatim as a bullet; callers format the
    /// dependency notes into the entry (e.g. `"NEX-2 (depends on NEX-1)"`).
    pub issues: &'a [String],
    /// The current plan artifact, if any (empty on first generation).
    ///
    /// Rendered so an agent revising a stub plan sees where it started.
    pub current_plan: Option<&'a PlanDoc>,
    /// Absolute path the planner must write its finished plan markdown to.
    ///
    /// When set, an explicit "Output" section instructs the agent to persist
    /// the plan to this exact path so cockpit can read and parse it back on
    /// completion. `None` omits the section (e.g. for tests that only check the
    /// instruction shape).
    pub output_path: Option<&'a std::path::Path>,
    /// Pre-filtered project skills to inject as conventions.
    ///
    /// Pass an empty slice to omit the conventions section.
    pub skills: &'a [Skill],
}

/// Assemble a deterministic initial plan-generation prompt per `SPEC.md` §9.
///
/// Sections appear in fixed order:
/// 1. Intent
///    (then the custom preamble, verbatim; omitted when None/empty)
/// 2. Issues (with dependency notes)
/// 3. Current plan (only when a non-empty stub exists)
/// 4. Instruction (name files, order, risks)
/// 5. Project conventions (skills)
///
/// Same inputs always produce the same output. This is the artifact-filling
/// counterpart of [`assemble_rework`]: it does not carry comments because the
/// plan does not exist yet.
pub fn assemble_plan_prompt(input: &PlanInput<'_>) -> AssembledPrompt {
    // INVARIANT: all writeln!/write! calls target a String, whose fmt::Write
    // impl is infallible — unwrap() cannot panic.
    let mut text = String::new();

    // §1 — Intent
    writeln!(text, "## Intent\n").unwrap();
    writeln!(text, "{}\n", input.intent).unwrap();

    // §1b — Custom preamble (verbatim override), fixed position after Intent.
    write_custom_preamble(&mut text, input.custom_preamble);

    // §2 — Issues
    writeln!(text, "## Issues\n").unwrap();
    if input.issues.is_empty() {
        writeln!(text, "No issues listed.\n").unwrap();
    } else {
        for issue in input.issues {
            writeln!(text, "- {issue}").unwrap();
        }
        writeln!(text).unwrap();
    }

    // §3 — Current plan (only if a non-empty stub exists)
    if let Some(plan) = input.current_plan {
        if !plan.raw.trim().is_empty() {
            writeln!(text, "## Current Plan\n").unwrap();
            writeln!(text, "{}\n", plan.raw).unwrap();
        }
    }

    // §4 — Instruction
    writeln!(text, "## Instruction\n").unwrap();
    writeln!(text, "{PLAN_INSTRUCTION}\n").unwrap();

    // §5 — Output (only when a destination path is supplied)
    if let Some(path) = input.output_path {
        writeln!(text, "## Output\n").unwrap();
        writeln!(
            text,
            "Write the finished plan as markdown to `{}`, using the pinned format:\n",
            path.display()
        )
        .unwrap();
        writeln!(text, "{PLAN_FORMAT}\n").unwrap();
    }

    // §6 — Project conventions (skills)
    let conventions = crate::skills::format_for_prompt(input.skills);
    if !conventions.is_empty() {
        text.push_str(&conventions);
    }

    let hash = sha256_hex(&text);

    AssembledPrompt { text, hash }
}

/// The pinned plan-document format, verbatim.
///
/// Mirrors [`crate::plan_parser`]'s accepted grammar so the planner writes a
/// file the parser can round-trip. Kept as a template (not the real values) so
/// the instruction is deterministic and self-describing.
const PLAN_FORMAT: &str = "\
# Plan: <one-line summary>

## Steps

1. <step title>
   <step description>

## Files

- path/to/file

## Risks

- <risk>";

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

/// Write a custom preamble section verbatim, if present and non-empty.
///
/// The heading is fixed so the section is stable and recognizable; the body is
/// the user's text exactly as supplied (never paraphrased). A `None` or
/// whitespace-only preamble writes nothing, keeping the output byte-identical
/// to the builtin-only prompt.
fn write_custom_preamble(text: &mut String, preamble: Option<&str>) {
    if let Some(body) = preamble {
        if !body.trim().is_empty() {
            // INVARIANT: writeln! into String is infallible.
            writeln!(text, "## Custom Instructions\n").unwrap();
            writeln!(text, "{body}\n").unwrap();
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
            custom_preamble: None,
            approved_plan: Some(&plan),
            artifact: &diff,
            comments: &comments,
            ci_failures: None,
            skills: &[],
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
            custom_preamble: None,
            approved_plan: None,
            artifact: &plan_artifact,
            comments: &comments,
            ci_failures: None,
            skills: &[],
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
            custom_preamble: None,
            approved_plan: None,
            artifact: &diff,
            comments: &[],
            ci_failures: None,
            skills: &[],
        };

        let result = assemble_rework(&input);

        let expected = include_str!("../tests/golden/rework_no_comments.txt");
        assert_eq!(result.text, expected, "no-comments golden file mismatch");
    }

    #[test]
    fn plan_generation_golden() {
        let issues = vec![
            "NEX-1 Define the widget trait".to_string(),
            "NEX-2 Add button widget (depends on NEX-1)".to_string(),
            "NEX-3 Wire event dispatch (depends on NEX-2)".to_string(),
        ];

        let input = PlanInput {
            intent: "Build a reusable widget framework",
            custom_preamble: None,
            issues: &issues,
            current_plan: None,
            output_path: None,
            skills: &[],
        };

        let result = assemble_plan_prompt(&input);

        let expected = include_str!("../tests/golden/plan_generation.txt");
        assert_eq!(
            result.text, expected,
            "plan-generation golden file mismatch"
        );
    }

    #[test]
    fn plan_generation_with_output_golden() {
        let issues = vec![
            "NEX-1 Define the widget trait".to_string(),
            "NEX-2 Add button widget (depends on NEX-1)".to_string(),
            "NEX-3 Wire event dispatch (depends on NEX-2)".to_string(),
        ];

        let output = std::path::Path::new("/tmp/cockpit-test/plans/proj-1.md");
        let input = PlanInput {
            intent: "Build a reusable widget framework",
            custom_preamble: None,
            issues: &issues,
            current_plan: None,
            output_path: Some(output),
            skills: &[],
        };

        let result = assemble_plan_prompt(&input);

        let expected = include_str!("../tests/golden/plan_generation_with_output.txt");
        assert_eq!(
            result.text, expected,
            "plan-generation-with-output golden file mismatch"
        );
    }

    #[test]
    fn plan_generation_includes_current_stub() {
        let stub = PlanDoc {
            summary: "stub".into(),
            steps: vec![],
            files: vec![],
            risks: vec![],
            raw: "# Plan: stub\n\n## Steps\n\n1. Placeholder".into(),
        };
        let input = PlanInput {
            intent: "Refine the plan",
            custom_preamble: None,
            issues: &["NEX-9 do a thing".to_string()],
            current_plan: Some(&stub),
            output_path: None,
            skills: &[],
        };
        let result = assemble_plan_prompt(&input);
        assert!(
            result.text.contains("## Current Plan"),
            "non-empty stub should be rendered"
        );
        assert!(result.text.contains("Placeholder"));
    }

    #[test]
    fn plan_generation_omits_empty_stub() {
        let stub = PlanDoc {
            summary: String::new(),
            steps: vec![],
            files: vec![],
            risks: vec![],
            raw: String::new(),
        };
        let input = PlanInput {
            intent: "Generate from scratch",
            custom_preamble: None,
            issues: &[],
            current_plan: Some(&stub),
            output_path: None,
            skills: &[],
        };
        let result = assemble_plan_prompt(&input);
        assert!(
            !result.text.contains("## Current Plan"),
            "empty stub should not produce a Current Plan section"
        );
    }

    #[test]
    fn plan_generation_deterministic() {
        let issues = vec!["NEX-1 a".to_string()];
        let input = PlanInput {
            intent: "x",
            custom_preamble: None,
            issues: &issues,
            current_plan: None,
            output_path: None,
            skills: &[],
        };
        let a = assemble_plan_prompt(&input);
        let b = assemble_plan_prompt(&input);
        assert_eq!(a.hash, b.hash, "same inputs must produce same hash");
        assert_eq!(a.hash.len(), 64);
    }

    #[test]
    fn hash_is_deterministic() {
        let diff = Artifact::Diff(DiffData {
            raw: "some diff".into(),
        });
        let comments = sample_diff_comments();

        let input = ReworkInput {
            intent: "test",
            custom_preamble: None,
            approved_plan: None,
            artifact: &diff,
            comments: &comments,
            ci_failures: None,
            skills: &[],
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
            custom_preamble: None,
            approved_plan: None,
            artifact: &diff,
            comments: &[],
            ci_failures: None,
            skills: &[],
        });

        let b = assemble_rework(&ReworkInput {
            intent: "intent b",
            custom_preamble: None,
            approved_plan: None,
            artifact: &diff,
            comments: &[],
            ci_failures: None,
            skills: &[],
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

    #[test]
    fn skills_appended_after_scope_guard() {
        let diff = Artifact::Diff(DiffData {
            raw: "diff --git a/f b/f".into(),
        });
        let skills = vec![Skill {
            name: "error-handling".into(),
            description: "Error conventions".into(),
            tags: vec![],
            body: "Use thiserror in core crates.".into(),
            path: PathBuf::from("errors.md"),
            source: crate::skills::SkillSource::Local,
        }];

        let input = ReworkInput {
            intent: "Fix error handling",
            custom_preamble: None,
            approved_plan: None,
            artifact: &diff,
            comments: &[],
            ci_failures: None,
            skills: &skills,
        };

        let result = assemble_rework(&input);

        // Skills section should appear after scope guard.
        let scope_pos = result
            .text
            .find("## Scope Guard")
            .expect("scope guard missing");
        let conventions_pos = result
            .text
            .find("## Project Conventions")
            .expect("conventions missing");
        assert!(
            conventions_pos > scope_pos,
            "conventions should follow scope guard"
        );
        assert!(result.text.contains("### error-handling"));
        assert!(result.text.contains("Use thiserror in core crates."));
    }

    #[test]
    fn rework_with_skills_golden() {
        // Golden proving a relevant skill lands in the prompt after the scope
        // guard, in the fixed conventions format.
        let diff = Artifact::Diff(DiffData {
            raw: "diff --git a/src/main.rs b/src/main.rs".into(),
        });
        let skills = vec![Skill {
            name: "error-handling".into(),
            description: "Error conventions".into(),
            tags: vec![],
            body: "Use thiserror in core crates.".into(),
            path: PathBuf::from("error-handling/SKILL.md"),
            source: crate::skills::SkillSource::Local,
        }];

        let input = ReworkInput {
            intent: "Fix error handling",
            custom_preamble: None,
            approved_plan: None,
            artifact: &diff,
            comments: &[],
            ci_failures: None,
            skills: &skills,
        };

        let result = assemble_rework(&input);
        let expected = include_str!("../tests/golden/rework_with_skills.txt");
        assert_eq!(result.text, expected, "skills golden mismatch");
    }

    #[test]
    fn empty_skills_no_conventions_section() {
        let diff = Artifact::Diff(DiffData {
            raw: "diff --git a/f b/f".into(),
        });

        let input = ReworkInput {
            intent: "Do something",
            custom_preamble: None,
            approved_plan: None,
            artifact: &diff,
            comments: &[],
            ci_failures: None,
            skills: &[],
        };

        let result = assemble_rework(&input);
        assert!(
            !result.text.contains("## Project Conventions"),
            "empty skills should not produce a conventions section"
        );
    }

    // ---------------------------------------------------------------
    // Custom preamble (verbatim override) tests
    // ---------------------------------------------------------------

    /// A helper diff artifact shared by the preamble tests.
    fn preamble_diff() -> Artifact {
        Artifact::Diff(DiffData {
            raw: "diff --git a/f b/f".into(),
        })
    }

    #[test]
    fn custom_preamble_present_golden() {
        let diff = preamble_diff();
        let input = ReworkInput {
            intent: "Fix the parser",
            custom_preamble: Some(
                "ALWAYS run `cargo fmt` before committing.\nPrefer small commits.",
            ),
            approved_plan: None,
            artifact: &diff,
            comments: &[],
            ci_failures: None,
            skills: &[],
        };

        let result = assemble_rework(&input);
        let expected = include_str!("../tests/golden/rework_custom_preamble.txt");
        assert_eq!(result.text, expected, "custom-preamble golden mismatch");
    }

    #[test]
    fn empty_preamble_identical_to_builtin_only() {
        let diff = preamble_diff();
        let base = ReworkInput {
            intent: "Fix the parser",
            custom_preamble: None,
            approved_plan: None,
            artifact: &diff,
            comments: &[],
            ci_failures: None,
            skills: &[],
        };
        let builtin_only = assemble_rework(&base);

        // None, empty, and whitespace-only all produce byte-identical output.
        for preamble in [None, Some(""), Some("   \n  ")] {
            let input = ReworkInput {
                intent: "Fix the parser",
                custom_preamble: preamble,
                approved_plan: None,
                artifact: &diff,
                comments: &[],
                ci_failures: None,
                skills: &[],
            };
            let result = assemble_rework(&input);
            assert_eq!(
                result.text, builtin_only.text,
                "empty/whitespace preamble ({preamble:?}) must equal builtin-only output"
            );
            assert_eq!(result.hash, builtin_only.hash);
        }
    }

    #[test]
    fn custom_preamble_injected_verbatim_after_intent() {
        let diff = preamble_diff();
        let verbatim = "Do EXACTLY this. Line two: `weird *chars* & symbols`.";
        let input = ReworkInput {
            intent: "Fix the parser",
            custom_preamble: Some(verbatim),
            approved_plan: None,
            artifact: &diff,
            comments: &[],
            ci_failures: None,
            skills: &[],
        };
        let result = assemble_rework(&input);

        // The user's text appears exactly, unparaphrased.
        assert!(
            result.text.contains(verbatim),
            "preamble must be injected verbatim"
        );
        // Fixed position: after Intent, before the artifact section.
        let intent_pos = result.text.find("## Intent").expect("intent missing");
        let preamble_pos = result
            .text
            .find("## Custom Instructions")
            .expect("custom section missing");
        let artifact_pos = result
            .text
            .find("## Current Artifact")
            .expect("artifact missing");
        assert!(
            intent_pos < preamble_pos && preamble_pos < artifact_pos,
            "preamble must sit between Intent and Current Artifact"
        );
    }

    #[test]
    fn plan_prompt_custom_preamble_injected_verbatim() {
        let verbatim = "Plan preamble: keep steps under 5.";
        let input = PlanInput {
            intent: "Build a framework",
            custom_preamble: Some(verbatim),
            issues: &["NEX-1 do it".to_string()],
            current_plan: None,
            output_path: None,
            skills: &[],
        };
        let result = assemble_plan_prompt(&input);
        assert!(result.text.contains(verbatim));
        let intent_pos = result.text.find("## Intent").expect("intent missing");
        let preamble_pos = result
            .text
            .find("## Custom Instructions")
            .expect("custom section missing");
        let issues_pos = result.text.find("## Issues").expect("issues missing");
        assert!(intent_pos < preamble_pos && preamble_pos < issues_pos);
    }

    #[test]
    fn plan_prompt_empty_preamble_identical() {
        let issues = vec!["NEX-1 do it".to_string()];
        let with_none = assemble_plan_prompt(&PlanInput {
            intent: "Build a framework",
            custom_preamble: None,
            issues: &issues,
            current_plan: None,
            output_path: None,
            skills: &[],
        });
        let with_empty = assemble_plan_prompt(&PlanInput {
            intent: "Build a framework",
            custom_preamble: Some("  "),
            issues: &issues,
            current_plan: None,
            output_path: None,
            skills: &[],
        });
        assert_eq!(with_none.text, with_empty.text);
    }

    #[test]
    fn builtin_intent_covers_all_modes() {
        for mode in [
            AgentMode::Plan,
            AgentMode::Implement,
            AgentMode::Fix,
            AgentMode::Restack,
        ] {
            assert!(
                !builtin_intent(mode).is_empty(),
                "builtin intent for {mode:?} must be non-empty"
            );
        }
        assert_eq!(scope_guard(), builtin_intent(AgentMode::Fix));
    }

    // ---------------------------------------------------------------
    // CI-failures section tests
    // ---------------------------------------------------------------

    #[test]
    fn ci_failures_present_golden() {
        let diff = preamble_diff();
        let input = ReworkInput {
            intent: "Fix the parser",
            custom_preamble: None,
            approved_plan: None,
            artifact: &diff,
            comments: &[],
            ci_failures: Some(
                "=== run 222 (failed jobs) ===\ntest_parse ... FAILED\nassertion failed: left == right",
            ),
            skills: &[],
        };

        let result = assemble_rework(&input);
        let expected = include_str!("../tests/golden/rework_ci_failures.txt");
        assert_eq!(result.text, expected, "ci-failures golden mismatch");
    }

    #[test]
    fn ci_failures_sits_after_comments_before_scope_guard() {
        let diff = preamble_diff();
        let logs = "flaky_test FAILED at step 3";
        let input = ReworkInput {
            intent: "Fix CI",
            custom_preamble: None,
            approved_plan: None,
            artifact: &diff,
            comments: &[],
            ci_failures: Some(logs),
            skills: &[],
        };
        let result = assemble_rework(&input);

        assert!(result.text.contains(logs), "CI logs injected verbatim");
        let comments_pos = result.text.find("## Comments").expect("comments missing");
        let ci_pos = result
            .text
            .find("## CI Failures")
            .expect("CI section missing");
        let guard_pos = result
            .text
            .find("## Scope Guard")
            .expect("scope guard missing");
        assert!(
            comments_pos < ci_pos && ci_pos < guard_pos,
            "CI Failures must sit between Comments and Scope Guard"
        );
    }

    #[test]
    fn empty_ci_failures_identical_to_none() {
        let diff = preamble_diff();
        let base = assemble_rework(&ReworkInput {
            intent: "Fix the parser",
            custom_preamble: None,
            approved_plan: None,
            artifact: &diff,
            comments: &[],
            ci_failures: None,
            skills: &[],
        });

        // None, empty, and whitespace-only all produce byte-identical output.
        for ci in [None, Some(""), Some("  \n \t")] {
            let result = assemble_rework(&ReworkInput {
                intent: "Fix the parser",
                custom_preamble: None,
                approved_plan: None,
                artifact: &diff,
                comments: &[],
                ci_failures: ci,
                skills: &[],
            });
            assert_eq!(
                result.text, base.text,
                "empty/whitespace ci_failures ({ci:?}) must equal the no-CI output"
            );
            assert_eq!(result.hash, base.hash);
        }
    }
}
