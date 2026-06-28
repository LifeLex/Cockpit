# PROGRESS.md

## T0.2 — Domain model + newtypes

**Status:** Complete

**What was done:**
- Created `crates/cockpit-core/src/model.rs` with all domain types from SPEC.md §6.
- Newtypes: `ReviewId`, `IssueRef`, `PrRef`, `CommentId`, `ProjectRef` — all distinct types
  wrapping `String`, with `new()`, `as_str()`, and `Display`.
- Enums: `GateState`, `AgentMode`, `CommentOrigin`, `Anchor`, `Artifact`.
- Structs: `PlanStep`, `PlanDoc`, `DiffData`, `Comment`, `AgentRun`, `ProjectPlan`, `Review`.
- Wired `pub mod model;` in `lib.rs`.

**Test results:**
- 7 tests pass (6 new in model + 1 existing smoke).
- `cargo fmt` clean, `cargo clippy -- -D warnings` clean, `cargo test --all` green.

**Decisions:**
- Used `SystemTime` instead of `Instant` for `AgentRun.started_at` because `Instant` is not
  serializable and has no cross-process meaning. Documented inline.
- `stale: bool` on `Review` kept as boolean per SPEC.md §6 — it is genuinely binary (an
  ancestor is or is not in rework) and the spec is explicit.
- `DiffData` is a placeholder with `raw: String` — will be fleshed out in T0.5/T4.3.
- Deferred `uuid` dependency for ID generation to the task that first needs runtime IDs.
- Used a `newtype_id!` macro for the five ID types since they are structurally identical.
- Added `Eq` to all types whose fields support it, per CLAUDE.md §2 "derive eagerly."

## T0.6 — Git adapter (worktrees)

**Status:** Complete

**What was done:**
- Created `crates/cockpit-core/src/adapters/mod.rs` and `adapters/git.rs`.
- Implemented `ensure_worktree` (creates worktree with branch from base, supports stacked bases),
  `reconcile` (re-reads HEAD sha after agent work), `prune_worktree` (removes worktree on Merged).
- `restack` stubbed as `Error::RestackNotImplemented` for Phase 3.
- `git::Error` with thiserror: WorktreeExists, BranchNotFound, DetachedHead,
  RestackNotImplemented, RebaseConflict, Git2(#[from]).
- Added `git2` and `tempfile` as workspace dependencies.

**Test results:**
- 11 new tests (18 total). All pass against real scratch git repos (no mocks).
- Tests cover: create worktree, stacked base, already-exists error, bad base error,
  reconcile reads new HEAD, reconcile nonexistent path, prune removes worktree,
  prune preserves branch, restack stub, multiple worktrees, correct branch creation.
- `cargo fmt` clean, `cargo clippy -- -D warnings` clean, `cargo test --all` green.

**Decisions:**
- Sync API — git2 is inherently synchronous; callers wrap in `spawn_blocking` if needed.
- Worktree name matches branch name for consistent lookup in prune.
- `prune_worktree` does not delete the branch — the branch may still be needed for the
  merged PR's reference.
- Used `git2` native worktree API with `WorktreeAddOptions::reference()` rather than
  shelling out to `git worktree add`.
