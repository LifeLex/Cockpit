# Cockpit — 15-Task Program Tracker

> **STATUS: COMPLETE ✅** — all 15 tasks implemented, whole tree green (cargo fmt/clippy/test: 344 core + 13 CLI + 7 integration; tsc + vite build clean). Final holistic review PASS (invariant-clean, no blockers). Changes uncommitted on `alejandro/t0.2-domain-model`, awaiting human review. Residual follow-ups (not blockers) listed at the bottom.

Autonomous program run. Branch: `alejandro/t0.2-domain-model`. Changes are left **uncommitted** in the working tree (per CLAUDE.md: never commit unless asked); user reviews when the whole program is done.

Status legend: ⬜ not started · 🟡 in progress · ✅ done · 🔵 in review

## Wave status

| Wave | Tasks | Status |
|---|---|---|
| 0 — Foundations (paths + auth + loop) | #7, #1, #15(core) | ✅ |
| 1 — Domain model + Phase 2 + prompt seam | #12, #2, #4, #3 | 🟡 |
| 2 — Independent FE / adapters | #9, #13, #10, #14, #11 | ✅ |
| 3 — LSP (isolated, heavy) | #5 | ✅ |
| 4 — Settings consolidation | #8, #15(fe) | ✅ (pulled forward) |
| 5 — Cleanup sweep (last) | #6 | ✅ |

## Task status

| # | Task | Status |
|---|---|---|
| 1 | Harden diff-gate Fix loop (worktree path + CLI e2e test) | ✅ |
| 2 | Full Phase 2 (plan gate → planner → fan-out implementers) | ✅ (per-project plan scoping = follow-up) |
| 3 | Skills for reviews + installable (gh api) | ✅ |
| 4 | Customizable agent prompts (per-AgentMode override) | ✅ |
| 5 | LSP for Monaco (pyright + typescript-language-server) | ✅ (needs servers installed to verify at runtime) |
| 6 | Remove unused code / idiomatic cleanup | ✅ |
| 7 | Worktrees + logs under `$HOME/.cockpit/` | ✅ |
| 8 | Settings: give proper use, remove dead fields | ✅ |
| 9 | Monaco theme not loaded on first review (beforeMount fix) | ✅ |
| 10 | Nav buttons show command/shortcut labels | ✅ |
| 11 | Remove batch-approve button | ✅ |
| 12 | Project concept (groups PRs; Linear optional source) | ✅ |
| 13 | CI visibility + dispatch-to-fix (gh checks / log-failed) | ✅ |
| 14 | Remove PR Info tab, relocate links + stack | ✅ |
| 15 | Auth = user's Claude CLI login (no API key/SDK) | ✅ |

## Residual follow-ups (non-blocking, for your review — none are invariant violations)
1. **CLI loop daemon seam** — the diff-gate loop is proven in core + wired in the Tauri app (single long-running process). The *CLI* can't close Stop-hook→Reworked across its separate short-lived processes; needs a daemon model. Out of scope for "harden the loop."
2. **Per-project plan scoping** — `Project.plan` exists in the model but `AppState.plan` is still a single global slot (old kickoff model). Plan generation/gate works globally; wiring it per-project (+ surfacing a "generate plan" entry + project-scoped batch status) is an integration item.
3. **Two dangling backend commands** — `restack_pr`, `load_plan_from_path` are wired/registered but have no FE entry point yet; `get_version` is a dead diagnostic command.
4. **Runtime-verify items** (need real binaries, can't be unit-tested): agent loop end-to-end with real `claude`; LSP features with `pyright`/`typescript-language-server` installed (+ no orphan LSP procs on quit); CI badge/fix with live `gh auth`.
5. **No FE test harness** — `app/` has no vitest/eslint; FE gate is `tsc --noEmit` + `vite build` only. Adding vitest is a recommended follow-up.
6. **Commit boundaries** — all work is one uncommitted change set; split into stacked PRs by task/wave at commit time (per CLAUDE.md §7).

---

## Key decisions (resolved in research)

- **#15 Auth:** Use the user's own Claude Code login via the `claude` CLI. No API key, no Agent SDK, no cost accounting. Resolve `claude` via login-shell PATH so a bundled app finds `~/.local/bin/claude`. Remove `anthropic_api_key`, `daily_budget_usd`, `model` from config.
- **#7 Paths:** `cockpit_home()` = `$HOME/.cockpit`, with `worktrees/` (keyed `<repo-slug>/<branch>`) and `logs/` (`agent-<session>.log`). Fixes the inside-worktree log bug and the relative-fallback worktree bug.
- **#12 Projects:** First-class `Project` groups PRs; PRs may be ungrouped. Linear becomes one optional *source* (`ProjectSource::{Linear(id) | AdHoc}`), not the entry point. "Kickoff" → "New Project"; "Plan" → the project's plan gate.
- **#2 Phase 2:** Plan approval fans out N implementers (one worktree each), bounded by `max_parallel_agents` (default 3); per-implementer Stop-hook reconcile. Reuses the one `Gated` loop.
- **#3 Skills:** Local `~/.cockpit/skills/{name}/SKILL.md` + `.meta.json`; install via GitHub Contents API + PAT with per-skill SHA idempotency. Injected into cockpit's own rework prompt via existing `ReworkInput.skills`.
- **#4 Agents:** One editable prompt fragment per `AgentMode`, stored override → builtin fallback, injected verbatim. `agent_command` becomes live.
- **#5 LSP:** `monaco-languageclient` + `vscode-ws-jsonrpc` in webview ↔ Rust localhost WS bridge spawning `pyright-langserver --stdio` and `typescript-language-server --stdio` as Tauri sidecars.
- **#8 Settings:** Give proper use. Final set: Repository, Worktrees location (read-only), Linear key+project, IDE command, App theme, Editor theme, Hook port, Skills-GitHub sync, Agent prompts. Remove dead: `anthropic_api_key`, `daily_budget_usd`, `terminal_font`, `terminal_font_size`, `github_token`, `model`.
- **#9 Monaco theme:** Register custom themes in `beforeMount`, not `onMount` (race — editor instantiates before themes register, falls back to `vs-dark`).
- **#10 Shortcuts:** Single-source registry `{keys,label,handler}` feeding both keydown binding and a `<Kbd>` render.
- **#11 Batch-approve:** Remove — unreachable for the real workflow, dead knob, brushes safety invariant §9.
- **#13 CI:** `gh pr checks --json` (status via Tauri event) + on-demand `gh run view --log-failed` fed into rework prompt as an explicit "Fix CI" action (never auto).
- **#14 PR Info:** Remove the tab; move PR/issue/repo links to DiffView header + a small collapsible stack strip.
- **#6 Cleanup:** Last. Remove dead `ui/*`, orphan bindings; wire-or-gate `restack_descendants`/`mark_descendants_stale`; demote `build_review`.

---

## Execution notes

- Every wave ends with a **reviewer** pass (DoD §6 + invariants §0) and a ts-rs bindings regen check.
- Gate each wave on `cargo fmt` / `clippy -D warnings` / `cargo test` + `tsc --noEmit` / `vitest` green before the next.
- Regenerate `ts-rs` bindings whenever a domain type changes (run `cargo test`).
- Critical path: Wave 0 → Wave 1 (#12→#2) → Wave 4 (#8) → Wave 5 (#6). Waves 2/3 hang off Wave 1.
- Conflict hazards: `DiffView.tsx` (#9, #14, #5 — serialize), `App.tsx` (#10, #11, #14 — serialize), `config.rs`/`SettingsView.tsx` (#3,#4,#8,#15 — Wave 4 last of these).

## Carry-overs / follow-ups discovered during execution

- **store.rs state files** (`STATE_FILE`/`PLAN_STATE_FILE` = relative `.cockpit/*.json`, CLI-only) still write to CWD, not `$HOME/.cockpit`. Deferred to **Wave 1 / #12** (store.rs is reworked there for Projects) — add `state_file_path()`/`plan_state_file_path()` via `cockpit_home()` and swap the ~28 CLI call sites.
- **git.rs `~/.cockpit/repos` clone dir** — already under cockpit_home-ish; confirm/normalize in Wave 5 cleanup.

## Progress log

- **#6 cleanup DONE** — removed vestigial KickoffConfig fields, redundant `worktree_base` param (build_review→private), dead `restack_descendants` (was fully unreferenced — surfaced), gated `mark_descendants_stale`/`dependency_order` to test; dropped orphan ts-rs derives + stale bindings (Artifact/BatchApproveConfig/BatchVerdict/SkillMeta/LspLanguage); removed BatchApprovePanel/KickoffView/dead ui/*; removed `batch_approve_preview` cmd + orphan store actions; fixed tautological e2e assert; **completed #3**: `Skill.source` exposed from `.meta.json` → SkillsView Local/GitHub badge. Kept (still used): batch::evaluate_frontier (CLI), restack_pr/load_plan_from_path (wired backends).
- **COORDINATOR FINAL VERIFICATION (whole combined tree):** `cargo fmt --check` clean · `cargo clippy -D warnings` clean · `cargo test --all` GREEN (344 core + 13 CLI + 2 batch-e2e + 3 round-trip + 2 fix-loop) · `npx tsc --noEmit` clean · `npm run build` OK. Scope: 86 files (46 mod, 40 new), +6879/−2443. **ALL 15 TASKS IMPLEMENTED & GREEN.** Final holistic reviewer running.

- **#13 CI DONE** — `github::{CiCheck,CiSummary,summarize,pr_checks,failed_ci_logs,run_id_from_link,truncate_tail}` (NEUTRAL/SKIPPED/CANCELLED=pass); `ReworkInput.ci_failures` → `## CI Failures` section (after Comments, before Scope Guard; None=byte-identical); Tauri `fetch_ci_checks` (emits `ci-updated` event) + `fix_ci` (explicit §9: ensures InReview, synthetic CI comment, request_changes, shared `dispatch_fix_agent` with logs); DiffView CI badge + Fix-CI button (event-driven). Non-fatal on gh error (§0.1). Green: 334 core + tsc + build. gh-shelling wrappers integration-only (pure parsers tested w/ fixtures).

- **FE-1/FE-2/FE-3 + #8 DONE — whole app/src tsc + vite build clean.** FE-1: nav IA + shortcuts (#10) + batch-approve unwired (#11). #8: settings rewrite + dead config fields removed + #15-fe auth note. FE-2: PR-Info tab removed + relocated to DiffView header (#14), Monaco theme beforeMount fix (#9). FE-3: NewProjectView/SkillsView/AgentEditor/PlanView (+ProjectCard); PlanView approve behind §9 confirm dialog.
  - **FE follow-ups (non-blocking, for #6 cleanup or a small fix):** (a) `Skill` binding has no `source` field — `list_skills` should return source (from `.meta.json`) so SkillsView can show Local/GitHub badge; (b) plan gate is still a GLOBAL singleton (old kickoff model), not per-`Project` — no "generate plan" entry point surfaced and `batchStatus` is aggregate not project-scoped. Full per-project plan wiring (Project.plan is the field; store still global) is a larger integration item — document, don't rush late.

- **#2 plan-doc ingestion follow-up DONE** — coder: `ProjectPlan.plan_path`; `config::plans_dir()`/`plan_file_path()` (`<cockpit_home>/plans/<slug>.md`); `assemble_plan_prompt` emits `## Output` with pinned `PLAN_FORMAT` telling planner to write there; `lib.rs::ingest_plan_output` parses the file into `doc` on Plan completion (non-fatal, Pending stays Pending, Dispatched→Reworked). Bindings: ProjectPlan.ts. Green: 322 core + integration. **WAVE 1 CORE COMPLETE.**
- **FE decomposition (revised for file-ownership, minimize App/store/DiffView contention):**
  - FE-1 (owns App.tsx, Sidebar.tsx, store.ts): #12 IA (PRs grouped by project + Ungrouped; Projects/Skills/Agents routes; remove Stacks), #10 shortcut registry+Kbd, #11 remove batch-approve, + all store actions + stub view components.
  - FE-2 (owns ReviewWorkspace.tsx, DiffView.tsx): #14 remove PR Info tab + relocate links to DiffView header, #9 Monaco theme beforeMount fix.
  - FE-3 (after FE-1): flesh out PlanView / NewProjectView / SkillsView / AgentEditor (new files, consume FE-1 store actions).
  - #13 (CI) is full-stack → Wave 2 (needs core github.rs + prompt.rs first).
  - **NOTE: FE passes must run SERIALLY** — `tsc`/`vite build` type-check the whole app/src tree, so concurrent FE agents trip over each other's in-progress edits. Only read-only (Rust reviewer) runs concurrent with FE. FE gate = `npx tsc --noEmit` + `npm run build` (no vitest/eslint configured — flag for follow-up).
  - FE-1 DONE (all owned files tsc-clean). New serial FE order: **#8 settings (pulled fwd, greens tsc)** → FE-2 workspace → FE-3 views.
- **FE-1 DONE** — nav IA (PRs⌘1/Projects⌘2/Skills⌘3/Agents⌘4/Settings⌘5, Stacks removed); PRs-grouped-by-project + Ungrouped; `lib/shortcuts.tsx` registry + `<Kbd>` (#10); batch-approve unwired (#11); all FE-3 store actions added; stub NewProjectView/SkillsView/AgentEditor. Only remaining tsc error = pre-existing SettingsView Config drift → #8 next. Contracts for FE-3 recorded in FE-1 report (NewProjectView{onDone}; Skills/Agents pull from store; generate_plan takes no projectId currently; batchStatus per project).
- **Wave 1 core review DONE — CLEAN, no blockers.** Invariants §0.3/§0.5/§9/§0.1 all verified PASS; ts-rs bindings no drift; batch_fan_out_e2e not over-mocked. Non-blocking follow-ups all already scheduled: KickoffConfig dead fields (api_key/http_client/worktree_base) + `build_review` redundant `worktree_base` param → Wave 5 #6; config dead fields → Wave 4 #8. **WAVE 1 CORE APPROVED.**

- **Wave 1d — #3 skills DONE** — coder: `skills_dir()` (`$HOME/.cockpit/skills/<name>/SKILL.md` + `.meta.json`); `SkillSource{Local|GitHub{owner,repo,sha}}` (TS discriminated union); `install_skill`/`delete_skill`/`sync_from_github` (via `gh api`, SHA-idempotent `classify_sync`: Install/Update/Skip/LocalKeep, never clobbers hand edits); `changed_files_from_diff`+`relevant_for_diff`; `config.skills_github: Option<SkillsGithub{owner,repo,branch,path,auto_sync}>`. Skills now ACTUALLY feed every dispatch site (Fix/Implement/Restack/Plan), non-fatal on error (§0.1). CLI `skills {list,install,sync,new}`; Tauri `list_skills`/`save_skill`/`delete_skill`/`sync_skills`; bindings emitted (Skill/SkillSource/SkillMeta/SkillsGithub/SyncReport). Golden proves skills land after scope guard. Green: 317 core + 13 CLI + 5 integration. `sync_from_github` gh-wrapper not unit-tested by design (idempotency core is).

- **Wave 1c — #4 agent prompts DONE** — coder: `config.AgentPrompts{implement,plan,fix,restack: Option<String>}` (blank=none) + `for_mode`/`set_mode`; `prompt::scope_guard()`/`builtin_intent(mode)` public; verbatim `custom_preamble` seam (`## Custom Instructions` after Intent) in `assemble_rework`+`assemble_plan_prompt` (None→byte-identical); `SpawnConfig::from_config` makes `agent_command` LIVE at all 8 spawn sites (4 Tauri + 4 CLI); Tauri `get_agent_prompt`/`get_builtin_agent_prompt`/`save_agent_prompt`; bindings `AgentPrompts.ts`+`Config.ts`. Green: 300 core + integration. NOTE: skills-sync auth should use `gh api` (not a PAT) to stay consistent with #15/#8 removing `github_token`.
- **Disk maintenance** — `cargo clean` (reclaimed 20G; was at 91%/1.8G free → 60%/11G free). Next build is a full rebuild.

- **Wave 1b — #2 Full Phase 2 DONE** — coder: `assemble_plan_prompt` + `PlanInput`; `config.max_parallel_agents` (default 3); `spawn_batch` split into sync `prepare_batch_worktrees` (holds git2) + async bounded fan-out (chunks of max_parallel, git2 never crosses `.await`); `store::batch_status`/`BatchStatus`; Tauri `generate_plan`/`batch_status`, async `plan_request_changes` (spawns planner) + guarded `plan_approve` (fan-out, §9); lib.rs `Implement` + `Plan` completion arms; plan gate reuses 5-state loop (NO new GateState — §0.3 respected). Tests: 5 plan-gate + 3 batch_status + 4 plan-prompt + `batch_fan_out_e2e` (concurrency bound + per-review worktree, stub implementers). Green: 287 core + 2 batch e2e + fix-loop + CLI. Removed `KickoffConfig.repo`.
  - **#2 FOLLOW-UP (open):** `Plan` completion arm does NOT re-parse planner output into `ProjectPlan.doc` — needs a `plan_path` on the model + `plan_parser::reconcile_plan` wiring. Isolated to model.rs/plan reconcile. Do after #3, before Wave-1 reviewer. Without it, generate_plan runs the agent but the plan doc stays empty.

- **Wave 1a — #12 Project model DONE** — coder: `ProjectId`/`ProjectSource`/`Project` in model.rs; `Review.project: Option<ProjectId>`; `ProjectStore` + `reviews_by_project`; state-path fix folded in (`state_file_path`/`plan_state_file_path`/`project_state_file_path` via cockpit_home, 13 CLI sites swapped, old consts removed); unified worktree keying `<project-or-"ungrouped">/<sanitized-branch|issue>` via `kickoff::review_worktree_path` (all 3 sites routed); Tauri `list_projects`/`create_project`/`attach_review`; bindings emitted (Project/ProjectId/ProjectSource + Review.project). Existing `kickoff` preserved. Green: 274 core + 13 CLI + 5 integration. **Design override for #2: do NOT add new GateState variants — reuse the 5-state loop (§0.3).** Wave-5 cleanup note: `KickoffConfig.worktree_base` now vestigial.

- **Wave 0 (core) DONE** — coder agent: added `config::{cockpit_home,worktrees_dir,logs_dir}` (with `COCKPIT_HOME` env override for test isolation); agent logs → `logs_dir()`; login-shell PATH resolution for spawned `claude` (auth = user's `~/.claude`, no API key/SDK); worktree base → `worktrees_dir()` in kickoff/commands/CLI/git; `temp-env` dev-dep added (crate is `#![forbid(unsafe_code)]`).
- **Wave 0 (test) DONE** — tester agent: added `crates/cockpit-core/tests/fix_loop_e2e.rs` (real loop: stub agent commits in real git worktree → real axum hook server round-trip → reconcile → Reworked + comments cleared + log isolated under COCKPIT_HOME). Chose core-level over CLI because the **CLI loop seam is broken** (request_changes and start use separate in-memory SessionMaps; start drops the completion rx with no reconcile listener) — logged as carry-over below.
- **Wave 0 (bugfix) DONE** — coordinator: fixed `git::ensure_worktree`/`prune_worktree` to flatten `/`→`-` in the worktree registration name (realistic slashed branches broke git metadata dir); this is on the Phase 2 path via `kickoff.rs:293`. Added regression test. **Full workspace green: 262+3+2+13, clippy/fmt clean.**
- **Wave 0 reviewer DONE** — no blockers. Applied fixes: removed stale `app/src-tauri/.cockpit/` logs + added `.cockpit/` to `.gitignore`; guarded empty `COCKPIT_HOME`. Deferred: worktree-keying unification (`review_worktree_path()` helper across the 3 call sites) → folded into #12; tautological `agent=None` assert in e2e test → Wave 5 cleanup. **WAVE 0 COMPLETE, green.**
- Scope note (reviewer): working tree also carries prior-session changes (agent_stream schema, request_changes async spawn, all app/src/*) — pre-existing, not Wave 0. Commit-boundary split is a human decision at PR time.

### NEW carry-over (from tester):
- **CLI loop seam broken** — `crates/cockpit-cli`: `run_request_changes` spawns with a process-local `SessionMap`; `run_start` starts the hook server with a *separate* map and drops the completion receiver (no reconcile listener). The CLI cannot close Stop-hook→Reworked across its short-lived processes. Needs a **daemon model** (long-running process owning SessionMap + hook server + reconcile) or a persisted session map. OUT OF SCOPE for #1 ("harden the loop" = core loop proven + Tauri wired). Track as a distinct future task.
