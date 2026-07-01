# Dogfooding checklist

The core loop, Phase 2 fan-out, CI, and LSP bridge are proven by headless
integration tests (`crates/cockpit-core/tests/*`), but several paths can only be
verified against a live environment (real `claude`, `gh`, and language servers)
with the app running. Run this checklist with `cargo tauri dev` from a repo where
you have open PRs.

## Prerequisites

- `claude` on your PATH and logged in (`claude` uses `~/.claude`; no API key).
- `gh` installed and authenticated (`gh auth status`).
- For LSP: `npm i -g pyright typescript-language-server`.
- A GitHub repo with at least one open PR (ideally one with failing CI).
- `~/.cockpit/config.toml` with `repo_path` set (and `linear_api_key` if using Linear projects).

## 1. Auth & spawn

- [ ] Open a review, add a comment, click **Request changes**. Confirm a `claude`
      process starts (`ps aux | grep claude`) with cwd = the review's worktree
      under `$HOME/.cockpit/worktrees/...`, and a log appears in `$HOME/.cockpit/logs/`.
- [ ] Confirm nothing is written into the reviewed repo's tree (no in-tree `.cockpit/`).

## 2. Diff-gate loop (the reliability bar)

- [ ] comment → Request changes → agent edits + pushes in its worktree → Stop-hook
      fires → the PR flips to **Reworked** and the ephemeral comments clear.
- [ ] The Agent tab streams events live; the diff refreshes to the new head.

## 3. Plan gate + Phase 2 (per project)

- [ ] Create a Project (blank or from Linear). Open it → **Generate plan** → the
      planner runs and the plan doc populates (Pending).
- [ ] Open for review → add a plan comment → **Request changes** → planner reworks → Reworked.
- [ ] **Approve & build** (confirm dialog) → N implementers fan out, one worktree
      each, bounded by `max_parallel_agents`; only THIS project's frontier reviews spawn.
- [ ] Open a second project; confirm its plan/batch state is independent.

## 4. CI tab

- [ ] On a PR with CI, the **CI** tab lists checks grouped by workflow with pass/fail/pending.
- [ ] A failing pipeline auto-expands and loads its failed-run logs inline; a passing
      one stays collapsed and triggers no `gh` call.
- [ ] For an expired/in-progress run (HTTP 410), the panel shows the "logs unavailable"
      message and a working **View run on GitHub** link.
- [ ] **Fix CI failures** (confirm) dispatches the fixer with the CI logs in the prompt.

## 5. External links

- [ ] The check link, and DiffView's PR/issue/repo links, open in your system browser
      (opener plugin). If a link fails, a visible error appears (not a silent no-op).

## 6. LSP

- [ ] Open a `.py` and a `.ts` file in the diff editor; confirm diagnostics/hover/completion
      appear (needs pyright / typescript-language-server installed).
- [ ] Close reviews / quit the app; confirm no orphan `pyright`/`tsserver` processes remain
      (`ps aux | grep -E 'pyright|typescript-language-server'`).

## 7. Restack

- [ ] For a stale review (parent reworked), the **Restack** button appears; clicking it
      rebases onto the parent's new head, or spawns the conflict-resolver on conflict.

## Known limitations to watch for

- The plan doc is ingested from `~/.cockpit/plans/<project>.md` written by the planner;
  if the planner doesn't write there, the plan stays empty (check the log).
- Skills are read from `~/.cockpit/skills/*/SKILL.md` and filtered by the diff's file
  extensions; verify a relevant skill actually appears in the agent's prompt (log).
