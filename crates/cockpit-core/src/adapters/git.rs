//! Git adapter — worktree management via `git2`.
//!
//! Provides `ensure_worktree`, `reconcile`, and `prune_worktree` for the
//! review lifecycle. Restack is stubbed until Phase 3.
//!
//! All operations are synchronous and short-lived. The caller can wrap in
//! `spawn_blocking` if needed in an async context.

use std::path::Path;

use git2::{BranchType, Repository, WorktreeAddOptions, WorktreePruneOptions};

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

    /// Restack is not yet implemented (Phase 3).
    #[error("restack is not yet implemented (Phase 3)")]
    RestackNotImplemented,

    /// Rebase hit conflicts.
    #[error("rebase hit conflicts in {count} files")]
    RebaseConflict {
        /// Number of files with conflicts.
        count: usize,
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

/// Restack a worktree onto a new base. **Not yet implemented (Phase 3).**
pub fn restack(_repo: &Repository, _worktree_path: &Path, _new_base: &str) -> Result<(), Error> {
    Err(Error::RestackNotImplemented)
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

    #[test]
    fn restack_returns_not_implemented() {
        let (repo, dir) = scratch_repo();
        let err = restack(&repo, dir.path(), "main").unwrap_err();
        assert!(
            matches!(err, Error::RestackNotImplemented),
            "expected RestackNotImplemented, got {err:?}"
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
