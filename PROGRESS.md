# PROGRESS.md

## SUMMARY

All four assigned tasks are **complete** with no blockers.

| Task | Status | Branch | Tests | Total tests on branch |
|------|--------|--------|-------|-----------------------|
| T0.2 — Domain model + newtypes | Complete | `alejandro/t0.2-domain-model` | 7 new | 8 |
| T0.3 — Gated trait + state machine | Complete | `alejandro/t0.3-gated-trait` | 34 new | 42 |
| T0.6 — Git adapter (worktrees) | Complete | `alejandro/t0.6-git-adapter` | 11 new | 19 |
| T1.1 — Deterministic prompt assembly | Complete | `alejandro/t1.1-prompt-assembly` | 9 new | 16 |

**Branch topology:** T0.3, T0.6, and T1.1 each branch from `alejandro/t0.2-domain-model`.
Each branch passes `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test --all`.

**No blockers.** No tasks required live credentials, running external services, or human
decisions that SPEC.md leaves open.

---

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

## T0.3 — Gated trait + state transitions

**Status:** Complete (on branch `alejandro/t0.3-gated-trait`)

**What was done:**
- Created `crates/cockpit-core/src/gate.rs` with the `Gated` trait and all state transitions.
- Pure state transitions as default methods: `open`, `request_changes`, `approve`,
  `mark_reworked`, `mark_agent_failed`.
- Effectful methods (`dispatch`, `reconcile`) are stubs returning `Error::NotImplemented`.
- `Gated` implemented for both `Review` and `ProjectPlan` — one loop, written once.
- Stale flag logic as inherent methods on `Review` (`mark_stale`, `clear_stale`).

**Test results:**
- 34 new tests. Every legal transition (6), every illegal transition (19), edge cases
  (no-comments, comments cleared on reworked, comments preserved on agent failure, full
  cycle, retry after failure), stale flag, and ProjectPlan full cycle.

**Decisions:**
- Separated pure state transitions from effectful dispatch/reconcile.
- `mark_agent_failed` preserves comments; `mark_reworked` clears them (Invariant 4).
- Stale logic on `Review` directly, not on `Gated` (Review-specific per SPEC.md §7).

## T0.6 — Git adapter (worktrees)

**Status:** Complete (on branch `alejandro/t0.6-git-adapter`)

**What was done:**
- Created `crates/cockpit-core/src/adapters/git.rs` with `ensure_worktree`, `reconcile`,
  `prune_worktree`. Restack stubbed for Phase 3.
- Added `git2` and `tempfile` as workspace dependencies.

**Test results:**
- 11 new tests against real scratch git repos (no mocks). Cover create, stacked base,
  already-exists error, bad base, reconcile after commit, prune removes worktree,
  prune preserves branch, multiple worktrees, correct branch creation.

**Decisions:**
- Sync API; callers wrap in `spawn_blocking` if needed.
- Worktree name matches branch name for consistent lookup.
- `prune_worktree` does not delete the branch.

## T1.1 — Deterministic prompt assembly

**Status:** Complete

**What was done:**
- Created `crates/cockpit-core/src/prompt.rs` with rework prompt assembly per SPEC.md §9.
- `ReworkInput` struct bundles intent, optional approved plan, artifact, and comments.
- `AssembledPrompt` struct returns the full text and its SHA-256 hash.
- `assemble_rework` produces prompts in fixed section order: Intent → Approved Plan
  (diff gate only) → Current Artifact → Comments with anchors → Scope Guard.
- `render_anchor` handles all three Anchor variants, enriches PlanStep with title
  when PlanDoc is available.
- Scope guard verbatim from SPEC.md §9, including test-weakening clause.
- Added `sha2` as workspace dependency for prompt hashing.
- Three golden files lock down exact output for diff-gate, plan-gate, and no-comments cases.

**Test results:**
- 9 new tests (16 total). All pass.
- Three golden-file tests compare exact prompt output byte-for-byte.
- Hash determinism and content-sensitivity tested.
- Anchor rendering tested for all variants including title enrichment.
- `cargo fmt` clean, `cargo clippy -- -D warnings` clean, `cargo test --all` green.

**Decisions:**
- Used `writeln!` into String (infallible) with `// INVARIANT:` comments per CLAUDE.md §2.
- The prompt includes `doc.raw` for plan artifacts and `data.raw` for diffs — raw content,
  not re-rendered from structured fields.
- Zero-comment case handled gracefully (shows "No comments.") — enforcement of ≥1 comment
  is the gate's job (T0.3), not the prompt assembler's.
- PlanStep anchors enriched with title when PlanDoc context is available; falls back to
  bare index when out of bounds or no doc context.
