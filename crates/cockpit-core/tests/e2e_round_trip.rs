//! End-to-end round-trip integration test proving the Phase 1 reliability bar.
//!
//! The full loop:
//!   Pending -> InReview -> (add comment) -> request_changes -> Dispatched
//!   -> spawn agent (stub) -> Stop hook fires -> CompletionEvent emitted
//!   -> mark_reworked -> Reworked (comments cleared) -> open -> InReview
//!
//! This exercises every layer through the actual code with a stub agent
//! command (`echo done`) instead of real `claude`.

use std::path::PathBuf;

use cockpit_core::adapters::agent::{self, SessionMap, SpawnConfig};
use cockpit_core::gate::Gated;
use cockpit_core::hook_server::{self, HookState};
use cockpit_core::model::*;
use cockpit_core::prompt::{self, ReworkInput};

/// Build a minimal `Review` starting in `Pending`.
fn make_test_review(worktree: PathBuf) -> Review {
    Review {
        id: ReviewId::new("e2e-review-1"),
        issue: IssueRef::new("TEST-1"),
        pr: PrRef::new("owner/repo#1"),
        branch: "alejandro/test-1-feature".into(),
        base: "main".into(),
        base_sha: "000".into(),
        worktree,
        gate_state: GateState::Pending,
        diff: DiffData {
            raw: "diff --git a/test.rs b/test.rs\n+fn hello() {}".into(),
        },
        head_sha: "abc123".into(),
        comments: vec![],
        parents: vec![],
        children: vec![],
        stale: false,
        agent: None,
    }
}

#[tokio::test]
async fn full_review_loop_round_trip() {
    // 1. Setup: create a temp dir as the worktree and a Review in Pending.
    let tmp_dir = tempfile::tempdir().expect("should create temp dir");
    let mut review = make_test_review(tmp_dir.path().to_path_buf());

    // ---------------------------------------------------------------
    // 2. Open: Pending -> InReview
    // ---------------------------------------------------------------
    review.open().expect("Pending -> InReview should succeed");
    assert_eq!(review.gate_state, GateState::InReview);

    // ---------------------------------------------------------------
    // 3. Add a comment with a DiffLine anchor
    // ---------------------------------------------------------------
    let comment = Comment {
        id: CommentId::new("c-1"),
        anchor: Anchor::DiffLine {
            path: PathBuf::from("test.rs"),
            range: (1, 1),
        },
        body: "Add error handling to this function".into(),
        origin: CommentOrigin::Local,
    };
    review.comments.push(comment);
    assert_eq!(review.comments.len(), 1);

    // ---------------------------------------------------------------
    // 4. Request changes: InReview -> Dispatched
    // ---------------------------------------------------------------
    review
        .request_changes()
        .expect("InReview -> Dispatched should succeed");
    assert_eq!(review.gate_state, GateState::Dispatched);

    // ---------------------------------------------------------------
    // 5. Assemble prompt and verify contents
    // ---------------------------------------------------------------
    let input = ReworkInput {
        intent: "Implement feature TEST-1",
        approved_plan: None,
        artifact: &Artifact::Diff(review.diff.clone()),
        comments: &review.comments,
    };
    let assembled = prompt::assemble_rework(&input);

    // The prompt must contain the comment body.
    assert!(
        assembled
            .text
            .contains("Add error handling to this function"),
        "prompt must include comment body"
    );

    // The prompt must contain the scope guard (the test-weakening clause).
    assert!(
        assembled.text.contains("Don't weaken or delete tests"),
        "prompt must include the scope guard"
    );

    // Hash must be a 64-char hex string (SHA-256).
    assert_eq!(assembled.hash.len(), 64, "prompt hash must be 64 hex chars");
    assert!(
        assembled.hash.chars().all(|c| c.is_ascii_hexdigit()),
        "prompt hash must be valid hex"
    );

    // ---------------------------------------------------------------
    // 6. Spawn agent with stub command
    // ---------------------------------------------------------------
    let session_map = SessionMap::new();
    let config = SpawnConfig {
        command: "echo".into(),
        base_args: vec![],
        tail_args: vec![],
    };

    // Bind the hook server first so we know the port for the hook URL.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("should bind to a random port");
    let port = listener
        .local_addr()
        .expect("should have local addr")
        .port();
    let hook_url = format!("http://127.0.0.1:{port}/hook/stop");

    let agent_run = agent::spawn_agent(
        &review.worktree,
        &assembled,
        AgentMode::Fix,
        review.id.as_str(),
        &session_map,
        &hook_url,
        &config,
    )
    .await
    .expect("spawn_agent should succeed with stub echo command");

    assert_eq!(agent_run.mode, AgentMode::Fix);
    assert_eq!(agent_run.prompt_hash, assembled.hash);
    assert!(agent_run.pid > 0, "spawned process must have a valid PID");

    // Verify the session was registered — find the session_id for our review.
    let session_id = session_map
        .find_by_object(review.id.as_str())
        .expect("session should be registered for our review");

    // Double-check via get().
    let entry = session_map
        .get(&session_id)
        .expect("session entry should be retrievable");
    assert_eq!(entry.object_id, review.id.as_str());
    assert_eq!(entry.mode, AgentMode::Fix);
    assert_eq!(entry.pid, agent_run.pid);

    // Store the agent run on the review (as a real orchestrator would).
    review.agent = Some(agent_run);

    // ---------------------------------------------------------------
    // 7. Hook server: start, POST stop, verify event
    // ---------------------------------------------------------------
    let (tx, mut rx) = hook_server::completion_channel();
    let hook_state = HookState {
        session_map: session_map.clone(),
        completion_tx: tx,
    };

    let app = hook_server::router(hook_state);

    // Start the hook server in a background task.
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    // POST to the stop hook with the session_id.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/hook/stop"))
        .json(&serde_json::json!({ "session_id": session_id }))
        .send()
        .await
        .expect("POST to stop hook should succeed");

    assert_eq!(
        resp.status().as_u16(),
        200,
        "stop hook should return 200 for a known session"
    );

    let body: serde_json::Value = resp.json().await.expect("response should be valid JSON");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["object_id"], review.id.as_str());

    // Verify the CompletionEvent was emitted on the broadcast channel.
    let event = rx
        .recv()
        .await
        .expect("should receive a CompletionEvent on the broadcast channel");
    assert_eq!(event.session_id, session_id);
    assert_eq!(event.object_id, review.id.as_str());
    assert_eq!(event.mode, AgentMode::Fix);

    // Verify the session was removed from the map.
    assert!(
        session_map.get(&session_id).is_none(),
        "session should be removed from the map after stop hook"
    );

    // ---------------------------------------------------------------
    // 8. Reconcile -> Reworked: comments cleared (Invariant 4)
    // ---------------------------------------------------------------
    assert!(
        !review.comments.is_empty(),
        "comments should still be present before mark_reworked"
    );

    review
        .mark_reworked()
        .expect("Dispatched -> Reworked should succeed");
    assert_eq!(review.gate_state, GateState::Reworked);
    assert!(
        review.comments.is_empty(),
        "comments must be cleared on Reworked (Invariant 4: comments are ephemeral)"
    );

    // ---------------------------------------------------------------
    // 9. Re-review: Reworked -> InReview (ready for another cycle)
    // ---------------------------------------------------------------
    review.open().expect("Reworked -> InReview should succeed");
    assert_eq!(review.gate_state, GateState::InReview);

    // ---------------------------------------------------------------
    // Cleanup
    // ---------------------------------------------------------------
    server_handle.abort();
    // tmp_dir is dropped automatically, cleaning up the worktree.
}

/// Agent failure should preserve comments so they can be re-dispatched.
#[tokio::test]
async fn agent_failure_preserves_comments() {
    let tmp_dir = tempfile::tempdir().expect("should create temp dir");
    let mut review = make_test_review(tmp_dir.path().to_path_buf());

    // Open and add a comment.
    review.open().expect("open should succeed");
    review.comments.push(Comment {
        id: CommentId::new("c-1"),
        anchor: Anchor::DiffLine {
            path: PathBuf::from("lib.rs"),
            range: (5, 10),
        },
        body: "Fix the off-by-one error".into(),
        origin: CommentOrigin::Local,
    });

    // Dispatch.
    review
        .request_changes()
        .expect("request_changes should succeed");
    assert_eq!(review.gate_state, GateState::Dispatched);
    assert_eq!(review.comments.len(), 1);

    // Simulate agent failure: Dispatched -> InReview, comments preserved.
    review
        .mark_agent_failed()
        .expect("mark_agent_failed should succeed");
    assert_eq!(review.gate_state, GateState::InReview);
    assert_eq!(
        review.comments.len(),
        1,
        "comments must survive agent failure for re-dispatch"
    );
    assert_eq!(review.comments[0].body, "Fix the off-by-one error");

    // Re-dispatch should work because comments are preserved.
    review
        .request_changes()
        .expect("re-dispatch should succeed with preserved comments");
    assert_eq!(review.gate_state, GateState::Dispatched);
}

/// Dispatching a parent should allow marking children stale.
///
/// This tests the stale-flag logic from `SPEC.md` 7:
///   "Review | a parent enters Dispatched | this.stale = true"
#[tokio::test]
async fn stale_flag_set_on_parent_dispatch() {
    let tmp_dir = tempfile::tempdir().expect("should create temp dir");

    // Parent review.
    let mut parent = Review {
        id: ReviewId::new("parent"),
        children: vec![ReviewId::new("child")],
        ..make_test_review(tmp_dir.path().join("parent"))
    };

    // Child review.
    let mut child = Review {
        id: ReviewId::new("child"),
        parents: vec![ReviewId::new("parent")],
        base: "alejandro/test-1-feature".into(),
        worktree: tmp_dir.path().join("child"),
        ..make_test_review(tmp_dir.path().join("child"))
    };

    // Both start Pending -> InReview.
    parent.open().expect("parent open");
    child.open().expect("child open");

    // Parent gets a comment and is dispatched.
    parent.comments.push(Comment {
        id: CommentId::new("pc-1"),
        anchor: Anchor::DiffLine {
            path: PathBuf::from("main.rs"),
            range: (1, 1),
        },
        body: "Refactor this".into(),
        origin: CommentOrigin::Local,
    });
    parent.request_changes().expect("parent dispatch");
    assert_eq!(parent.gate_state, GateState::Dispatched);

    // When a parent enters Dispatched, the child should be marked stale.
    // (In the real orchestrator this happens automatically; here we simulate it.)
    assert!(
        !child.stale,
        "child should not be stale before parent dispatch is detected"
    );
    child.mark_stale();
    assert!(
        child.stale,
        "child should be stale after parent enters Dispatched"
    );

    // The stale flag does not block transitions.
    child.comments.push(Comment {
        id: CommentId::new("cc-1"),
        anchor: Anchor::DiffLine {
            path: PathBuf::from("child.rs"),
            range: (1, 1),
        },
        body: "Fix child issue".into(),
        origin: CommentOrigin::Local,
    });
    child
        .request_changes()
        .expect("stale child can still dispatch");
    assert!(child.stale, "stale flag survives transitions");

    // After parent is reworked + restack succeeds, child clears stale.
    parent.mark_reworked().expect("parent reworked");
    child.clear_stale();
    assert!(
        !child.stale,
        "child stale cleared after parent rework + restack"
    );
}
