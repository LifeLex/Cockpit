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

## T0.3 — Gated trait + state transitions

**Status:** Complete

**What was done:**
- Created `crates/cockpit-core/src/gate.rs` with the `Gated` trait and all state transitions.
- Pure state transitions as default methods: `open`, `request_changes`, `approve`,
  `mark_reworked`, `mark_agent_failed`.
- Effectful methods (`dispatch`, `reconcile`) are stubs returning `Error::NotImplemented`.
- `Gated` implemented for both `Review` and `ProjectPlan` — one loop, written once.
- Stale flag logic as inherent methods on `Review` (`mark_stale`, `clear_stale`).
- `gate::Error` with `thiserror`: `IllegalTransition`, `NoComments`, `NotImplemented`.

**Test results:**
- 34 new tests (41 total). All pass.
- Every legal transition tested (6 transitions from SPEC.md §7).
- Every illegal transition tested (19 invalid from/event pairs).
- Edge cases: no-comments rejected, comments cleared on reworked, comments preserved on
  agent failure, full cycle, agent-failed-then-redispatch, stale flag orthogonal to loop.
- `ProjectPlan` full cycle verifies same trait, no forking.

**Decisions:**
- Separated pure state transitions (default trait methods) from effectful dispatch/reconcile.
  This makes the state machine testable without real adapters.
- `mark_agent_failed` preserves comments — they are still pending feedback for re-dispatch.
- `mark_reworked` clears comments — enforces Invariant 4 (ephemeral).
- `gate_state_mut`/`comments_mut` are public trait methods (Rust traits can't have private
  methods) but documented as implementation details.
- Stale logic is on `Review` directly, not on `Gated`, because it's Review-specific per
  SPEC.md §7 ("stale gates the frontier, not the loop").
