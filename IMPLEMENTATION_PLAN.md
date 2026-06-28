# IMPLEMENTATION_PLAN.md — Cockpit

A task graph for building cockpit with the architect → coder → tester → reviewer pipeline.
Source of truth: `SPEC.md` (what) and `CLAUDE.md` (how). This file decomposes the SPEC's
phases into tasks sized at roughly one stacked PR each.

## How to run this plan

For every task:

1. **architect** drafts the approach against `SPEC.md` + `CLAUDE.md` (files, order, risks).
   This is the plan-gate step — get it approved before code.
2. **coder** implements only the approved task. No drive-by refactors, no scope creep.
3. **tester** writes/extends tests; for state-machine tasks, tests come first.
4. **reviewer** checks the diff against `CLAUDE.md` §6 (Definition of Done) and §0
   (Invariants). Advisory only; a human approves.

Tasks are stacked in dependency order (the same DAG model cockpit itself manages). A task is
mergeable only when its acceptance criteria pass. **Build in validation order, not runtime
order** — the shared loop is proven on the diff gate (Phase 1) before the plan gate reuses it.

Legend: `[deps: …]` lists prerequisite task IDs. Each task names its primary subagent owner.

---

## Phase 0 — Core skeleton + adapters (headless)

Goal: `cockpit project <id>` reads issues, builds the DAG, prints the frontier;
`cockpit ingest` lists existing PRs. No loop yet.

### T0.1 — Workspace + toolchain scaffold  ·  architect→coder  ·  [deps: —]
Create the Cargo workspace (`cockpit-core`, `cockpit-cli`), `rust-toolchain.toml` (edition
2024), CI running `fmt`, `clippy -D warnings`, `test`.
**Done when:** empty crates build and CI is green on all three checks.

### T0.2 — Domain model + newtypes  ·  coder  ·  [deps: T0.1]
Implement `model.rs`: `ReviewId`/`IssueRef`/`PrRef` newtypes, `Review`, `ProjectPlan`,
`GateState`, `Artifact`, `PlanDoc`, `Comment`, `Anchor`, `AgentRun`/`AgentMode` per `SPEC.md`
§6. Derive the required traits; comments ephemeral (no `resolved`, no durable sha).
**Done when:** types compile; a unit test constructs a small DAG of `Review`s and asserts the
parent/child edges.

### T0.3 — `Gated` trait + state transitions  ·  tester→coder  ·  [deps: T0.2]
Implement `gate.rs`: the `Gated` trait and every transition in `SPEC.md` §7 as explicit
functions, plus the `stale` flag logic. **Tests first.**
**Done when:** a test exercises every transition including failure edges (agent-failed →
InReview) and stale set/clear; illegal transitions are rejected.

### T0.4 — Linear adapter (read-only)  ·  coder  ·  [deps: T0.2]
`adapters/linear.rs`: read a project's issues + dependency relations via GraphQL; build the
DAG. `thiserror` error type.
**Done when:** given a project id, returns issues + edges; integration test against a fixture.

### T0.5 — GitHub adapter (gh shell-out) + branch linkage  ·  coder  ·  [deps: T0.2]
`adapters/github.rs`: `pr list --json`, `pr diff`, `pr checks` via `gh`. Parse the Linear
issue id out of each PR head branch to link PR → issue (per `SPEC.md` §16, settled).
**Done when:** lists PRs with diffs; branch→issue parsing covered by unit tests incl. edge
cases (no id, malformed).

### T0.6 — Git adapter (worktrees)  ·  coder  ·  [deps: T0.2]
`adapters/git.rs` with `git2`: `ensure_worktree` (stacked base = parent branch),
`reconcile`, `prune_worktree`. Restack stubbed (Phase 3).
**Done when:** can create/reconcile/prune a worktree against a scratch repo in tests.

### T0.7 — `cockpit project` + `cockpit ingest` CLI  ·  coder  ·  [deps: T0.3,T0.4,T0.5]
Wire the CLI to build the DAG, compute the frontier (`SPEC.md` §5/§8), and print it.
**Done when:** both commands produce correct frontier/PR output against a real test project.

**Phase 0 exit:** the frontier prints correctly from real data; all adapters tested.

---

## Phase 1 — The loop at the diff gate (headless)  ★ critical milestone

Goal: prove the shared loop round-trips end to end from the CLI. **This is the product in
miniature; everything after is leverage on top.**

### T1.1 — Deterministic prompt assembly  ·  architect→coder  ·  [deps: T0.2]
`prompt.rs`: build the rework prompt per `SPEC.md` §9 — intent, approved plan (diff gate),
current artifact, anchored comments, scope guard incl. the test-weakening clause. Hash + log
the assembled prompt.
**Done when:** golden-file tests assert exact prompt structure for a sample review.

### T1.2 — Agent spawn (PTY)  ·  coder  ·  [deps: T0.6, T1.1]
`adapters/agent.rs`: spawn `claude` in a worktree via PTY (reuse Plannotator's PTY),
`AgentMode::Fix`, capture logs, track pid, map `session_id → (object, mode)`.
**Done when:** spawns a process in a worktree and records the session mapping; covered by a
test with a stub command.

### T1.3 — Stop-hook listener (axum)  ·  coder  ·  [deps: T0.3, T1.2]
`hook_server.rs`: axum server on a fixed localhost port; `/hook/stop` maps session → object,
calls `reconcile` (re-read git, rerun ci/test deltas), clears ephemeral comments, →
`Reworked`, emits a completion signal.
**Done when:** a simulated POST drives a `Dispatched` review to `Reworked` and clears its
comments.

### T1.4 — `comment add` + `request-changes` CLI  ·  coder  ·  [deps: T1.1,T1.2,T1.3]
CLI verbs: add an anchored comment; `request-changes <pr>` gathers open comments → assembles
prompt → spawns fixer → `Dispatched`.
**Done when:** the full verb set works on a real PR.

### T1.5 — End-to-end round-trip test  ·  tester  ·  [deps: T1.4]
The reliability bar: comment → request-changes → agent fixes → pushes → Stop hook → state
flips to `Reworked`, comments cleared, ready for re-review — no manual terminal step.
**Done when:** this runs green against a real (small) PR + agent.

**Phase 1 exit:** the loop round-trips reliably. If it does, the rest is presentation and
reuse; if it doesn't, fix it here before anything else.

---

## Phase 2 — Batch kickoff + optional plan gate

Goal: originate work from a Linear project; reuse the Phase-1 loop on a `ProjectPlan`.

### T2.1 — Plan-doc format + parser  ·  architect→coder  ·  [deps: T0.2]
Decide and pin a structured plan-output format via the planner subagent's instructions
(resolve `SPEC.md` §16 open item), parse it into `PlanDoc` (steps + files + risks as
anchors).
**Done when:** parser round-trips the pinned format; anchors resolve to steps/files.

### T2.2 — Planner spawn + plan gate via `Gated`  ·  coder  ·  [deps: T2.1, T1.x]
Run the loop on `ProjectPlan` with `AgentMode::Plan`; reconcile re-parses the plan doc.
**Done when:** a plan can be commented on and re-planned through the same transitions as a
diff review.

### T2.3 — `cockpit kickoff <project>`  ·  coder  ·  [deps: T2.2, T0.7]
Kick off: optionally produce a plan (→ plan gate) or skip; on approval/skip, spawn the
implementer for every issue, establishing stacked worktrees (base = parent branch). Each
build opens a PR → a `Review` at the diff gate.
**Done when:** a project goes plan→approved→batch-of-PRs (and skip→PRs) with no manual steps.

**Phase 2 exit:** a project can be taken from issues to a batch of diff-gate reviews.

---

## Phase 3 — Restack on rework

### T3.1 — Restack algorithm  ·  architect→coder  ·  [deps: T0.6, T0.3]
Base `Reworked` marks descendants `stale` at dispatch; rebase each descendant in dependency
order via `git2`; clean rebases are pure git.
**Done when:** a 3-PR stack reworked at the base auto-restacks the upper two cleanly.

### T3.2 — Conflict-resolver dispatch  ·  coder  ·  [deps: T3.1, T1.2]
On rebase conflict, spawn the conflict-resolver subagent (`AgentMode::Restack`); on success
clear `stale`.
**Done when:** an induced conflict is resolved via agent and the stack settles.

**Phase 3 exit:** stacked rework is hands-off except for genuine conflicts.

---

## Phase 4 — Tauri shell

Goal: wrap the proven core. The shell is presentation over working logic.

### T4.1 — Tauri scaffold + state + IPC codegen  ·  architect→coder  ·  [deps: Phase 1]
`app/src-tauri` with `AppState` holding core handles (`Arc`), `generate_handler!`
registration, `CommandError` (Serialize) mapping from core errors, `ts-rs` codegen for
domain types with a CI staleness check. Least-privilege capabilities.
**Done when:** a trivial command round-trips and generated TS types are committed + checked.

### T4.2 — Frontier view + agent status  ·  coder  ·  [deps: T4.1]
React + Zustand: the frontier list, per-object agent status, gate controls; subscribe to the
Stop-hook completion event to flip state live.
**Done when:** the morning frontier renders and updates on agent completion.

### T4.3 — Diff gate UI (Monaco)  ·  coder  ·  [deps: T4.2]
Monaco diff editor with inline comment threads + the `ci_delta`/`test_count_delta` flags;
`request changes` calls the command. Do not hand-roll the diff viewer.
**Done when:** a real PR is reviewed and reworked entirely from the desktop app.

### T4.4 — Plan gate UI  ·  coder  ·  [deps: T4.2, T2.2]
Render `PlanDoc` as a commentable document (Plannotator annotation port); approve → build.
**Done when:** a plan is reviewed and approved from the app, triggering the batch.

**Phase 4 exit:** the full loop is usable from the desktop app, both gates.

---

## Phase 5 — Polish

- **T5.1** Batch-approve the clean frontier (size/CI/test-delta heuristics, advisory).
- **T5.2** Optional GitHub comment mirror (confirmed side effect).
- **T5.3** Multi-stack view.

---

## Critical path & risks

- **Critical path:** T0.1 → T0.2 → T0.3 → (T1.1,T1.2,T1.3) → T1.4 → **T1.5**. Everything
  downstream assumes the loop proven at T1.5.
- **Top risk:** the Stop-hook → reconcile round-trip (T1.3). It's the difference between a
  loop and babysitting. De-risk it first inside Phase 1; reuse the Plannotator interceptor.
- **Second risk:** plan-doc parsing (T2.1). Pin the format or anchors will be flaky. Doesn't
  block Phase 1.
- **Do not** start Phase 4 (Tauri) before T1.5 is green. Building UI over an unproven loop is
  the main way this project would waste a week.
