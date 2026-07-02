//! Deterministic diff-derived review signals: size class, change risk paths,
//! test delta, and test-weakening flags.
//!
//! Everything here is a **pure** function of a unified diff string. No I/O, no
//! git, no network — the engine walks the diff once and classifies lines with
//! documented heuristics. The output ([`DiffSignals`]) is what the diff gate
//! surfaces to the reviewer so they can spend attention where it matters.
//!
//! Line/side on each [`WeakeningFlag`] is a jump target: additions carry their
//! New-side line number, deletions carry their Old-side line number, matching
//! the diff-side convention used elsewhere in the crate (see
//! `adapters::github::validate_comment_in_diff` for the same hunk-walking shape,
//! reimplemented locally here to avoid coupling to the adapter).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::{CiSummary, DiffSide};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Coarse size bucket for a diff, by total changed lines (additions +
/// deletions): `S` < 50, `M` < 200, `L` < 600, `Xl` >= 600.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub enum SizeClass {
    /// Small: fewer than 50 changed lines.
    S,
    /// Medium: 50..200 changed lines.
    M,
    /// Large: 200..600 changed lines.
    L,
    /// Extra-large: 600 or more changed lines.
    Xl,
}

/// A category of change that warrants extra reviewer scrutiny.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub enum RiskFlag {
    /// A database/schema migration (`migration`/`migrations/` in path, or `.sql`).
    Migration,
    /// A dependency lockfile (machine-generated churn).
    Lockfile,
    /// A CI pipeline definition (workflows, `ci.yml`, GitLab CI, Jenkinsfile).
    CiConfig,
    /// Authentication / secret material (`auth`, `credential`, `secret`, `token`).
    Auth,
    /// Any other file under `.github/` not already classified as [`RiskFlag::CiConfig`].
    GithubDir,
    /// A package manifest whose declared dependencies may have changed.
    Dependency,
}

/// A single file flagged as risky, paired with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct RiskPath {
    /// Why this path is risky.
    pub flag: RiskFlag,
    /// Repo-relative path of the flagged file.
    pub path: PathBuf,
}

/// How the diff moved the test surface: files touched and assertion churn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct TestDelta {
    /// Number of distinct test files the diff touched.
    pub test_files_changed: u32,
    /// Assertion-bearing lines added across the diff.
    pub assertions_added: u32,
    /// Assertion-bearing lines removed across the diff.
    pub assertions_removed: u32,
}

/// The kind of test-weakening a [`WeakeningFlag`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub enum WeakeningKind {
    /// An assertion line was removed from a test file.
    DeletedAssertion,
    /// An `#[ignore]` attribute was added.
    IgnoreAdded,
    /// A `|| true` short-circuit was added (silences a failing command).
    OrTrue,
    /// A skip/todo/focus marker was added (`.skip`, `.todo`, `.only`, `xit`, …).
    SkipOrTodo,
    /// A test function opener was removed from a test file.
    DeletedTestFn,
    /// A whole test file was deleted (pure deletions to `/dev/null`).
    DeletedTestFile,
    /// A snapshot file was substantially rewritten (large removal).
    SnapshotRewrite,
}

/// A single suspected test-weakening, with a jump target into the diff.
///
/// `line`/`side` point at the offending line: the New side for additions
/// (e.g. [`WeakeningKind::IgnoreAdded`]), the Old side for deletions
/// (e.g. [`WeakeningKind::DeletedAssertion`]). File-level flags
/// ([`WeakeningKind::DeletedTestFile`], [`WeakeningKind::SnapshotRewrite`])
/// point at the first removed line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct WeakeningFlag {
    /// What kind of weakening this is.
    pub kind: WeakeningKind,
    /// Repo-relative path of the file.
    pub path: PathBuf,
    /// Line number of the offending line on [`Self::side`].
    pub line: u32,
    /// Which side of the diff [`Self::line`] refers to.
    pub side: DiffSide,
    /// The trimmed offending line, capped at ~120 characters.
    pub excerpt: String,
}

/// The full deterministic signal set for one diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct DiffSignals {
    /// Total added lines.
    pub additions: u32,
    /// Total removed lines.
    pub deletions: u32,
    /// Number of files the diff touches (one per `diff --git` header).
    pub files_changed: u32,
    /// Size bucket derived from additions + deletions.
    pub size_class: SizeClass,
    /// Test-surface movement.
    pub test_delta: TestDelta,
    /// Risky files, at most one flag per file.
    pub risk_paths: Vec<RiskPath>,
    /// Suspected test-weakening flags.
    pub weakening: Vec<WeakeningFlag>,
}

/// A single command an agent ran during its work, paired with its outcome.
///
/// Defined here in the evidence module so [`EvidenceSummary`] can carry it now;
/// Phase D's trajectory module reuses this type to summarize what an agent
/// executed (there is no separate trajectory module yet).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct CommandRun {
    /// The command line the agent ran.
    pub command: String,
    /// Whether the command exited successfully.
    pub ok: bool,
}

/// The review-time evidence bundle: deterministic diff signals, the CI rollup,
/// and the commands the agent ran.
///
/// Assembled per review so the diff gate can show, in one place, what changed
/// (the [`DiffSignals`]), whether CI is green (the [`CiSummary`]), and what the
/// agent actually executed (the [`CommandRun`]s).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct EvidenceSummary {
    /// Deterministic diff-derived signals for the review.
    pub signals: DiffSignals,
    /// Rolled-up CI status, when a CI check has populated it.
    pub ci: Option<CiSummary>,
    /// Commands the agent ran; empty until Phase D fills it from the trajectory.
    pub agent_ran: Vec<CommandRun>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Compute [`DiffSignals`] from a unified diff in a single pass.
///
/// The diff is expected in `git diff` format: each file preceded by a
/// `diff --git a/… b/…` header, `--- `/`+++ ` path headers, and
/// `@@ -a,b +c,d @@` hunk headers (omitted counts default to 1). Old/new line
/// numbers are tracked per hunk so weakening flags carry a real jump target.
///
/// Assertion counting is content-based across *all* files (not just test
/// files): the [`is_assertion_line`] heuristic is purely textual, matching the
/// literal signal definition. Weakening flags that would create refactor noise
/// (deleted assertions / deleted test functions) are restricted to test files;
/// see [`removed_weakening_kind`].
pub fn compute_diff_signals(diff: &str) -> DiffSignals {
    let mut builder = SignalBuilder::default();

    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            builder.start_file();
            continue;
        }

        // Path headers only appear before the first hunk. Gating on `!in_hunk`
        // disambiguates them from removed lines like `--- foo` inside a hunk.
        if !builder.in_hunk {
            if let Some(value) = line.strip_prefix("--- ") {
                builder.old_path = parse_file_header_path(value);
                continue;
            }
            if let Some(value) = line.strip_prefix("+++ ") {
                builder.new_path = parse_file_header_path(value);
                builder.new_is_dev_null = builder.new_path.is_none();
                builder.classify_file();
                continue;
            }
        }

        if let Some((old_start, new_start)) = parse_hunk_header(line) {
            builder.old_line = old_start;
            builder.new_line = new_start;
            builder.in_hunk = true;
            continue;
        }

        if !builder.in_hunk {
            // Preamble lines (`index …`, mode changes, binary markers).
            continue;
        }

        if let Some(content) = line.strip_prefix('+') {
            builder.add_line(content);
            builder.new_line += 1;
        } else if let Some(content) = line.strip_prefix('-') {
            builder.remove_line(content);
            builder.old_line += 1;
        } else if line.starts_with('\\') {
            // `\ No newline at end of file` — a marker, not a real line.
        } else {
            // Context line (leading space, or a stray empty line): both advance.
            builder.old_line += 1;
            builder.new_line += 1;
        }
    }

    builder.finish()
}

// ---------------------------------------------------------------------------
// Single-pass builder
// ---------------------------------------------------------------------------

/// Minimum removed lines for a `.snap` rewrite to count as [`WeakeningKind::SnapshotRewrite`].
const SNAPSHOT_MIN_REMOVED: u32 = 30;

/// Accumulates signals across one walk of a diff. Per-file fields reset on each
/// `diff --git`; global counters persist.
#[derive(Default)]
struct SignalBuilder {
    // Global counters.
    additions: u32,
    deletions: u32,
    files_changed: u32,
    test_files_changed: u32,
    assertions_added: u32,
    assertions_removed: u32,
    risk_paths: Vec<RiskPath>,
    weakening: Vec<WeakeningFlag>,

    // Current-file state.
    file_open: bool,
    old_path: Option<String>,
    new_path: Option<String>,
    canonical: Option<PathBuf>,
    is_test: bool,
    is_snapshot: bool,
    new_is_dev_null: bool,
    file_added: u32,
    file_removed: u32,
    first_removed_old_line: Option<u32>,
    /// Per-line weakening buffered so a whole-file flag can supersede it.
    file_weakening: Vec<WeakeningFlag>,

    // Current-hunk state.
    in_hunk: bool,
    old_line: u32,
    new_line: u32,
}

impl SignalBuilder {
    /// Begin a new file: flush the previous one, then reset per-file state.
    fn start_file(&mut self) {
        self.flush_file();
        self.file_open = true;
        self.files_changed += 1;
        self.old_path = None;
        self.new_path = None;
        self.canonical = None;
        self.is_test = false;
        self.is_snapshot = false;
        self.new_is_dev_null = false;
        self.file_added = 0;
        self.file_removed = 0;
        self.first_removed_old_line = None;
        self.file_weakening.clear();
        self.in_hunk = false;
    }

    /// Classify the current file once both path headers are known: whether it
    /// is a test/snapshot file and which single risk flag (if any) it carries.
    fn classify_file(&mut self) {
        // Prefer the new-side path; fall back to the old side for deletions
        // whose new side is `/dev/null`.
        let Some(path) = self.new_path.clone().or_else(|| self.old_path.clone()) else {
            return;
        };
        self.is_test = is_test_file(&path);
        self.is_snapshot = is_snapshot_file(&path);
        if self.is_test {
            self.test_files_changed += 1;
        }
        if let Some(flag) = classify_risk(&path) {
            self.risk_paths.push(RiskPath {
                flag,
                path: PathBuf::from(&path),
            });
        }
        self.canonical = Some(PathBuf::from(&path));
    }

    /// Record an added line (content already stripped of its `+`).
    fn add_line(&mut self, content: &str) {
        self.additions += 1;
        self.file_added += 1;
        if is_assertion_line(content) {
            self.assertions_added += 1;
        }
        if let Some(kind) = added_weakening_kind(content) {
            self.file_weakening.push(WeakeningFlag {
                kind,
                path: self.canonical.clone().unwrap_or_default(),
                line: self.new_line,
                side: DiffSide::New,
                excerpt: make_excerpt(content),
            });
        }
    }

    /// Record a removed line (content already stripped of its `-`).
    fn remove_line(&mut self, content: &str) {
        self.deletions += 1;
        self.file_removed += 1;
        if self.first_removed_old_line.is_none() {
            self.first_removed_old_line = Some(self.old_line);
        }
        if is_assertion_line(content) {
            self.assertions_removed += 1;
        }
        if let Some(kind) = removed_weakening_kind(content, self.is_test, self.is_snapshot) {
            self.file_weakening.push(WeakeningFlag {
                kind,
                path: self.canonical.clone().unwrap_or_default(),
                line: self.old_line,
                side: DiffSide::Old,
                excerpt: make_excerpt(content),
            });
        }
    }

    /// Finalize the current file: emit a whole-file weakening flag if one
    /// applies (which supersedes the per-line flags to avoid noise), otherwise
    /// flush the buffered per-line flags.
    fn flush_file(&mut self) {
        if !self.file_open {
            return;
        }
        self.file_open = false;

        if let Some(path) = self.canonical.clone() {
            // Deleted test file: a test file that is pure deletions to /dev/null.
            if self.is_test && self.new_is_dev_null && self.file_added == 0 && self.file_removed > 0
            {
                self.weakening.push(WeakeningFlag {
                    kind: WeakeningKind::DeletedTestFile,
                    line: self.first_removed_old_line.unwrap_or(1),
                    side: DiffSide::Old,
                    excerpt: make_excerpt(&path.display().to_string()),
                    path,
                });
                self.file_weakening.clear();
                return;
            }
            // Snapshot rewrite: a large removal in a snapshot file, where the
            // removed count is at least the threshold and at least double the
            // added count (i.e. content was discarded, not merely regenerated).
            if self.is_snapshot
                && self.file_removed >= SNAPSHOT_MIN_REMOVED
                && self.file_removed >= self.file_added.saturating_mul(2)
            {
                self.weakening.push(WeakeningFlag {
                    kind: WeakeningKind::SnapshotRewrite,
                    line: self.first_removed_old_line.unwrap_or(1),
                    side: DiffSide::Old,
                    excerpt: make_excerpt(&path.display().to_string()),
                    path,
                });
                self.file_weakening.clear();
                return;
            }
        }

        self.weakening.append(&mut self.file_weakening);
    }

    /// Consume the builder and produce the final signals.
    fn finish(mut self) -> DiffSignals {
        self.flush_file();
        let total = self.additions.saturating_add(self.deletions);
        DiffSignals {
            additions: self.additions,
            deletions: self.deletions,
            files_changed: self.files_changed,
            size_class: classify_size(total),
            test_delta: TestDelta {
                test_files_changed: self.test_files_changed,
                assertions_added: self.assertions_added,
                assertions_removed: self.assertions_removed,
            },
            risk_paths: self.risk_paths,
            weakening: self.weakening,
        }
    }
}

// ---------------------------------------------------------------------------
// Heuristics
// ---------------------------------------------------------------------------

/// Bucket a total changed-line count: `S` < 50, `M` < 200, `L` < 600, else `Xl`.
fn classify_size(total: u32) -> SizeClass {
    match total {
        0..=49 => SizeClass::S,
        50..=199 => SizeClass::M,
        200..=599 => SizeClass::L,
        _ => SizeClass::Xl,
    }
}

/// A path is a test file when it contains `test` (covers `tests/`, `__tests__`,
/// `*_test.rs`, `*.test.ts(x)`) or a `.spec.` segment (`*.spec.*`). The `test`
/// substring is intentionally broad per the signal definition; it can match
/// non-test paths that happen to contain "test", which is accepted as a
/// low-cost false positive.
fn is_test_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("test") || lower.contains(".spec.")
}

/// A path is a snapshot file when it ends in `.snap` or lives under a
/// `__snapshots__` directory.
fn is_snapshot_file(path: &str) -> bool {
    path.ends_with(".snap") || path.contains("__snapshots__")
}

/// A changed line asserts when it mentions `assert` (Rust `assert!`/`assert_eq!`/
/// `assert_ne!`/`debug_assert*`), `expect(` (TS/vitest/jest), or a `.toBe`/
/// `.toEqual` matcher.
fn is_assertion_line(content: &str) -> bool {
    content.contains("assert")
        || content.contains("expect(")
        || content.contains(".toBe")
        || content.contains(".toEqual")
}

/// The single risk flag for a path, in priority order (each file flags once).
///
/// `Migration` and `Lockfile` outrank `CiConfig`, which outranks `Auth`, which
/// outranks the catch-all `GithubDir`, which outranks `Dependency`. The order
/// resolves overlaps (e.g. `.github/workflows/ci.yml` is `CiConfig`, not
/// `GithubDir`).
fn classify_risk(path: &str) -> Option<RiskFlag> {
    let lower = path.to_ascii_lowercase();
    let name = file_name(path);

    if lower.contains("migration") || lower.ends_with(".sql") {
        return Some(RiskFlag::Migration);
    }

    const LOCKFILES: [&str; 6] = [
        "Cargo.lock",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "uv.lock",
        "poetry.lock",
    ];
    if LOCKFILES.contains(&name) {
        return Some(RiskFlag::Lockfile);
    }

    if lower.contains(".github/workflows/")
        || name == "ci.yml"
        || lower.contains(".gitlab-ci")
        || name == "Jenkinsfile"
    {
        return Some(RiskFlag::CiConfig);
    }

    if has_auth_indicator(path) {
        return Some(RiskFlag::Auth);
    }

    if lower.contains(".github/") {
        return Some(RiskFlag::GithubDir);
    }

    if name == "Cargo.toml" || name == "package.json" {
        return Some(RiskFlag::Dependency);
    }

    None
}

/// Whether a path names auth/secret material.
///
/// `credential`, `secret`, and `token` match as plain (case-insensitive)
/// substrings — they have no notable collision words, so plurals and camelCase
/// are covered. `auth` is matched unless immediately followed by `o`, which
/// filters the `autho…` stem (`author`, `authored`, `authorize`) while keeping
/// `auth.rs`, `auth_token`, `authService`, and `authentication`. As a
/// documented limitation, `authorization`/`authorize` share that stem and so do
/// not flag.
fn has_auth_indicator(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.contains("credential") || lower.contains("secret") || lower.contains("token") {
        return true;
    }
    for (idx, _) in lower.match_indices("auth") {
        if lower.as_bytes().get(idx + 4).copied() != Some(b'o') {
            return true;
        }
    }
    false
}

/// The weakening kind for an added line, if any (first match wins).
fn added_weakening_kind(content: &str) -> Option<WeakeningKind> {
    if content.contains("#[ignore") {
        return Some(WeakeningKind::IgnoreAdded);
    }
    if content.contains("|| true") {
        return Some(WeakeningKind::OrTrue);
    }
    if is_skip_or_todo(content) {
        return Some(WeakeningKind::SkipOrTodo);
    }
    None
}

/// Skip/todo/focus markers. `.only(` is included: focusing a single test
/// silently skips the rest of the suite, which weakens it.
fn is_skip_or_todo(content: &str) -> bool {
    const MARKERS: [&str; 7] = [
        ".skip(",
        ".todo(",
        "it.skip",
        "describe.skip",
        "xit(",
        "xdescribe(",
        ".only(",
    ];
    MARKERS.iter().any(|marker| content.contains(marker))
}

/// The weakening kind for a removed line, if any.
///
/// Restricted to non-snapshot test files: removing an assertion or a test
/// function opener only counts as weakening inside a hand-written test file, so
/// ordinary production refactors do not generate noise.
fn removed_weakening_kind(
    content: &str,
    is_test: bool,
    is_snapshot: bool,
) -> Option<WeakeningKind> {
    if !is_test || is_snapshot {
        return None;
    }
    if is_test_fn_opening(content.trim()) {
        return Some(WeakeningKind::DeletedTestFn);
    }
    if is_assertion_line(content) {
        return Some(WeakeningKind::DeletedAssertion);
    }
    None
}

/// Whether a trimmed line opens a test: a Rust `#[test]` attribute, a
/// `fn test_…` opener, or a JS `it(`/`test(` opener.
fn is_test_fn_opening(trimmed: &str) -> bool {
    trimmed.contains("#[test]")
        || trimmed.starts_with("fn test_")
        || trimmed.starts_with("it(")
        || trimmed.starts_with("test(")
}

/// Trim a line and cap it at ~120 characters for use as a flag excerpt.
fn make_excerpt(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.chars().count() <= 120 {
        trimmed.to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

/// The final path component (basename), or the whole path if it has no `/`.
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

// ---------------------------------------------------------------------------
// Local unified-diff header parsing
// ---------------------------------------------------------------------------

/// Extract the repo-relative path from a `---`/`+++` header value, stripping the
/// `a/`/`b/` prefix and trailing tab metadata. Returns `None` for `/dev/null`.
fn parse_file_header_path(value: &str) -> Option<String> {
    let token = value.split('\t').next().unwrap_or(value).trim();
    if token == "/dev/null" {
        return None;
    }
    let stripped = token
        .strip_prefix("a/")
        .or_else(|| token.strip_prefix("b/"))
        .unwrap_or(token);
    Some(stripped.to_string())
}

/// Parse a hunk header (`@@ -a,b +c,d @@`) into `(old_start, new_start)`.
///
/// Returns `None` for any non-hunk line. Counts are ignored here (line tracking
/// derives lengths from the body), so only the two start numbers are returned.
fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("@@ ")?;
    let close = rest.find(" @@")?;
    let spec = &rest[..close];

    let mut parts = spec.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    Some((parse_hunk_start(old)?, parse_hunk_start(new)?))
}

/// Parse the start line from a hunk range component (`a,b` or `a`).
fn parse_hunk_start(component: &str) -> Option<u32> {
    let start = match component.split_once(',') {
        Some((start, _)) => start,
        None => component,
    };
    start.parse().ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A diff that adds `n` lines to a plain (non-test) file.
    fn diff_adding(n: u32) -> String {
        let mut s = String::new();
        s.push_str("diff --git a/data.txt b/data.txt\n");
        s.push_str("--- a/data.txt\n");
        s.push_str("+++ b/data.txt\n");
        s.push_str(&format!("@@ -0,0 +1,{n} @@\n"));
        for i in 0..n {
            s.push_str(&format!("+row {i}\n"));
        }
        s
    }

    /// A minimal diff that touches (one context + one added line) `path`.
    fn touch(path: &str) -> String {
        format!(
            "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1,1 +1,2 @@\n unchanged\n+changed\n"
        )
    }

    #[test]
    fn size_class_boundary_s_m() {
        assert_eq!(
            compute_diff_signals(&diff_adding(49)).size_class,
            SizeClass::S
        );
        let m = compute_diff_signals(&diff_adding(50));
        assert_eq!(m.size_class, SizeClass::M);
        assert_eq!(m.additions, 50);
        assert_eq!(m.deletions, 0);
    }

    #[test]
    fn size_class_boundary_m_l() {
        assert_eq!(
            compute_diff_signals(&diff_adding(199)).size_class,
            SizeClass::M
        );
        assert_eq!(
            compute_diff_signals(&diff_adding(200)).size_class,
            SizeClass::L
        );
    }

    #[test]
    fn size_class_boundary_l_xl() {
        assert_eq!(
            compute_diff_signals(&diff_adding(599)).size_class,
            SizeClass::L
        );
        assert_eq!(
            compute_diff_signals(&diff_adding(600)).size_class,
            SizeClass::Xl
        );
    }

    #[test]
    fn risk_flags_by_path() {
        let cases: [(&str, RiskFlag); 6] = [
            ("db/migrations/001_init.sql", RiskFlag::Migration),
            ("Cargo.lock", RiskFlag::Lockfile),
            (".github/workflows/ci.yml", RiskFlag::CiConfig),
            ("src/auth.rs", RiskFlag::Auth),
            (".github/dependabot.yml", RiskFlag::GithubDir),
            ("Cargo.toml", RiskFlag::Dependency),
        ];
        for (path, expected) in cases {
            let signals = compute_diff_signals(&touch(path));
            assert_eq!(
                signals.risk_paths.len(),
                1,
                "path {path} should yield exactly one risk flag"
            );
            assert_eq!(signals.risk_paths[0].flag, expected, "flag for {path}");
            assert_eq!(signals.risk_paths[0].path, PathBuf::from(path));
        }
    }

    #[test]
    fn author_rs_does_not_flag_auth() {
        let signals = compute_diff_signals(&touch("src/author.rs"));
        assert!(
            signals.risk_paths.is_empty(),
            "author.rs must not flag Auth (the `autho` stem is excluded)"
        );
    }

    #[test]
    fn test_delta_counts_assertions_and_files() {
        let diff = concat!(
            "diff --git a/tests/alpha.rs b/tests/alpha.rs\n",
            "--- a/tests/alpha.rs\n",
            "+++ b/tests/alpha.rs\n",
            "@@ -1,4 +1,5 @@\n",
            " fn setup() {}\n",
            "+    assert_eq!(1, 1);\n",
            "+    assert!(ok());\n",
            "-    assert_ne!(2, 3);\n",
            " fn teardown() {}\n",
            "diff --git a/src/widget.test.ts b/src/widget.test.ts\n",
            "--- a/src/widget.test.ts\n",
            "+++ b/src/widget.test.ts\n",
            "@@ -1,2 +1,3 @@\n",
            " describe('w', () => {});\n",
            "+  expect(v).toBe(1);\n",
            " done();\n",
        );
        let s = compute_diff_signals(diff);
        assert_eq!(s.test_delta.test_files_changed, 2);
        assert_eq!(s.test_delta.assertions_added, 3);
        assert_eq!(s.test_delta.assertions_removed, 1);
    }

    #[test]
    fn weakening_deleted_assertion() {
        let diff = concat!(
            "diff --git a/tests/foo.rs b/tests/foo.rs\n",
            "--- a/tests/foo.rs\n",
            "+++ b/tests/foo.rs\n",
            "@@ -1,4 +1,3 @@\n",
            " fn test_thing() {\n",
            "     let x = compute();\n",
            "-    assert_eq!(x, 42);\n",
            " }\n",
        );
        let s = compute_diff_signals(diff);
        assert_eq!(s.weakening.len(), 1);
        let w = &s.weakening[0];
        assert_eq!(w.kind, WeakeningKind::DeletedAssertion);
        assert_eq!(w.side, DiffSide::Old);
        assert_eq!(w.line, 3);
        assert_eq!(w.path, PathBuf::from("tests/foo.rs"));
        assert_eq!(w.excerpt, "assert_eq!(x, 42);");
    }

    #[test]
    fn weakening_ignore_added() {
        let diff = concat!(
            "diff --git a/src/lib.rs b/src/lib.rs\n",
            "--- a/src/lib.rs\n",
            "+++ b/src/lib.rs\n",
            "@@ -10,2 +10,3 @@\n",
            " mod m {\n",
            "+#[ignore]\n",
            " }\n",
        );
        let s = compute_diff_signals(diff);
        assert_eq!(s.weakening.len(), 1);
        let w = &s.weakening[0];
        assert_eq!(w.kind, WeakeningKind::IgnoreAdded);
        assert_eq!(w.side, DiffSide::New);
        assert_eq!(w.line, 11);
        assert_eq!(w.excerpt, "#[ignore]");
    }

    #[test]
    fn weakening_or_true() {
        let diff = concat!(
            "diff --git a/scripts/run.sh b/scripts/run.sh\n",
            "--- a/scripts/run.sh\n",
            "+++ b/scripts/run.sh\n",
            "@@ -1,2 +1,3 @@\n",
            " #!/bin/sh\n",
            "+cargo check || true\n",
            " echo ok\n",
        );
        let s = compute_diff_signals(diff);
        assert_eq!(s.weakening.len(), 1);
        let w = &s.weakening[0];
        assert_eq!(w.kind, WeakeningKind::OrTrue);
        assert_eq!(w.side, DiffSide::New);
        assert_eq!(w.line, 2);
        assert_eq!(w.excerpt, "cargo check || true");
    }

    #[test]
    fn weakening_skip_todo_and_only() {
        let skip = concat!(
            "diff --git a/src/a.test.ts b/src/a.test.ts\n",
            "--- a/src/a.test.ts\n",
            "+++ b/src/a.test.ts\n",
            "@@ -1,2 +1,3 @@\n",
            " describe('a', () => {\n",
            "+  it.skip('wip', () => {});\n",
            " });\n",
        );
        let s = compute_diff_signals(skip);
        assert_eq!(s.weakening.len(), 1);
        assert_eq!(s.weakening[0].kind, WeakeningKind::SkipOrTodo);
        assert_eq!(s.weakening[0].side, DiffSide::New);
        assert_eq!(s.weakening[0].line, 2);

        let only = concat!(
            "diff --git a/src/b.test.ts b/src/b.test.ts\n",
            "--- a/src/b.test.ts\n",
            "+++ b/src/b.test.ts\n",
            "@@ -1,2 +1,3 @@\n",
            " describe('b', () => {\n",
            "+  it.only('focus', () => {});\n",
            " });\n",
        );
        let s2 = compute_diff_signals(only);
        assert_eq!(s2.weakening.len(), 1);
        assert_eq!(s2.weakening[0].kind, WeakeningKind::SkipOrTodo);
    }

    #[test]
    fn weakening_deleted_test_fn() {
        let diff = concat!(
            "diff --git a/tests/bar.rs b/tests/bar.rs\n",
            "--- a/tests/bar.rs\n",
            "+++ b/tests/bar.rs\n",
            "@@ -1,4 +1,3 @@\n",
            " fn helper() {}\n",
            " mod inner {}\n",
            "-fn test_old() {}\n",
            " fn other() {}\n",
        );
        let s = compute_diff_signals(diff);
        assert_eq!(s.weakening.len(), 1);
        let w = &s.weakening[0];
        assert_eq!(w.kind, WeakeningKind::DeletedTestFn);
        assert_eq!(w.side, DiffSide::Old);
        assert_eq!(w.line, 3);
        assert_eq!(w.excerpt, "fn test_old() {}");
    }

    #[test]
    fn weakening_deleted_test_file() {
        let diff = concat!(
            "diff --git a/tests/gone.rs b/tests/gone.rs\n",
            "deleted file mode 100644\n",
            "index 89abcde..0000000\n",
            "--- a/tests/gone.rs\n",
            "+++ /dev/null\n",
            "@@ -1,4 +0,0 @@\n",
            "-#[test]\n",
            "-fn test_a() {\n",
            "-    assert!(true);\n",
            "-}\n",
        );
        let s = compute_diff_signals(diff);
        assert_eq!(
            s.weakening.len(),
            1,
            "whole-file deletion collapses to a single flag"
        );
        let w = &s.weakening[0];
        assert_eq!(w.kind, WeakeningKind::DeletedTestFile);
        assert_eq!(w.side, DiffSide::Old);
        assert_eq!(w.line, 1);
        assert_eq!(w.path, PathBuf::from("tests/gone.rs"));
    }

    #[test]
    fn weakening_snapshot_rewrite() {
        // 30 removed, 5 added: >= threshold and >= 2x added -> fires.
        let mut big = String::new();
        big.push_str("diff --git a/src/__snapshots__/foo.snap b/src/__snapshots__/foo.snap\n");
        big.push_str("--- a/src/__snapshots__/foo.snap\n");
        big.push_str("+++ b/src/__snapshots__/foo.snap\n");
        big.push_str("@@ -1,30 +1,5 @@\n");
        for i in 0..30 {
            big.push_str(&format!("-old {i}\n"));
        }
        for i in 0..5 {
            big.push_str(&format!("+new {i}\n"));
        }
        let s = compute_diff_signals(&big);
        assert_eq!(s.weakening.len(), 1);
        assert_eq!(s.weakening[0].kind, WeakeningKind::SnapshotRewrite);
        assert_eq!(s.weakening[0].side, DiffSide::Old);

        // 10 removed: below the 30-line threshold -> no flag.
        let mut small = String::new();
        small.push_str("diff --git a/src/__snapshots__/bar.snap b/src/__snapshots__/bar.snap\n");
        small.push_str("--- a/src/__snapshots__/bar.snap\n");
        small.push_str("+++ b/src/__snapshots__/bar.snap\n");
        small.push_str("@@ -1,10 +1,1 @@\n");
        for i in 0..10 {
            small.push_str(&format!("-old {i}\n"));
        }
        small.push_str("+new 0\n");
        let s2 = compute_diff_signals(&small);
        assert!(
            s2.weakening.is_empty(),
            "10 removed is below the snapshot threshold"
        );
    }

    #[test]
    fn clean_refactor_has_no_weakening() {
        let diff = concat!(
            "diff --git a/src/util.rs b/src/util.rs\n",
            "--- a/src/util.rs\n",
            "+++ b/src/util.rs\n",
            "@@ -1,3 +1,3 @@\n",
            " fn add(a: i32, b: i32) -> i32 {\n",
            "-    a + b\n",
            "+    a.wrapping_add(b)\n",
            " }\n",
        );
        let s = compute_diff_signals(diff);
        assert!(s.weakening.is_empty());
        assert_eq!(s.additions, 1);
        assert_eq!(s.deletions, 1);
        assert_eq!(s.files_changed, 1);
        assert_eq!(s.size_class, SizeClass::S);
    }

    #[test]
    fn removed_assertion_in_non_test_file_is_not_weakening() {
        let diff = concat!(
            "diff --git a/src/main.rs b/src/main.rs\n",
            "--- a/src/main.rs\n",
            "+++ b/src/main.rs\n",
            "@@ -1,4 +1,3 @@\n",
            " fn main() {\n",
            "-    assert!(ready());\n",
            "     run();\n",
            " }\n",
        );
        let s = compute_diff_signals(diff);
        assert!(
            s.weakening.is_empty(),
            "removing an assertion outside a test file is not weakening"
        );
        // Counting is content-based across all files, so it still tallies.
        assert_eq!(s.test_delta.assertions_removed, 1);
        assert_eq!(s.test_delta.test_files_changed, 0);
    }

    #[test]
    fn empty_diff_is_zeroed() {
        let s = compute_diff_signals("");
        assert_eq!(s.additions, 0);
        assert_eq!(s.deletions, 0);
        assert_eq!(s.files_changed, 0);
        assert_eq!(s.size_class, SizeClass::S);
        assert_eq!(
            s.test_delta,
            TestDelta {
                test_files_changed: 0,
                assertions_added: 0,
                assertions_removed: 0,
            }
        );
        assert!(s.risk_paths.is_empty());
        assert!(s.weakening.is_empty());
    }
}
