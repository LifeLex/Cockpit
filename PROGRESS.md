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
