# Cockpit roadmap — reducing the review bottleneck

*Written 2026-07-01. Grounded in two things: the verified external evidence in
[`RESEARCH_REVIEW_BOTTLENECK.md`](./RESEARCH_REVIEW_BOTTLENECK.md) (cited below as R§n) and
three independent code audits of what cockpit actually does today. Supersedes nothing;
`SPEC.md` remains the model of record — this is the prioritized path.*

## Why cockpit exists (sharpened)

Code production is cheap; human review is the constraint — Meta measures the gap directly
(+105.9% lines/diff YoY while review-within-24h falls, R§1), and in the wild the failure
mode is already **rubber-stamping**: 61% of agent PRs get no recorded review at all (R§2).
The evidence says review of agent PRs is *steering work* — 26% of comments are rework
commands, not line critique (R§4) — which is exactly cockpit's gated review→rework loop.

So cockpit's job, precisely: **maximize verified decisions per reviewer-hour, without
becoming the rubber stamp.** Every roadmap item below is judged against that sentence.

---

## Phase A — Close the loop (implemented — PR #12)

The audits found cockpit couldn't yet *finish* a review. This program fixes the defects;
it is the precondition for everything else.

| # | Item | Defect it fixes |
|---|------|-----------------|
| A1 | Real diff line numbers (map hunk fragments → file lines, `side` on anchors) | Comment anchors, rework prompts, and GitHub mirrors all pointed at wrong lines |
| A2 | Approve + Merge (`GateState::Merged`, `gh pr merge --squash` behind explicit confirm, worktree GC) | The happy path was unreachable — no Approve button existed, no merge anywhere |
| A3 | HEAD-authoritative agent outcome (`apply_agent_completion`) | Failed/no-op agents reported success, flipped to Reworked, and destroyed the reviewer's comments |
| A4 | Intent surfacing (PR title/body on `Review`, intent panel, real intent in rework prompts) | Reviewers judged "did it do what was asked" with only a branch slug; fixers got a one-token intent |
| A5 | State persistence (`~/.cockpit/state.json`, atomic, revision-driven flush) | Every restart erased all gate states, comments, and plans |
| A6 | Stack edges for imported PRs + concurrent ingestion | gh-stack stacks rendered flat (stale/restack/frontier dead); refresh took 2N+1 serial subprocesses |
| A7 | Stale propagation (descendants marked stale on dispatch/merge) | The Restack affordance existed but nothing ever set `stale` |
| A8 | Non-blocking plan approve (background fan-out via the streaming spawn path) | Approving a plan froze the UI until every implementer finished; piped stdout could deadlock |
| A9 | Real GitHub reviews (line-anchored `pulls/{n}/reviews` with APPROVE / REQUEST_CHANGES verdicts) | "Submit Review" posted top-level comment spam with wrong line numbers; approval never reached the teammate |
| A10 | Interdiff re-review (`dispatch_snapshot`, default "changes since your review" view) | Every rework cycle cost a full re-read; the requests being addressed were deleted |
| A11 | Agent panel scoping + Stop + logs (events keyed by review, `kill_agent`, open log) | Parallel agents interleaved into one timeline; no way to stop a runaway agent |
| A12 | Worktree correctness for imported PRs (shell + fixer run on the PR branch, not the main checkout) | The Shell tab "verified" whatever branch the main checkout happened to be on |
| A13 | Dead-code removal (workflow.rs engine, `get_version`, `transition_event`, dead frontier slice) | Unused seams misleading every future change |
| A14 | Stack-grouped board (render the DAG with the existing `stack-tree` lib, enabled by A6) | Reviews form a dependency DAG; the board showed a flat list |

## Phase B — Verification instead of reading (implemented — PR #13)

DORA's mechanism (volume exposes weak control systems, R§1) and Meta's RADAR result (R§3)
both say the same thing: the cheapest review minute is the one replaced by machine evidence
the reviewer can *trust at a glance*.

- **B1. Evidence strip per PR** — one glanceable row above the diff: CI x/y with the failing
  job named · test delta (added/changed/deleted test files and assertion counts — SPEC §8's
  unimplemented `ci_delta`/`test_count_delta`) · "agent ran: `cargo test` ✓ (from trajectory)"
  · lockfile/migration/config-touch flags. *This is the RADAR eligibility-gate idea recast as
  presentation: deterministic signals first, human judgment second.*
- **B2. Advisory reviewer pre-pass** — a local reviewer subagent (SPEC §2/§10 already permits
  it) that annotates the diff with flags before the human opens it. Atlassian measured
  −30.8% cycle time from exactly this (R§3); MSR 2026 says it must stay advisory, never the
  gate (R§2). Findings render as dismissible pins, never block, never auto-approve.
- **B3. Test-weakening detector** — diff-level heuristic (deleted assertions, `#[ignore]`,
  `|| true`, snapshot wholesale updates) surfaced as a red flag. Agents weaken tests to get
  green; CLAUDE.md §0.6 bans it — cockpit should *see* it.
- **B4. Full-file diff context** — feed Monaco whole files (worktree when local, `gh api`
  contents when not) so reviewers can scroll beyond hunks and LSP hovers are truthful.
  (A1 fixed the line numbers; this fixes the blindfold.)

## Phase C — Route human attention by risk (implemented — PR #14)

Cockpit can't train a Diff Risk Score on Meta-scale history, but the *minimum viable risk
signal* (open question R-OQ1) is computable locally today: diff size, path sensitivity
(auth/config/migrations/CI/infra), test delta, CI status, stack position, agent-trajectory
anomalies (retries, scope excursions, long thinking on one file).

- **C1. Triage ranking v2** — board order = needs-human ∧ CI-green ∧ small ∧ frontier-root
  first ("decisions you can make in 2 minutes"), risky/red/large last, with the *reason*
  shown on the card (NASA-annunciator style, already the design system's discipline).
- **C2. Fast lane presentation** — a visually distinct "small + green + low-risk + plan-conformant"
  shelf. Explicitly NOT auto-approve (30% distrust, R§2; §9 guardrails) — it compresses the
  *decision*, not the *authority*.
- **C3. Risk chips on cards** — size class, sensitive-path flag, test-delta, CI — the four
  signals the research says carry most of the routing value.
- **C4. Batch CI without N subprocesses** — `gh pr list/search --json statusCheckRollup` gives
  rollups in the list call; cards get CI state at fetch cost ~0.

## Phase D — Make the rework cycle disappear (implemented — PR #15)

The loop is cockpit's moat (R§4). After A10's interdiff, the remaining cycle costs:

- **D1. Addressed-request checklist** — map each dispatched request to the interdiff region
  that answers it ("request → change" pairing), so re-review is confirm-per-request, not
  re-read.
- **D2. Agent transcript persistence** — keep per-review trajectory summaries
  (`~/.cockpit/logs` already holds raw logs; surface them) so "what did it try?" never
  requires re-running.
- **D3. Auto-rebase hygiene** — when a parent merges, offer one-click dependency-ordered
  restack of the whole stack (A7's ordering helper makes this possible).
- **D4. Notify on reviewable** — background refresh + OS notification when a PR flips
  Reworked / a new review-request arrives; suggest the next frontier item while agents run.

## Phase E — The teammate half of the job (implemented — PR #16)

The user reviews colleagues' PRs too; today that flow is read-only-plus-comment-spam (fixed
in A9). Next:

- **E1. Ingest GitHub review state** — show teammates' existing reviews/comments (as
  read-only context, `GitHubMirror` origin) so cockpit isn't blind to the conversation.
- **E2. Re-review on push** — when a review-requested PR gets new commits, the interdiff
  machinery (A10) applies: "changes since your last review" for teammates' PRs.
- **E3. Linear description in intent** — kickoff reviews currently carry only the issue
  title; fetch the description for the intent panel and prompts.

## Phase F — interaction refinements (from research round 2)

Locked in by [`RESEARCH_INTERACTION_PATTERNS.md`](./RESEARCH_INTERACTION_PATTERNS.md)
(2026-07-02). Not scheduled yet; sequenced after B–E land.

- **F1. Finding taxonomy + downvote** — category chips on advisory findings (logic bug /
  edge case / security / performance / …) and a per-finding thumbs-down feeding a local
  noise metric (Graphite's field-tested trust mechanic). Extends `ReviewFinding`.
- **F2. Amortized steering rules** — org/repo-level natural-language review rules
  (a `review-rules.md` alongside skills) injected into the pre-pass prompt; steering
  configured once, not per review.
- **F3. Plan editing** — the plan gate gains direct authoring/editing (9/11 experts author
  plans themselves; approve/reject-only serves the minority mode).
- **F4. Chunked plan checkpoints** — experts hand agents ~2 steps at a time; explore
  splitting batch builds into gated step-chunks rather than one fan-out (needs design; must
  not fork the Gated loop).
- **F5. Guided reading order** — risk-sorted file tree (signals-driven) that orders
  attention without pre-supplying verdicts (priming hazard).
- **F6. Size discipline nudge** — ">400 changed LOC — consider splitting" on cards/evidence
  strip (Cisco sizing data).
- **Binding constraint from the trust-miscalibration study**: no confidence scores or
  polished provenance displays unless validated against detection *accuracy* — faster+more
  confident with unchanged accuracy is the failure mode.

## Deliberate non-goals (guardrails, restated)

- **No auto-approve, no auto-merge, no batch-approve.** Every terminal action stays behind
  an explicit human click (§9). The research's strongest negative result (R§2) is what
  happens when this erodes.
- **No AI verdict as gate.** Pre-review is advisory annotation only.
- **No durable comment threads.** Comments stay ephemeral per cycle (Invariant §0.4);
  `dispatch_snapshot` is single-cycle read-only history, not a thread.
- **Local-first stands.** Nothing in the loop blocks on GitHub round-trips (§0.1).

## Known evidence gaps

- The commercial landscape (Graphite, CodeRabbit, Greptile, BugBot, Conductor, Terragon)
  did not survive verification in the research pass — competitive positioning needs a
  dedicated follow-up before we copy or counter-position against any of them.
- Stacked-PR throughput gains are believed but unquantified in the literature; cockpit's own
  dogfooding is currently our best data source. Instrumenting review-session times
  (locally, private) would let cockpit measure its own thesis.
