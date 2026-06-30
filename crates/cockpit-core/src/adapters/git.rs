//! Git adapter — worktree management and restack via `git2`.
//!
//! Provides `ensure_worktree`, `reconcile`, `prune_worktree`, and `restack`
//! for the review lifecycle.
//!
//! All operations are synchronous and short-lived. The caller can wrap in
//! `spawn_blocking` if needed in an async context.

use std::path::{Path, PathBuf};

use git2::{BranchType, Oid, Repository, WorktreeAddOptions, WorktreePruneOptions};
use tokio::process::Command;

/// Errors from git worktree operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The target path already has a worktree.
    #[error("worktree already exists at {0}")]
    WorktreeExists(std::path::PathBuf),

    /// The branch could not be found in the repository.
    #[error("branch `{0}` not found")]
    BranchNotFound(String),

    /// HEAD is detached in the worktree (unexpected after agent work).
    #[error("HEAD is detached in worktree at {0}")]
    DetachedHead(std::path::PathBuf),

    /// Rebase hit conflicts.
    #[error("rebase hit conflicts in {count} files")]
    RebaseConflict {
        /// Number of files with conflicts.
        count: usize,
    },

    /// No common ancestor found between the branch and its base.
    #[error("no merge base found between `{branch}` and `{base}`")]
    NoMergeBase {
        /// The branch being restacked.
        branch: String,
        /// The target base branch.
        base: String,
    },

    /// `git fetch` for a remote branch failed.
    #[error("failed to fetch branch `{branch}`: {reason}")]
    FetchFailed {
        /// The branch that was being fetched.
        branch: String,
        /// Human-readable reason for the failure.
        reason: String,
    },

    /// `git worktree add` via CLI failed.
    #[error("worktree add failed for `{branch}`: {reason}")]
    WorktreeAddFailed {
        /// The branch the worktree was being created for.
        branch: String,
        /// Human-readable reason for the failure.
        reason: String,
    },

    /// `git worktree remove` via CLI failed.
    #[error("worktree remove failed at {path}: {reason}")]
    WorktreeRemoveFailed {
        /// The worktree path that was being removed.
        path: std::path::PathBuf,
        /// Human-readable reason for the failure.
        reason: String,
    },

    /// Underlying git2 error.
    #[error(transparent)]
    Git2(#[from] git2::Error),
}

/// Create a git worktree for a review branch, checked out from `base`.
///
/// If `base` is a parent review's branch (stacked PR), the worktree starts
/// from that branch's tip. Returns the OID of the initial HEAD commit.
///
/// The worktree is registered under the name `branch` in the parent repo.
pub fn ensure_worktree(
    repo: &Repository,
    path: &Path,
    branch: &str,
    base: &str,
) -> Result<git2::Oid, Error> {
    if path.exists() {
        return Err(Error::WorktreeExists(path.to_path_buf()));
    }

    let base_ref = repo
        .find_branch(base, BranchType::Local)
        .map_err(|_| Error::BranchNotFound(base.to_string()))?;
    let base_commit = base_ref.get().peel_to_commit()?;

    let new_branch = repo.branch(branch, &base_commit, false)?;
    let branch_ref = new_branch.into_reference();

    let mut opts = WorktreeAddOptions::new();
    opts.reference(Some(&branch_ref));

    repo.worktree(branch, path, Some(&opts))?;

    Ok(base_commit.id())
}

/// Re-read the HEAD SHA of a worktree after agent work.
///
/// Opens the worktree as a repository and returns the HEAD commit's OID.
pub fn reconcile(worktree_path: &Path) -> Result<git2::Oid, Error> {
    let wt_repo = Repository::open(worktree_path)?;
    let head = wt_repo.head()?;

    if !head.is_branch() {
        return Err(Error::DetachedHead(worktree_path.to_path_buf()));
    }

    let commit = head.peel_to_commit()?;
    Ok(commit.id())
}

/// Remove a worktree and clean up its directory.
///
/// The worktree is identified by the name it was registered under (which
/// matches the branch name passed to [`ensure_worktree`]).
pub fn prune_worktree(repo: &Repository, worktree_name: &str) -> Result<(), Error> {
    let wt = repo.find_worktree(worktree_name)?;
    let mut prune_opts = WorktreePruneOptions::new();
    prune_opts.valid(true).working_tree(true);
    wt.prune(Some(&mut prune_opts))?;
    Ok(())
}

/// Fetch a remote branch if not available locally, then create an isolated worktree.
///
/// The worktree is placed at `repo_path/.cockpit/worktrees/<pr_id>` and checked
/// out on `branch`. If the branch does not exist locally, `git fetch origin <branch>`
/// is run first via `tokio::process::Command`.
///
/// Returns the path to the created worktree directory.
pub async fn prepare_worktree(
    repo_path: &Path,
    branch: &str,
    pr_id: &str,
) -> Result<PathBuf, Error> {
    let worktree_path = repo_path.join(".cockpit").join("worktrees").join(pr_id);

    if worktree_path.exists() {
        return Err(Error::WorktreeExists(worktree_path));
    }

    // Check if the branch exists locally via git2.
    let repo = Repository::discover(repo_path).map_err(Error::Git2)?;
    let has_local = repo.find_branch(branch, BranchType::Local).is_ok();
    // Also check for a remote tracking branch.
    let remote_ref = format!("origin/{branch}");
    let has_remote = repo.find_branch(&remote_ref, BranchType::Remote).is_ok();
    // Drop the repo before any .await to avoid holding non-Send type across await.
    drop(repo);

    if !has_local && !has_remote {
        // Fetch the branch from origin.
        let output = Command::new("git")
            .current_dir(repo_path)
            .args(["fetch", "origin", branch])
            .output()
            .await
            .map_err(|e| Error::FetchFailed {
                branch: branch.to_string(),
                reason: e.to_string(),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::FetchFailed {
                branch: branch.to_string(),
                reason: stderr.into_owned(),
            });
        }
    }

    // Create the worktree via `git worktree add <path> <branch>`.
    // Using the CLI here because git2's worktree API does not handle
    // remote tracking branches as cleanly as the CLI does.
    let output = Command::new("git")
        .current_dir(repo_path)
        .args(["worktree", "add", &worktree_path.to_string_lossy(), branch])
        .output()
        .await
        .map_err(|e| Error::WorktreeAddFailed {
            branch: branch.to_string(),
            reason: e.to_string(),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::WorktreeAddFailed {
            branch: branch.to_string(),
            reason: stderr.into_owned(),
        });
    }

    Ok(worktree_path)
}

/// Remove a worktree created by [`prepare_worktree`].
///
/// Runs `git worktree remove --force <path>` to clean up the worktree
/// directory and its git metadata.
pub async fn cleanup_worktree(repo_path: &Path, pr_id: &str) -> Result<(), Error> {
    let worktree_path = repo_path.join(".cockpit").join("worktrees").join(pr_id);

    let output = Command::new("git")
        .current_dir(repo_path)
        .args([
            "worktree",
            "remove",
            "--force",
            &worktree_path.to_string_lossy(),
        ])
        .output()
        .await
        .map_err(|e| Error::WorktreeRemoveFailed {
            path: worktree_path.clone(),
            reason: e.to_string(),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::WorktreeRemoveFailed {
            path: worktree_path,
            reason: stderr.into_owned(),
        });
    }

    Ok(())
}

/// Rebase a branch onto a new base.
///
/// This is the core restack operation: after a parent branch is reworked,
/// each descendant branch needs to be rebased onto the parent's new HEAD.
///
/// `fork_point` is the OID where the branch originally forked from its
/// parent. Only commits after this point are replayed. If `None`, the
/// merge-base between the branch and `new_base_branch` is used (correct
/// for the first restack level, but callers doing chained restacks should
/// pass the explicit fork point).
///
/// Uses `git rebase --onto`-style semantics:
/// 1. Identify the fork point (the boundary between inherited and own commits).
/// 2. Collect commits from fork_point..branch_tip (the branch's own commits).
/// 3. Cherry-pick each commit (using proper 3-way merge with each commit's
///    parent as the ancestor) onto the new base.
/// 4. If any cherry-pick has conflicts, abort and return `Ok(false)`.
/// 5. If all clean, move the branch ref and return `Ok(true)`.
///
/// The branch ref is only updated on success — on conflict it remains at its
/// original position.
pub fn restack(
    repo: &Repository,
    branch_name: &str,
    new_base_branch: &str,
    fork_point: Option<Oid>,
) -> Result<bool, Error> {
    let branch = repo
        .find_branch(branch_name, BranchType::Local)
        .map_err(|_| Error::BranchNotFound(branch_name.to_string()))?;
    let branch_tip = branch.get().peel_to_commit()?;
    let branch_tip_oid = branch_tip.id();

    let base_branch = repo
        .find_branch(new_base_branch, BranchType::Local)
        .map_err(|_| Error::BranchNotFound(new_base_branch.to_string()))?;
    let base_tip = base_branch.get().peel_to_commit()?;
    let base_tip_oid = base_tip.id();

    // Determine the fork point: either explicit or computed via merge-base.
    let fork_oid = match fork_point {
        Some(oid) => oid,
        None => repo
            .merge_base(branch_tip_oid, base_tip_oid)
            .map_err(|_| Error::NoMergeBase {
                branch: branch_name.to_string(),
                base: new_base_branch.to_string(),
            })?,
    };

    // If the branch is already based on (or at) the new base tip, nothing to do.
    if fork_oid == base_tip_oid {
        return Ok(true);
    }

    // Collect the branch's own commits (fork_point..branch_tip) in ancestor-first order.
    let commits = collect_commits(repo, fork_oid, branch_tip_oid)?;

    if commits.is_empty() {
        // Branch has no unique commits — just move the ref to the new base.
        let ref_name = format!("refs/heads/{branch_name}");
        repo.reference(
            &ref_name,
            base_tip_oid,
            true,
            &format!("restack: fast-forward {branch_name} to {new_base_branch}"),
        )?;
        return Ok(true);
    }

    // Cherry-pick each commit onto the new base.
    let result = cherry_pick_chain(repo, &commits, base_tip_oid)?;

    match result {
        CherryPickResult::Clean(new_tip_oid) => {
            // Update the branch ref to point at the new tip.
            let ref_name = format!("refs/heads/{branch_name}");
            repo.reference(
                &ref_name,
                new_tip_oid,
                true,
                &format!("restack: rebase {branch_name} onto {new_base_branch}"),
            )?;
            Ok(true)
        }
        CherryPickResult::Conflict => {
            // Branch ref is untouched — we never moved it. Nothing to roll back.
            Ok(false)
        }
    }
}

/// Result of attempting to cherry-pick a chain of commits.
enum CherryPickResult {
    /// All commits applied cleanly; the OID is the new tip.
    Clean(Oid),
    /// At least one commit had conflicts.
    Conflict,
}

/// Collect commits in the range `(base..tip]`, returned in ancestor-first order.
///
/// Walks from `tip` back to (but not including) `base`, then reverses.
fn collect_commits(repo: &Repository, base: Oid, tip: Oid) -> Result<Vec<Oid>, Error> {
    let mut commits = Vec::new();
    let mut current = tip;

    loop {
        if current == base {
            break;
        }
        commits.push(current);
        let commit = repo.find_commit(current)?;
        // Only follow first parent (linear history).
        if commit.parent_count() == 0 {
            break;
        }
        current = commit.parent_id(0)?;
    }

    commits.reverse();
    Ok(commits)
}

/// Cherry-pick a sequence of commits onto `onto_oid`, without moving any branch ref.
///
/// Each cherry-pick is a proper 3-way merge: the commit's parent tree is the
/// ancestor, the current state is "ours", and the commit's tree is "theirs".
/// This correctly handles restacking through multiple levels because each
/// commit's diff is computed relative to its own parent, not relative to
/// some shared ancestor.
///
/// Returns the OID of the final commit if all picks were clean, or `Conflict`
/// if any pick produced merge conflicts.
fn cherry_pick_chain(
    repo: &Repository,
    commits: &[Oid],
    onto_oid: Oid,
) -> Result<CherryPickResult, Error> {
    let mut current_oid = onto_oid;

    for &commit_oid in commits {
        let commit = repo.find_commit(commit_oid)?;
        let current_commit = repo.find_commit(current_oid)?;

        // Proper cherry-pick: 3-way merge using the commit's parent tree
        // as the common ancestor. This isolates each commit's diff so it
        // applies cleanly regardless of how the base was rebased.
        let parent = commit.parent(0)?;
        let ancestor_tree = parent.tree()?;
        let our_tree = current_commit.tree()?;
        let their_tree = commit.tree()?;

        let mut index = repo.merge_trees(&ancestor_tree, &our_tree, &their_tree, None)?;

        if index.has_conflicts() {
            return Ok(CherryPickResult::Conflict);
        }

        // Write the merged tree.
        let tree_oid = index.write_tree_to(repo)?;
        let tree = repo.find_tree(tree_oid)?;

        // Create the replayed commit (preserving author and message).
        let new_oid = repo.commit(
            None, // don't update any ref yet
            &commit.author(),
            &commit.committer(),
            commit.message().unwrap_or(""),
            &tree,
            &[&current_commit],
        )?;

        current_oid = new_oid;
    }

    Ok(CherryPickResult::Clean(current_oid))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a scratch repo with one commit on `main`.
    fn scratch_repo() -> (Repository, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        // git2 init creates a repo with no commits. Create an initial commit
        // on a "main" branch so we have something to base worktrees on.
        {
            let sig = git2::Signature::now("test", "test@test.com").unwrap();
            let tree_oid = repo.treebuilder(None).unwrap().write().unwrap();
            let tree = repo.find_tree(tree_oid).unwrap();
            repo.commit(Some("refs/heads/main"), &sig, &sig, "initial", &tree, &[])
                .unwrap();
        }

        repo.set_head("refs/heads/main").unwrap();

        (repo, dir)
    }

    /// Make a commit in a worktree repo, returning the new OID.
    fn commit_in_worktree(wt_path: &Path) -> git2::Oid {
        let wt_repo = Repository::open(wt_path).unwrap();
        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        let head = wt_repo.head().unwrap().peel_to_commit().unwrap();
        let tree = head.tree().unwrap();
        wt_repo
            .commit(Some("HEAD"), &sig, &sig, "wt commit", &tree, &[&head])
            .unwrap()
    }

    #[test]
    fn ensure_worktree_creates_worktree() {
        let (repo, dir) = scratch_repo();
        let wt_path = dir.path().join("wt-feature");

        let oid = ensure_worktree(&repo, &wt_path, "feature-branch", "main").unwrap();

        // The worktree directory should exist
        assert!(wt_path.exists(), "worktree directory should be created");

        // The returned OID should match main's HEAD
        let main_oid = repo
            .find_branch("main", BranchType::Local)
            .unwrap()
            .get()
            .peel_to_commit()
            .unwrap()
            .id();
        assert_eq!(oid, main_oid);

        // Opening the worktree as a repo should work
        let wt_repo = Repository::open(&wt_path).unwrap();
        let wt_head = wt_repo.head().unwrap().peel_to_commit().unwrap().id();
        assert_eq!(wt_head, oid);
    }

    #[test]
    fn ensure_worktree_stacked_base() {
        let (repo, dir) = scratch_repo();

        // Create a "parent" branch with an extra commit
        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        let main_commit = repo
            .find_branch("main", BranchType::Local)
            .unwrap()
            .get()
            .peel_to_commit()
            .unwrap();
        let tree = main_commit.tree().unwrap();
        let parent_oid = repo
            .commit(
                Some("refs/heads/parent-branch"),
                &sig,
                &sig,
                "parent commit",
                &tree,
                &[&main_commit],
            )
            .unwrap();

        // Create worktree based on parent-branch, not main
        let wt_path = dir.path().join("wt-child");
        let oid = ensure_worktree(&repo, &wt_path, "child-branch", "parent-branch").unwrap();

        assert_eq!(
            oid, parent_oid,
            "worktree should start at parent branch's tip"
        );
    }

    #[test]
    fn ensure_worktree_already_exists() {
        let (repo, dir) = scratch_repo();
        let wt_path = dir.path().join("wt-dup");

        ensure_worktree(&repo, &wt_path, "branch-a", "main").unwrap();

        // Second call should fail
        let err = ensure_worktree(&repo, &wt_path, "branch-b", "main").unwrap_err();
        assert!(
            matches!(err, Error::WorktreeExists(_)),
            "expected WorktreeExists, got {err:?}"
        );
    }

    #[test]
    fn reconcile_reads_head() {
        let (repo, dir) = scratch_repo();
        let wt_path = dir.path().join("wt-recon");
        let initial_oid = ensure_worktree(&repo, &wt_path, "recon-branch", "main").unwrap();

        // Make a new commit in the worktree
        let new_oid = commit_in_worktree(&wt_path);
        assert_ne!(initial_oid, new_oid);

        // Reconcile should return the new OID
        let reconciled = reconcile(&wt_path).unwrap();
        assert_eq!(reconciled, new_oid);
    }

    #[test]
    fn reconcile_nonexistent_path() {
        let result = reconcile(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    #[test]
    fn prune_worktree_removes_worktree() {
        let (repo, dir) = scratch_repo();
        let wt_path = dir.path().join("wt-prune");

        ensure_worktree(&repo, &wt_path, "prune-branch", "main").unwrap();
        assert!(wt_path.exists());

        prune_worktree(&repo, "prune-branch").unwrap();

        // The worktree directory should be gone
        assert!(!wt_path.exists(), "worktree directory should be removed");

        // The worktree should no longer be listed
        let wts = repo.worktrees().unwrap();
        let names: Vec<&str> = wts.iter().filter_map(|x| x.ok().flatten()).collect();
        assert!(
            !names.contains(&"prune-branch"),
            "worktree should be unregistered"
        );
    }

    // ------------------------------------------------------------------
    // Restack tests
    // ------------------------------------------------------------------

    /// Helper: create a file, stage it, and commit.
    fn commit_file(repo: &Repository, path: &str, content: &[u8], message: &str) -> git2::Oid {
        let full_path = repo.workdir().unwrap().join(path);
        std::fs::write(&full_path, content).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new(path)).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])
            .unwrap()
    }

    /// Helper: create a scratch repo with an initial commit that has a real
    /// file, so cherry-picks have something to diff against. Returns the
    /// repo with HEAD on `main`.
    fn scratch_repo_with_file() -> (Repository, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        // Create an initial commit with a file. Scoped to drop borrows
        // before we return `repo`.
        {
            let file_path = dir.path().join("base.txt");
            std::fs::write(&file_path, b"line 1\n").unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(std::path::Path::new("base.txt")).unwrap();
            index.write().unwrap();
            let tree_oid = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_oid).unwrap();
            let sig = git2::Signature::now("test", "test@test.com").unwrap();
            repo.commit(Some("refs/heads/main"), &sig, &sig, "initial", &tree, &[])
                .unwrap();
        }

        repo.set_head("refs/heads/main").unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();

        (repo, dir)
    }

    /// Helper: create a branch at the current HEAD.
    fn create_branch_at_head(repo: &Repository, name: &str) {
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch(name, &head, false).unwrap();
    }

    /// Helper: switch HEAD to a branch.
    fn checkout_branch(repo: &Repository, name: &str) {
        let refname = format!("refs/heads/{name}");
        repo.set_head(&refname).unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
    }

    /// Helper: get the tip OID of a branch.
    fn branch_tip(repo: &Repository, name: &str) -> git2::Oid {
        repo.find_branch(name, BranchType::Local)
            .unwrap()
            .get()
            .peel_to_commit()
            .unwrap()
            .id()
    }

    /// Helper: collect commit messages from tip back to (but not including)
    /// the given stop OID.
    fn commit_messages_since(repo: &Repository, tip: Oid, stop: Oid) -> Vec<String> {
        let mut msgs = Vec::new();
        let mut current = tip;
        loop {
            if current == stop {
                break;
            }
            let commit = repo.find_commit(current).unwrap();
            msgs.push(commit.message().unwrap_or("").to_string());
            if commit.parent_count() == 0 {
                break;
            }
            current = commit.parent_id(0).unwrap();
        }
        msgs.reverse();
        msgs
    }

    #[test]
    fn restack_clean_rebase() {
        let (repo, _dir) = scratch_repo_with_file();

        // Create branch-a from main.
        create_branch_at_head(&repo, "branch-a");
        checkout_branch(&repo, "branch-a");
        commit_file(&repo, "a.txt", b"a content\n", "commit on a");

        // Record where branch-b will fork from branch-a.
        let b_fork = branch_tip(&repo, "branch-a");

        // Create branch-b from branch-a.
        create_branch_at_head(&repo, "branch-b");
        checkout_branch(&repo, "branch-b");
        commit_file(&repo, "b.txt", b"b content\n", "commit on b");

        // Now "rework" branch-a: add another commit.
        checkout_branch(&repo, "branch-a");
        commit_file(&repo, "a2.txt", b"a2 content\n", "rework on a");

        // Restack branch-b onto branch-a's new head.
        let clean = restack(&repo, "branch-b", "branch-a", Some(b_fork)).unwrap();
        assert!(clean, "rebase should be clean");

        // branch-b should now be based on branch-a's new tip.
        let a_tip = branch_tip(&repo, "branch-a");
        let b_tip = branch_tip(&repo, "branch-b");

        // branch-b's parent chain should go through branch-a's tip.
        let b_commit = repo.find_commit(b_tip).unwrap();
        assert_eq!(
            b_commit.parent_id(0).unwrap(),
            a_tip,
            "branch-b's parent should be branch-a's tip after restack"
        );

        // The commit message from branch-b's own commit should be preserved.
        assert_eq!(b_commit.message().unwrap(), "commit on b");

        // branch-b should have the files from a, a's rework, and b.
        let b_tree = b_commit.tree().unwrap();
        assert!(b_tree.get_name("a.txt").is_some(), "a.txt should exist");
        assert!(b_tree.get_name("a2.txt").is_some(), "a2.txt should exist");
        assert!(b_tree.get_name("b.txt").is_some(), "b.txt should exist");
    }

    #[test]
    fn restack_three_pr_stack() {
        let (repo, _dir) = scratch_repo_with_file();

        // Build stack: main → branch-a → branch-b → branch-c.
        create_branch_at_head(&repo, "branch-a");
        checkout_branch(&repo, "branch-a");
        commit_file(&repo, "a.txt", b"a content\n", "commit on a");

        // Record fork points BEFORE creating children.
        let b_fork = branch_tip(&repo, "branch-a");

        create_branch_at_head(&repo, "branch-b");
        checkout_branch(&repo, "branch-b");
        commit_file(&repo, "b.txt", b"b content\n", "commit on b");

        let c_fork = branch_tip(&repo, "branch-b");

        create_branch_at_head(&repo, "branch-c");
        checkout_branch(&repo, "branch-c");
        commit_file(&repo, "c.txt", b"c content\n", "commit on c");

        // Rework branch-a.
        checkout_branch(&repo, "branch-a");
        commit_file(&repo, "a-rework.txt", b"rework\n", "rework on a");

        // Restack b onto a (fork_point = old a tip = b_fork).
        let clean_b = restack(&repo, "branch-b", "branch-a", Some(b_fork)).unwrap();
        assert!(clean_b, "restack b onto a should be clean");

        // Restack c onto b (fork_point = old b tip = c_fork).
        let clean_c = restack(&repo, "branch-c", "branch-b", Some(c_fork)).unwrap();
        assert!(clean_c, "restack c onto b should be clean");

        // Verify chain integrity.
        let a_tip = branch_tip(&repo, "branch-a");
        let b_tip = branch_tip(&repo, "branch-b");
        let c_tip = branch_tip(&repo, "branch-c");

        // c -> b -> a chain
        let c_commit = repo.find_commit(c_tip).unwrap();
        assert_eq!(c_commit.parent_id(0).unwrap(), b_tip);

        let b_commit = repo.find_commit(b_tip).unwrap();
        assert_eq!(b_commit.parent_id(0).unwrap(), a_tip);

        // All files should be in c's tree.
        let c_tree = c_commit.tree().unwrap();
        for name in &["base.txt", "a.txt", "a-rework.txt", "b.txt", "c.txt"] {
            assert!(
                c_tree.get_name(name).is_some(),
                "{name} should exist in branch-c's tree"
            );
        }

        // Verify commit messages are preserved in order.
        let main_oid = branch_tip(&repo, "main");
        let c_msgs = commit_messages_since(&repo, c_tip, main_oid);
        assert_eq!(
            c_msgs,
            vec!["commit on a", "rework on a", "commit on b", "commit on c",]
        );
    }

    #[test]
    fn restack_with_conflict() {
        let (repo, _dir) = scratch_repo_with_file();

        // branch-a modifies base.txt.
        create_branch_at_head(&repo, "branch-a");
        checkout_branch(&repo, "branch-a");
        commit_file(&repo, "base.txt", b"modified by a\n", "a modifies base");

        // branch-b also modifies base.txt (from original main).
        let b_fork = branch_tip(&repo, "main");
        checkout_branch(&repo, "main");
        create_branch_at_head(&repo, "branch-b");
        checkout_branch(&repo, "branch-b");
        commit_file(
            &repo,
            "base.txt",
            b"modified by b (conflict)\n",
            "b modifies base",
        );

        let original_b_tip = branch_tip(&repo, "branch-b");

        // Restack branch-b onto branch-a should conflict.
        let clean = restack(&repo, "branch-b", "branch-a", Some(b_fork)).unwrap();
        assert!(!clean, "rebase should report conflict");

        // Branch should be untouched.
        let after_b_tip = branch_tip(&repo, "branch-b");
        assert_eq!(
            original_b_tip, after_b_tip,
            "branch-b should be unchanged after conflict"
        );
    }

    #[test]
    fn restack_no_op() {
        let (repo, _dir) = scratch_repo_with_file();

        // branch-a is based on main and has one commit.
        create_branch_at_head(&repo, "branch-a");
        checkout_branch(&repo, "branch-a");
        commit_file(&repo, "a.txt", b"a\n", "commit on a");

        let original_tip = branch_tip(&repo, "branch-a");

        // Restack branch-a onto main (already up to date) — merge-base works.
        let clean = restack(&repo, "branch-a", "main", None).unwrap();
        assert!(clean, "no-op restack should return true");

        // Tip should be unchanged.
        assert_eq!(
            branch_tip(&repo, "branch-a"),
            original_tip,
            "branch should not change on no-op restack"
        );
    }

    #[test]
    fn restack_branch_not_found() {
        let (repo, _dir) = scratch_repo_with_file();
        let err = restack(&repo, "nonexistent", "main", None).unwrap_err();
        assert!(
            matches!(err, Error::BranchNotFound(_)),
            "expected BranchNotFound, got {err:?}"
        );
    }

    #[test]
    fn restack_base_not_found() {
        let (repo, _dir) = scratch_repo_with_file();
        create_branch_at_head(&repo, "branch-a");
        let err = restack(&repo, "branch-a", "nonexistent", None).unwrap_err();
        assert!(
            matches!(err, Error::BranchNotFound(_)),
            "expected BranchNotFound, got {err:?}"
        );
    }

    #[test]
    fn restack_without_fork_point_uses_merge_base() {
        let (repo, _dir) = scratch_repo_with_file();

        // Simple case: branch-a from main, rework main, restack branch-a.
        // merge-base works fine here because the history hasn't been rewritten.
        create_branch_at_head(&repo, "branch-a");
        checkout_branch(&repo, "branch-a");
        commit_file(&repo, "a.txt", b"a\n", "commit on a");

        // Add a commit to main.
        checkout_branch(&repo, "main");
        commit_file(&repo, "main2.txt", b"main2\n", "second main commit");

        // Restack branch-a onto main (merge-base computed automatically).
        let clean = restack(&repo, "branch-a", "main", None).unwrap();
        assert!(clean, "restack should be clean");

        // branch-a should now be on top of main's new tip.
        let main_tip = branch_tip(&repo, "main");
        let a_tip = branch_tip(&repo, "branch-a");
        let a_commit = repo.find_commit(a_tip).unwrap();
        assert_eq!(
            a_commit.parent_id(0).unwrap(),
            main_tip,
            "branch-a should be rebased onto main's new tip"
        );
    }

    #[test]
    fn ensure_worktree_bad_base() {
        let (repo, dir) = scratch_repo();
        let wt_path = dir.path().join("wt-bad-base");

        let err = ensure_worktree(&repo, &wt_path, "some-branch", "nonexistent").unwrap_err();
        assert!(
            matches!(err, Error::BranchNotFound(_)),
            "expected BranchNotFound, got {err:?}"
        );
    }

    #[test]
    fn multiple_worktrees() {
        let (repo, dir) = scratch_repo();

        let wt1 = dir.path().join("wt-1");
        let wt2 = dir.path().join("wt-2");

        ensure_worktree(&repo, &wt1, "branch-1", "main").unwrap();
        ensure_worktree(&repo, &wt2, "branch-2", "main").unwrap();

        assert!(wt1.exists());
        assert!(wt2.exists());

        let wts = repo.worktrees().unwrap();
        let names: Vec<&str> = wts.iter().filter_map(|x| x.ok().flatten()).collect();
        assert!(names.contains(&"branch-1"));
        assert!(names.contains(&"branch-2"));

        // Prune one, the other survives
        prune_worktree(&repo, "branch-1").unwrap();
        assert!(!wt1.exists());
        assert!(wt2.exists());
    }

    #[test]
    fn ensure_worktree_creates_correct_branch() {
        let (repo, dir) = scratch_repo();
        let wt_path = dir.path().join("wt-branch-check");

        ensure_worktree(&repo, &wt_path, "my-feature", "main").unwrap();

        // The branch should exist in the parent repo
        let branch = repo.find_branch("my-feature", BranchType::Local);
        assert!(branch.is_ok(), "branch should be created in parent repo");

        // The worktree should be on that branch
        let wt_repo = Repository::open(&wt_path).unwrap();
        let head_ref = wt_repo.head().unwrap();
        assert!(head_ref.is_branch());
        let name = head_ref.shorthand().unwrap();
        assert_eq!(name, "my-feature");
    }

    #[test]
    fn prune_preserves_branch() {
        let (repo, dir) = scratch_repo();
        let wt_path = dir.path().join("wt-keep-branch");

        ensure_worktree(&repo, &wt_path, "keep-branch", "main").unwrap();
        prune_worktree(&repo, "keep-branch").unwrap();

        // Branch should still exist after pruning the worktree
        let branch = repo.find_branch("keep-branch", BranchType::Local);
        assert!(branch.is_ok(), "branch should survive worktree pruning");
    }
}
