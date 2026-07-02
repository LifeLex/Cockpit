//! WAVE 0 acceptance test — the Phase-1 reliability bar (Task #1).
//!
//! Proves the diff-gate Fix loop end to end against the *real* loop pieces:
//! the real `Gated` state transitions, the real `spawn_agent` adapter, the
//! real axum Stop-hook server, and a **real local git worktree**. The only
//! substitution is the agent command itself — a stub `bash` script standing in
//! for `claude`, because there is no `claude` binary (and no network) in CI.
//!
//! The chain under test:
//!
//! ```text
//! comment added
//!   -> request_changes         (InReview  -> Dispatched)
//!   -> spawn_agent in worktree (stub edits + commits a file; a real fix)
//!   -> agent process exits, then POSTs the Stop hook
//!   -> hook server reconcile   (session removed, CompletionEvent emitted)
//!   -> git reconcile           (worktree HEAD advanced == "pushed")
//!   -> mark_reworked           (Dispatched -> Reworked, comments cleared)
//! ```
//!
//! Two things distinguish this from `e2e_round_trip.rs`:
//!   1. The "worktree" is a genuine `git worktree`, not a bare tempdir, and the
//!      stub agent makes a real commit in it — so `git::reconcile` reads a HEAD
//!      SHA that actually advanced. That is the "agent runs in its worktree and
//!      pushes" half of the reliability bar, exercised for real.
//!   2. All filesystem side effects are isolated under a per-test tempdir via
//!      the `COCKPIT_HOME` override, so the test never touches `$HOME/.cockpit`.
//!
//! Choice of surface: a **core-level** integration test rather than a CLI
//! binary test. The CLI's `request-changes` spawns the agent with a fresh,
//! process-local `SessionMap` that is never shared with the hook server started
//! by `cockpit start` (separate process, separate map), and there is no
//! reconcile listener in `run_start` that flips a review to `Reworked`. Driving
//! the full chain through the CLI binary today would therefore test a broken
//! seam, not real behavior. The core-level test exercises the actual loop
//! pieces the CLI (and Tauri app) are meant to wire together, with the least
//! mocking possible.

use std::path::{Path, PathBuf};

use cockpit_core::adapters::agent::{self, SessionMap, SpawnConfig};
use cockpit_core::adapters::git;
use cockpit_core::gate::Gated;
use cockpit_core::hook_server::{self, HookState};
use cockpit_core::model::*;
use cockpit_core::prompt::{self, ReworkInput};
use git2::Repository;

// ---------------------------------------------------------------------------
// Local git fixture
// ---------------------------------------------------------------------------

/// A real git repository with a committed file on `main`, plus a review branch
/// checked out in its own worktree. Owns the tempdir so everything is cleaned
/// up on drop.
struct GitFixture {
    _repo_dir: tempfile::TempDir,
    worktree_path: PathBuf,
    branch: String,
}

impl GitFixture {
    /// Initialize a repo with an initial commit on `main`, create `branch`, and
    /// add a worktree for it. The worktree contains a real tracked file so the
    /// stub agent has something to modify.
    fn new(branch: &str) -> Self {
        let repo_dir = tempfile::tempdir().expect("create repo tempdir");
        let repo = Repository::init(repo_dir.path()).expect("git init");

        // Initial commit on main with a real file.
        {
            let file_path = repo_dir.path().join("app.rs");
            std::fs::write(&file_path, b"fn hello() {}\n").expect("write app.rs");
            let mut index = repo.index().expect("index");
            index.add_path(Path::new("app.rs")).expect("add app.rs");
            index.write().expect("index write");
            let tree_oid = index.write_tree().expect("write tree");
            let tree = repo.find_tree(tree_oid).expect("find tree");
            let sig = git2::Signature::now("test", "test@test.com").expect("sig");
            repo.commit(Some("refs/heads/main"), &sig, &sig, "initial", &tree, &[])
                .expect("initial commit");
        }
        repo.set_head("refs/heads/main").expect("set head main");
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .expect("checkout main");

        // Create the review branch + its worktree using the real git adapter.
        let worktree_path = repo_dir.path().join("wt-review");
        git::ensure_worktree(&repo, &worktree_path, branch, "main")
            .expect("ensure_worktree for review branch");

        Self {
            _repo_dir: repo_dir,
            worktree_path,
            branch: branch.to_string(),
        }
    }

    /// Read the current HEAD OID of the review branch's worktree.
    fn worktree_head(&self) -> git2::Oid {
        git::reconcile(&self.worktree_path).expect("reconcile worktree HEAD")
    }
}

// ---------------------------------------------------------------------------
// Stub agent script
// ---------------------------------------------------------------------------

/// Write a stub agent script that behaves like a fixer: it edits a tracked file
/// in its CWD (the worktree), stages it, and commits — the local-first stand-in
/// for "agent fixes + pushes".
///
/// The stub deliberately does *only* the git work. It does not POST the Stop
/// hook itself: `spawn_agent` mints the session id internally (a UUID) and the
/// child cannot know it, so the test drives the Stop-hook POST with the real
/// registered session id. That keeps the round-trip deterministic while still
/// exercising the real `hook_server` and the real `git::reconcile` against a
/// genuinely advanced worktree HEAD.
///
/// Returns the path to the executable script.
fn write_fixer_script(dir: &Path) -> PathBuf {
    // git identity is set inline so the commit succeeds in CI where no global
    // git config exists.
    let script = r#"#!/usr/bin/env bash
set -euo pipefail
git config user.email "agent@test.com"
git config user.name "agent"
printf 'fn fixed() {}\n' >> app.rs
git add app.rs
git commit -q -m "fix: address review comment"
"#;
    let path = dir.join("fixer.sh");
    std::fs::write(&path, script).expect("write fixer script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).expect("stat script").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod script");
    }
    path
}

// ---------------------------------------------------------------------------
// Review fixture
// ---------------------------------------------------------------------------

/// Build a `Review` in `Pending`, pointed at a real worktree.
fn make_review(worktree: PathBuf, branch: &str, head_sha: &str) -> Review {
    Review {
        id: ReviewId::new("fix-loop-review-1"),
        issue: IssueRef::new("TEST-42"),
        pr: PrRef::new("owner/repo#7"),
        title: String::new(),
        body: String::new(),
        branch: branch.into(),
        base: "main".into(),
        base_sha: "000".into(),
        source: ReviewSource::Frontier,
        worktree,
        gate_state: GateState::Pending,
        diff: DiffData {
            raw: "diff --git a/app.rs b/app.rs\n+fn hello() {}".into(),
        },
        head_sha: head_sha.into(),
        comments: vec![],
        parents: vec![],
        children: vec![],
        stale: false,
        agent: None,
        repo_slug: None,
        project: None,
        dispatch_snapshot: None,
        ci_summary: None,
        review_findings: vec![],
        conversation: vec![],
        last_reviewed_sha: None,
    }
}

// ---------------------------------------------------------------------------
// The acceptance test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn diff_gate_fix_loop_round_trip() {
    // `ensure_worktree` registers the worktree under the branch name; git's
    // worktree metadata dir cannot contain slashes, so the fixture uses a
    // flat branch name. (The real prepare_worktree path sanitizes slashes.)
    let branch = "test-42-fix";
    let git_fx = GitFixture::new(branch);
    let head_before = git_fx.worktree_head();

    // Isolate every on-disk side effect (agent logs live under logs_dir()) in a
    // tempdir via COCKPIT_HOME. temp_env restores the env afterward and avoids
    // the `unsafe` set_var forbidden by the crate's forbid(unsafe_code).
    let cockpit_home = tempfile::tempdir().expect("create COCKPIT_HOME tempdir");
    let script_dir = tempfile::tempdir().expect("create script tempdir");

    temp_env::async_with_vars([("COCKPIT_HOME", Some(cockpit_home.path()))], async {
        let mut review = make_review(
            git_fx.worktree_path.clone(),
            &git_fx.branch,
            &head_before.to_string(),
        );

        // -----------------------------------------------------------
        // 1. Pending -> InReview
        // -----------------------------------------------------------
        review.open().expect("Pending -> InReview");
        assert_eq!(review.gate_state, GateState::InReview);

        // -----------------------------------------------------------
        // 2. Add a diff-line comment (the review feedback)
        // -----------------------------------------------------------
        review.comments.push(Comment {
            id: CommentId::new("c-1"),
            anchor: Anchor::DiffLine {
                path: PathBuf::from("app.rs"),
                range: (1, 1),
                side: DiffSide::New,
            },
            body: "Add a fixed() function".into(),
            origin: CommentOrigin::Local,
        });
        assert_eq!(review.comments.len(), 1);

        // -----------------------------------------------------------
        // 3. request_changes: InReview -> Dispatched (needs >=1 comment)
        // -----------------------------------------------------------
        review.request_changes().expect("InReview -> Dispatched");
        assert_eq!(review.gate_state, GateState::Dispatched);

        // -----------------------------------------------------------
        // 4. Assemble the real rework prompt
        // -----------------------------------------------------------
        let artifact = Artifact::Diff(review.diff.clone());
        let input = ReworkInput {
            intent: review.issue.as_str(),
            custom_preamble: None,
            approved_plan: None,
            artifact: &artifact,
            comments: &review.comments,
            ci_failures: None,
            skills: &[],
        };
        let assembled = prompt::assemble_rework(&input);
        assert!(
            assembled.text.contains("Add a fixed() function"),
            "prompt must carry the comment body"
        );

        // -----------------------------------------------------------
        // 5. Bind the hook server port, then spawn the stub fixer agent
        //    in the real worktree. The stub commits a file (fix + push)
        //    and POSTs the Stop hook.
        // -----------------------------------------------------------
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind hook server");
        let port = listener.local_addr().expect("local addr").port();
        let hook_url = format!("http://127.0.0.1:{port}/hook/stop");

        let session_map = SessionMap::new();
        let script = write_fixer_script(script_dir.path());
        let config = SpawnConfig {
            command: "bash".into(),
            base_args: vec![script.to_string_lossy().into_owned()],
            tail_args: vec![],
        };

        let spawn_result = agent::spawn_agent(
            &review.worktree,
            &assembled,
            AgentMode::Fix,
            review.id.as_str(),
            &session_map,
            &hook_url,
            &config,
        )
        .await
        .expect("spawn_agent with stub fixer");

        review.agent = Some(spawn_result.run);

        // Wait for the stub to finish its git work before we read the
        // worktree HEAD or drive the hook.
        let output = spawn_result
            .child
            .wait_with_output()
            .await
            .expect("await stub agent");
        assert!(
            output.status.success(),
            "stub fixer must succeed; log at {}",
            spawn_result.log_path.display()
        );

        // The session must be registered under the review id.
        let session_id = session_map
            .find_by_object(review.id.as_str())
            .expect("session registered for review");

        // -----------------------------------------------------------
        // 6. The agent "pushed": the worktree HEAD advanced for real.
        // -----------------------------------------------------------
        let head_after = git_fx.worktree_head();
        assert_ne!(
            head_before, head_after,
            "worktree HEAD must advance after the fixer commits"
        );

        // -----------------------------------------------------------
        // 7. Stop-hook reconcile: start the real server, POST the real
        //    session id, assert the CompletionEvent and session removal.
        // -----------------------------------------------------------
        let (tx, mut rx) = hook_server::completion_channel();
        let hook_state = HookState {
            session_map: session_map.clone(),
            completion_tx: tx,
        };
        let server = tokio::spawn(async move {
            axum::serve(listener, hook_server::router(hook_state))
                .await
                .ok();
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(&hook_url)
            .json(&serde_json::json!({ "session_id": session_id }))
            .send()
            .await
            .expect("POST stop hook");
        assert_eq!(resp.status().as_u16(), 200, "known session -> 200");

        let event = rx.recv().await.expect("CompletionEvent emitted");
        assert_eq!(event.session_id, session_id);
        assert_eq!(event.object_id, review.id.as_str());
        assert_eq!(event.mode, AgentMode::Fix);

        assert!(
            session_map.get(&session_id).is_none(),
            "session removed from map after Stop hook"
        );

        // -----------------------------------------------------------
        // 8. git reconcile: record the new HEAD as the reviewed head_sha,
        //    exactly as the real reconcile step would.
        // -----------------------------------------------------------
        let reconciled = git_fx.worktree_head();
        review.head_sha = reconciled.to_string();
        assert_eq!(
            review.head_sha,
            head_after.to_string(),
            "reconciled head_sha must be the agent's new commit"
        );

        // -----------------------------------------------------------
        // 9. mark_reworked: Dispatched -> Reworked, comments cleared
        //    (Invariant 4: comments are ephemeral).
        // -----------------------------------------------------------
        assert!(
            !review.comments.is_empty(),
            "comments present before mark_reworked"
        );
        review.mark_reworked().expect("Dispatched -> Reworked");
        assert_eq!(review.gate_state, GateState::Reworked);
        assert!(
            review.comments.is_empty(),
            "comments cleared on Reworked (Invariant 4)"
        );

        // Clearing the `agent` run after a successful reconcile is the caller's
        // responsibility (the reconcile step in `lib.rs`), not part of
        // `mark_reworked` — the gate transition deliberately leaves `agent`
        // untouched. This test drives the transitions directly, so at this
        // point the dispatched run is still attached; we assert that
        // `mark_reworked` did not clear it, documenting the ownership split.
        assert!(
            review.agent.is_some(),
            "mark_reworked leaves the agent run attached; clearing is the reconcile caller's job"
        );

        // -----------------------------------------------------------
        // 10. Re-open for the next cycle: Reworked -> InReview
        // -----------------------------------------------------------
        review.open().expect("Reworked -> InReview");
        assert_eq!(review.gate_state, GateState::InReview);

        server.abort();
    })
    .await;

    // Assert log isolation: the agent log landed under COCKPIT_HOME, never in
    // the real $HOME/.cockpit.
    let logs = cockpit_home.path().join("logs");
    assert!(
        logs.exists(),
        "agent logs must be written under COCKPIT_HOME/logs"
    );
}

// ---------------------------------------------------------------------------
// Failure edge: agent produced no change / failed -> back to InReview,
// comments preserved for re-dispatch. Driven against the real transitions
// with a stub agent that intentionally does no git work.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_failed_edge_preserves_comments_for_redispatch() {
    let branch = "test-42-nofix";
    let git_fx = GitFixture::new(branch);
    let head_before = git_fx.worktree_head();

    let cockpit_home = tempfile::tempdir().expect("create COCKPIT_HOME tempdir");

    temp_env::async_with_vars([("COCKPIT_HOME", Some(cockpit_home.path()))], async {
        let mut review = make_review(
            git_fx.worktree_path.clone(),
            &git_fx.branch,
            &head_before.to_string(),
        );

        review.open().expect("open");
        review.comments.push(Comment {
            id: CommentId::new("c-1"),
            anchor: Anchor::DiffLine {
                path: PathBuf::from("app.rs"),
                range: (1, 1),
                side: DiffSide::New,
            },
            body: "Fix the off-by-one".into(),
            origin: CommentOrigin::Local,
        });
        review.request_changes().expect("dispatch");
        assert_eq!(review.gate_state, GateState::Dispatched);

        // Spawn a stub that does NOT touch git (simulates an agent that made no
        // change). Use `true`, which exits 0 without modifying the worktree.
        let session_map = SessionMap::new();
        let config = SpawnConfig {
            command: "true".into(),
            base_args: vec![],
            tail_args: vec![],
        };
        let assembled = prompt::assemble_rework(&ReworkInput {
            intent: review.issue.as_str(),
            custom_preamble: None,
            approved_plan: None,
            artifact: &Artifact::Diff(review.diff.clone()),
            comments: &review.comments,
            ci_failures: None,
            skills: &[],
        });
        let spawn_result = agent::spawn_agent(
            &review.worktree,
            &assembled,
            AgentMode::Fix,
            review.id.as_str(),
            &session_map,
            "http://127.0.0.1:1/hook/stop",
            &config,
        )
        .await
        .expect("spawn no-op stub");
        let output = spawn_result
            .child
            .wait_with_output()
            .await
            .expect("await no-op stub");
        assert!(output.status.success());

        // The worktree HEAD did NOT advance -> nothing to reconcile -> the
        // orchestrator treats this as an agent failure.
        let head_after = git_fx.worktree_head();
        assert_eq!(
            head_before, head_after,
            "no-op agent must leave the worktree HEAD unchanged"
        );

        // mark_agent_failed: Dispatched -> InReview, comments preserved.
        review
            .mark_agent_failed()
            .expect("agent-failed -> InReview");
        assert_eq!(review.gate_state, GateState::InReview);
        assert_eq!(
            review.comments.len(),
            1,
            "comments survive agent failure for re-dispatch (Invariant 4 clears only on Reworked)"
        );

        // Re-dispatch works because the comments are still there.
        review.request_changes().expect("re-dispatch");
        assert_eq!(review.gate_state, GateState::Dispatched);
    })
    .await;
}
