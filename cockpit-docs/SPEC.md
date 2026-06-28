# SPEC.md — Cockpit

A local-first desktop tool that takes a Linear project to merged PRs. A project may start
from an existing plan or a kickoff; if a plan exists it is reviewed once at a project-level
**plan gate**; then the agent implements the whole batch of PRs; then each PR is reviewed
at a per-PR **diff gate** with a Monaco diff and in-app comments, and `request changes`
hands the comments back to the agent.

Working name: `cockpit`. Stack: Rust core + Tauri 2 shell. Reuses the PTY + axum
hook-interception (and, for the plan gate, the annotation surface) from Plannotator.

---

## 1. Problem

A Linear project is a graph of dependent issues handed to local Claude Code agents. Two
things are missing from today's flow and both are expensive:

- **No plan gate.** When there is a project plan, the human doesn't get to confirm the
  approach before the batch is built, so wrong-approach mistakes surface at diff-review
  time (expensive) instead of plan time (free).
- **No reliable rework loop.** Comments don't reliably reach the agent, the branch
  doesn't reliably update, and re-review is unstructured — worse under a stack, where
  reworking a base invalidates everything above it.

## 2. Goal

One reliable review loop, run on one optional project plan and on each PR diff. "Reliable"
means every state transition is explicit, the agent reliably picks up comments, the
artifact reliably updates, and the stack reliably restacks.

Non-goals (v1):

- AI that reviews the code *for* you (CodeRabbit/Greptile do that). cockpit is a human
  review cockpit; a reviewer subagent may pre-flag, advisory only.
- Cloud execution. Agents run locally in worktrees — the whole advantage.
- Team/multi-user, auth, sharing. Single-user, local.

## 3. Core principle

**The local app is the source of truth; the GitHub PR is a published artifact.**

Reviewed objects are tied to worktrees on disk, not to GitHub's review API. Comments live
in the app's local store, anchored to a location in the current artifact so they survive
the artifact changing. Rework is dispatched in-process to a `claude` run in the worktree.
GitHub is touched only at the edges (read PRs/diffs, optionally mirror comments, merge).

## 4. One loop, two places it runs

The review loop is a trait, implemented by two kinds of object:

- `ProjectPlan` — **optional, one per project.** Runs the loop once on the plan doc.
- `Review` — **one per PR.** Runs the loop on the PR diff.

```
review loop (shared):
  Pending → InReview → (request changes) Dispatched → Reworked → InReview → … → Approved
```

`ProjectPlan::Approved` triggers implementing the whole batch. `Review::Approved` triggers
merge. The plan gate can be absent entirely (no plan → skip straight to implementation).

Lifecycle (runtime order):

```
New project
  ├─ plan exists → PROJECT PLAN GATE (loop on the plan) ─┐
  └─ kick off → (optional plan, else skip) ──────────────┤
                                       plan approved / skipped
                                                          ▼
                              Implement all PRs  (agent builds the batch)
                                                          ▼
                              per PR:  DIFF GATE  (loop on the diff)
                                                          ▼
                              Merge   (+ restack on rework)
```

## 5. Entry + kickoff

- Read the Linear project's issues and dependency relations → build the DAG (the same
  graph used for restack).
- **If a plan exists** (Claude produced one, or one is attached): load it into a
  `ProjectPlan` and enter the plan gate.
- **If kicking off:** optionally spawn the planner to produce a project plan (→ plan
  gate), or skip planning and go straight to implementation.
- **On plan approval (or skip):** spawn implementation for every issue. Establish a
  worktree per issue; a stacked issue's worktree base is its parent issue's branch, so the
  stack is wired at build time. Each build opens a PR → a `Review` at the diff gate.
- **Linkage:** branches follow Linear's generated name with the issue id embedded, so
  cockpit maps PR → issue by parsing the branch. This drives both the diff-gate↔issue link
  and the stack edges.

## 6. Data model

```rust
struct ProjectPlan {            // optional, one per project
    project: ProjectRef,
    doc: PlanDoc,
    gate_state: GateState,      // shared loop
    comments: Vec<Comment>,
    agent: Option<AgentRun>,    // planner
}

struct Review {                 // one per PR (diff gate)
    id: ReviewId,
    issue: IssueRef,
    pr: PrRef,
    branch: String,
    base: String,               // base branch OR parent's branch (stacked)
    worktree: PathBuf,
    gate_state: GateState,      // shared loop
    diff: DiffData,
    head_sha: String,
    comments: Vec<Comment>,
    parents: Vec<ReviewId>,     // ancestors in the stack (from Linear deps)
    children: Vec<ReviewId>,
    stale: bool,                // an ancestor is in rework; don't deep-review yet
    agent: Option<AgentRun>,    // fixer / restack
}

/// The shared loop. Both ProjectPlan and Review implement it.
trait Gated {
    fn gate_state(&self) -> GateState;
    fn comments(&self) -> &[Comment];
    fn dispatch(&mut self) -> Result<AgentRun>;   // assemble prompt + spawn in worktree
    fn reconcile(&mut self) -> Result<()>;        // after Stop hook: re-read artifact
}

enum GateState { Pending, InReview, Dispatched, Reworked, Approved }

struct PlanDoc {                // parsed from plan output
    summary: String,
    steps: Vec<PlanStep>,       // ordered; comment anchors
    files: Vec<PathBuf>,        // intended touch set; comment anchors
    risks: Vec<String>,         // migrations, new deps, breaking changes
    raw: String,
}

// Ephemeral: a comment lives for one review→rework cycle, cleared on Reworked.
struct Comment {
    id: CommentId,
    anchor: Anchor,             // points into the *current* artifact only
    body: String,
    origin: CommentOrigin,      // Local | GitHubMirror
}

enum Anchor {
    PlanStep(usize),
    PlanFile(PathBuf),
    DiffLine { path: PathBuf, range: (u32, u32) },  // current head; not durable
}

struct AgentRun { pid: u32, mode: AgentMode, started_at: Instant, prompt_hash: String, log_path: PathBuf }
enum AgentMode { Plan, Implement, Fix, Restack }
```

## 7. State machine

The shared loop (applies to `ProjectPlan` and to each `Review`):

| From       | Event                            | To         |
|------------|----------------------------------|------------|
| Pending    | open in cockpit                  | InReview   |
| InReview   | request changes (≥1 comment)     | Dispatched |
| InReview   | approve                          | Approved   |
| Dispatched | Stop hook + artifact reconciled  | Reworked   |
| Dispatched | agent failed / no change         | InReview   |
| Reworked   | open in cockpit                  | InReview   |

Cross-object / DAG transitions:

| From                       | Event                          | To                       |
|----------------------------|--------------------------------|--------------------------|
| ProjectPlan / Approved     | spawn implementation (batch)   | N Reviews / Pending      |
| (no plan)                  | skip planning                  | N Reviews / Pending      |
| Review / Approved          | merge succeeds → prune worktree | Merged (terminal)       |
| Review                     | a parent enters Dispatched     | this.stale = true        |
| Review.stale               | parent Reworked + restack ok   | this.stale = false       |

`stale` gates the *frontier* (what's safe to deep-review), not the loop.

## 8. The loop (must not break)

Same steps for the plan gate and each diff gate; only the artifact, `AgentMode`, and the
reconcile step differ.

1. **Render artifact.** Plan gate → `PlanDoc` as a commentable document (steps + file set
   + risks; Plannotator port). Diff gate → Monaco diff with inline `ci_delta` and
   `test_count_delta` flags.
2. **Comment.** In-app, stored locally, anchored.
3. **Request changes.** Gather all open comments → one rework request.
4. **Assemble prompt.** §9. Deterministic, hashed, logged.
5. **Spawn.** `claude` in the worktree via PTY. Plan gate → plan mode (planner). Diff gate
   → fixer.
6. **Agent works.** Plan gate → revised plan. Diff gate → edit, test, commit,
   `git push --force-with-lease`.
7. **Close the loop.** Claude Code Stop hook POSTs the axum endpoint (§11). cockpit maps
   session → object, calls `reconcile` (re-parse plan, or re-read git + rerun `ci_delta`),
   clears the dispatched comments (they're ephemeral), → `Reworked`.
8. **Re-review / advance.** Reworked → InReview. Plan approved → implement the batch. Diff
   approved → merge. On any base change with children, restack (§13).

v1 reliability bar: steps 1–7 round-trip on one real PR at the diff gate, from the CLI.

## 9. Prompt assembly

Deterministic, ordered. Prevents the "agent misses the point and loops" failure.

Plan prompt: project intent + issue list + dependency notes + conventions/skills →
"produce a plan; name the files, the order, and the risks."

Rework prompt (either gate):
1. Intent (project plan, or the issue's acceptance criteria).
2. The approved plan (diff gate only — the contract the code was built against).
3. The current artifact (plan doc, or diff).
4. Gathered comments, each with its anchor rendered.
5. Scope guard: "Address only the comments above. Don't refactor unrelated code. Don't
   weaken or delete tests. If a comment is wrong or impossible, stop and say so." The
   test-weakening clause is the highest-ROI line — agents reach for `|| true` and test
   deletion to get green.

## 10. Subagents & skills

cockpit selects a subagent per dispatch by `AgentMode`; definitions live in the repo's
`.claude/`:

- **planner** — project plan + plan rework (plan mode).
- **implementer** — initial batch build of all PRs from the approved plan.
- **fixer** — diff-gate rework (scoped execute).
- **reviewer** — optional ingest pre-flag, advisory only.
- **conflict-resolver** — restack conflicts.

Skills encode repo conventions (monorepo layout, "prefer existing util X") so neither the
planner nor the fixer reimplements prior art.

## 11. Stop-hook listener

`cockpit-core` runs an axum server on a fixed localhost port. The repo's Claude Code config
registers a Stop hook that POSTs `{ session_id }` to `/hook/stop`. cockpit keeps a
`session_id → (object, AgentMode)` map populated at spawn; on callback it reconciles the
right artifact and transitions. Reuse the Plannotator interceptor wholesale.

## 12. Guardrails on side effects

Require explicit UI confirmation; never triggered by ingested content or agent output:
`gh pr merge`; mirroring local comments to the GitHub thread; approving the plan (which
spawns the batch build). Force-push happens inside the agent's worktree, not from cockpit.

## 13. Restack-on-rework

When a base `Review` reaches `Reworked`, descendants were already marked `stale` at
dispatch. Rebase each descendant onto the new base in dependency order: clean rebases via
`git2`; conflicts spawn the conflict-resolver subagent. Successful restack clears `stale`.

## 14. Adapters

- **linear.rs** — read project issues + dependency relations (DAG). GraphQL. cockpit writes
  nothing to Linear in v1.
- **github.rs** — shell out to `gh` (`pr list --json`, `pr diff`, `pr checks`, `pr merge`).
  Parses the Linear issue id out of each PR's head branch to link PR → issue.
- **git.rs** — `git2`: `ensure_worktree` (stacked base = parent branch), `reconcile`,
  `restack`, conflict detection, `prune_worktree` (called on `Merged`).
- **agent.rs** — PTY spawn of `claude` (plan / implement / fix), plan-doc parsing, prompt
  assembly, log capture, pid tracking.

## 15. Build phases

Validation order, not runtime order — build and prove the shared loop first (on the diff
gate), then reuse it for the plan gate, because both share it.

- **Phase 0 — core + adapters, headless.** Domain model, `Gated` trait + state machine,
  all adapters. Proof: `cockpit project <id>` reads issues, builds the DAG, prints the
  frontier; `cockpit ingest` lists existing PRs.
- **Phase 1 — the loop at the diff gate, headless.** `comment add`, `request-changes`,
  spawn fixer, Stop-hook reconcile to `Reworked`. Proof: comment → dispatch → agent fixes
  + pushes → state flips, all from the CLI. **The product in miniature.**
- **Phase 2 — batch kickoff + optional plan gate.** Reuse the loop with the planner on a
  `ProjectPlan`; `cockpit kickoff <project>` plans (or skips), approve, then the
  implementer spawns all PRs. Proof: a project goes plan→approved→batch of PRs (or
  skip→PRs) with no manual terminal steps.
- **Phase 3 — restack-on-rework.** Base rework marks children stale; auto-rebase; conflict
  resolver only on conflict. Proof: a 3-PR stack reworked at the base.
- **Phase 4 — Tauri shell.** Wrap the proven core. Frontier list; Monaco diff + in-app
  comment threads (diff gate); plan renderer (Plannotator port, plan gate); per-object
  agent status. Don't hand-roll the diff viewer — Monaco's diff editor.
- **Phase 5 — polish.** Batch-approve the clean frontier, optional GitHub comment mirror,
  multi-stack view.

## 16. Decisions

- **Issue→PR linkage — settled.** Linear embeds the issue identifier in the branch name it
  generates (e.g. `alejandro/nex-123-...`). github.rs parses the issue id from the PR head
  branch; no PR-body markers or attachment lookups needed.
- **Comments are ephemeral — settled.** A comment lives for one review→rework cycle. On
  `Reworked` the dispatched comments are cleared; the next cycle starts fresh on the new
  artifact. No fuzzy re-anchoring, no durability across diff churn.
- **Worktree GC — settled.** On `Merged`, prune the worktree (git.rs `prune_worktree`).
- **Axum port — open.** Default to a single fixed localhost port (single-user); revisit
  only if it ever needs to be per-project.
- **Plan-doc parsing — open.** Pin a structured plan-output format via the planner
  subagent, or parse loose markdown? Pinning makes the plan anchors reliable.
