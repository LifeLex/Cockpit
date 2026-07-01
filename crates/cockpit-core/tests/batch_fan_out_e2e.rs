//! WAVE 1b acceptance test (Task #2) — the plan-approval implementer fan-out.
//!
//! Proves [`cockpit_core::kickoff::spawn_batch`] against the *real* pieces: the
//! real `prepare_batch_worktrees` (real `git2` worktree creation), the real
//! `spawn_agent` adapter, and **real local git worktrees** — one per review.
//! The only substitution is the agent command: a stub `bash` script standing in
//! for the `claude` implementer (there is no `claude` binary in CI).
//!
//! What it locks down (the two things the plan calls out):
//!   1. **Concurrency bound respected.** Each stub records how many stubs are
//!      running concurrently (via a shared `running/` marker directory). With
//!      `max_parallel_agents = 1` the observed maximum must never exceed 1;
//!      `spawn_batch` awaits each wave before starting the next.
//!   2. **Each review gets its own worktree** and a real commit lands there —
//!      the implementer "builds the initial code" — while the review stays
//!      `Pending` (no auto-advance; Invariant §0.5).
//!
//! All on-disk side effects (agent logs) are isolated under a per-test
//! `COCKPIT_HOME` tempdir, so the test never touches `$HOME/.cockpit`.

use std::path::{Path, PathBuf};

use cockpit_core::adapters::agent::{SessionMap, SpawnConfig};
use cockpit_core::adapters::git;
use cockpit_core::kickoff::{self, KickoffConfig};
use cockpit_core::model::*;
use git2::Repository;

// ---------------------------------------------------------------------------
// Multi-worktree git fixture
// ---------------------------------------------------------------------------

/// A real git repository with an initial commit on `main`. Worktrees for review
/// branches are created by the code under test (`prepare_batch_worktrees`), so
/// the fixture only needs the base repo. Owns the tempdir for cleanup.
struct RepoFixture {
    dir: tempfile::TempDir,
}

impl RepoFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("create repo tempdir");
        let repo = Repository::init(dir.path()).expect("git init");
        {
            let file_path = dir.path().join("app.rs");
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
        Self { dir }
    }

    fn open(&self) -> Repository {
        Repository::open(self.dir.path()).expect("open repo")
    }
}

// ---------------------------------------------------------------------------
// Stub implementer script
// ---------------------------------------------------------------------------

/// Write a stub implementer that (a) records live concurrency via a shared
/// marker directory and (b) makes a real commit in its worktree — the
/// local-first stand-in for "the implementer builds the initial code".
///
/// `running_dir` is a shared directory; the stub creates a uniquely-named file
/// on entry, records the current file count into `max_log`, then removes its
/// file on exit. The peak count across all stubs is the observed concurrency.
fn write_implementer_script(dir: &Path, running_dir: &Path, max_log: &Path) -> PathBuf {
    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
running_dir="{running}"
max_log="{maxlog}"
mkdir -p "$running_dir"
marker="$running_dir/$$"
: > "$marker"
# Record how many stubs are running right now.
count=$(ls "$running_dir" | wc -l | tr -d ' ')
echo "$count" >> "$max_log"
# Do the real git work in the worktree (CWD).
git config user.email "agent@test.com"
git config user.name "agent"
printf 'fn built() {{}}\n' >> app.rs
git add app.rs
git commit -q -m "impl: initial build"
# Small window so overlapping stubs (if any) are observed.
sleep 0.2
rm -f "$marker"
"#,
        running = running_dir.display(),
        maxlog = max_log.display(),
    );
    let path = dir.join("implementer.sh");
    std::fs::write(&path, script).expect("write implementer script");
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

/// Build a `Pending` frontier review (no parents) pointed at a worktree path
/// under the repo. `prepare_batch_worktrees` will actually create the worktree.
fn make_review(id: &str, branch: &str, worktree: PathBuf, project: &ProjectId) -> Review {
    Review {
        id: ReviewId::new(id),
        issue: IssueRef::new(format!("ISSUE-{id}")),
        pr: PrRef::new(format!("owner/repo#{id}")),
        title: String::new(),
        body: String::new(),
        branch: branch.into(),
        base: "main".into(),
        base_sha: String::new(),
        source: ReviewSource::Frontier,
        worktree,
        gate_state: GateState::Pending,
        diff: DiffData { raw: String::new() },
        head_sha: String::new(),
        comments: vec![],
        parents: vec![],
        children: vec![],
        stale: false,
        agent: None,
        repo_slug: None,
        project: Some(project.clone()),
        dispatch_snapshot: None,
    }
}

// ---------------------------------------------------------------------------
// The acceptance test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_batch_bounds_concurrency_and_builds_each_worktree() {
    let repo_fx = RepoFixture::new();
    let cockpit_home = tempfile::tempdir().expect("COCKPIT_HOME tempdir");
    let script_dir = tempfile::tempdir().expect("script tempdir");

    let running_dir = script_dir.path().join("running");
    let max_log = script_dir.path().join("maxlog.txt");

    temp_env::async_with_vars([("COCKPIT_HOME", Some(cockpit_home.path()))], async {
        let project = ProjectId::new("p-batch");

        // Two independent (frontier) reviews. Worktrees live under the repo dir;
        // git worktree metadata dirs cannot contain slashes, so the branch
        // names are flat.
        let wt_a = repo_fx.dir.path().join("wt-a");
        let wt_b = repo_fx.dir.path().join("wt-b");
        let mut reviews = vec![
            make_review("r-a", "review-a", wt_a.clone(), &project),
            make_review("r-b", "review-b", wt_b.clone(), &project),
        ];

        // Both are frontier (no parents).
        let frontier = kickoff::select_frontier_reviews(&reviews);
        assert_eq!(frontier.len(), 2, "both independent reviews are frontier");

        // Phase 1: prepare worktrees (real git2). Scope the repo so it drops.
        let prepared = {
            let repo = repo_fx.open();
            kickoff::prepare_batch_worktrees(&mut reviews, &repo, &ProjectRef::new("p-batch"), None)
                .expect("prepare worktrees")
        };
        assert_eq!(prepared.len(), 2);

        // Each review got its own worktree on disk.
        assert!(wt_a.exists(), "worktree A must exist");
        assert!(wt_b.exists(), "worktree B must exist");
        assert!(
            reviews.iter().all(|r| !r.base_sha.is_empty()),
            "base_sha recorded for each review after prepare"
        );

        // Phase 2: bounded fan-out with max_parallel_agents = 1.
        let session_map = SessionMap::new();
        let script = write_implementer_script(script_dir.path(), &running_dir, &max_log);
        let spawn_config = SpawnConfig {
            command: "bash".into(),
            base_args: vec![script.to_string_lossy().into_owned()],
            tail_args: vec![],
        };
        let config = KickoffConfig {
            session_map: &session_map,
            hook_url: "http://127.0.0.1:1/hook/stop",
            spawn_config: &spawn_config,
            max_parallel_agents: 1,
        };

        kickoff::spawn_batch(&mut reviews, &prepared, &config)
            .await
            .expect("spawn_batch");

        // -----------------------------------------------------------
        // Assertion 1: concurrency bound respected (max observed == 1).
        // -----------------------------------------------------------
        let log = std::fs::read_to_string(&max_log).expect("read max log");
        let observed_max = log
            .lines()
            .filter_map(|l| l.trim().parse::<usize>().ok())
            .max()
            .expect("at least one concurrency sample");
        assert_eq!(
            observed_max, 1,
            "with max_parallel_agents=1 at most one implementer runs at a time"
        );

        // Both stubs actually ran (two samples recorded).
        let samples = log.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(samples, 2, "both implementers ran");

        // -----------------------------------------------------------
        // Assertion 2: each worktree got a real commit (built code) and each
        // review carries an implementer AgentRun; state stays Pending.
        // -----------------------------------------------------------
        for (wt, review) in [(&wt_a, &reviews[0]), (&wt_b, &reviews[1])] {
            let head = git::reconcile(wt).expect("reconcile worktree HEAD");
            let base = git2::Oid::from_str(&review.base_sha).expect("parse base sha");
            assert_ne!(
                head, base,
                "worktree HEAD must advance past base after the implementer commits"
            );

            let agent = review.agent.as_ref().expect("implementer agent attached");
            assert_eq!(agent.mode, AgentMode::Implement);

            assert_eq!(
                review.gate_state,
                GateState::Pending,
                "review stays Pending after implementer builds (no auto-advance)"
            );
        }

        // Sessions were registered by review id (fan-out keys by ReviewId).
        assert!(
            session_map.find_by_object("r-a").is_some(),
            "session registered for r-a"
        );
        assert!(
            session_map.find_by_object("r-b").is_some(),
            "session registered for r-b"
        );
    })
    .await;

    // Log isolation: agent logs landed under COCKPIT_HOME, never real $HOME.
    let logs = cockpit_home.path().join("logs");
    assert!(logs.exists(), "agent logs under COCKPIT_HOME/logs");
}

// ---------------------------------------------------------------------------
// Concurrency bound at max=2 lets both run together (upper edge of the bound).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_batch_allows_up_to_max_parallel() {
    let repo_fx = RepoFixture::new();
    let cockpit_home = tempfile::tempdir().expect("COCKPIT_HOME tempdir");
    let script_dir = tempfile::tempdir().expect("script tempdir");
    let running_dir = script_dir.path().join("running");
    let max_log = script_dir.path().join("maxlog.txt");

    temp_env::async_with_vars([("COCKPIT_HOME", Some(cockpit_home.path()))], async {
        let project = ProjectId::new("p-batch");
        let wt_a = repo_fx.dir.path().join("wt-a");
        let wt_b = repo_fx.dir.path().join("wt-b");
        let mut reviews = vec![
            make_review("r-a", "review-a", wt_a, &project),
            make_review("r-b", "review-b", wt_b, &project),
        ];

        let prepared = {
            let repo = repo_fx.open();
            kickoff::prepare_batch_worktrees(&mut reviews, &repo, &ProjectRef::new("p-batch"), None)
                .expect("prepare worktrees")
        };

        let session_map = SessionMap::new();
        let script = write_implementer_script(script_dir.path(), &running_dir, &max_log);
        let spawn_config = SpawnConfig {
            command: "bash".into(),
            base_args: vec![script.to_string_lossy().into_owned()],
            tail_args: vec![],
        };
        let config = KickoffConfig {
            session_map: &session_map,
            hook_url: "http://127.0.0.1:1/hook/stop",
            spawn_config: &spawn_config,
            max_parallel_agents: 2,
        };

        kickoff::spawn_batch(&mut reviews, &prepared, &config)
            .await
            .expect("spawn_batch");

        let log = std::fs::read_to_string(&max_log).expect("read max log");
        let observed_max = log
            .lines()
            .filter_map(|l| l.trim().parse::<usize>().ok())
            .max()
            .expect("at least one concurrency sample");
        // With max=2 and two reviews, both may run together — the bound is an
        // upper limit, never exceeded.
        assert!(
            observed_max <= 2,
            "observed concurrency {observed_max} must not exceed max_parallel_agents=2"
        );
    })
    .await;
}
