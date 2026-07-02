//! Git adapter — worktree management and restack via `git2`.
//!
//! Provides `ensure_worktree`, `reconcile`, `prune_worktree`, and `restack`
//! for the review lifecycle.
//!
//! All operations are synchronous and short-lived. The caller can wrap in
//! `spawn_blocking` if needed in an async context.

use std::path::{Path, PathBuf};

use git2::{BranchType, DiffFormat, Oid, Repository, WorktreeAddOptions, WorktreePruneOptions};
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

    /// Could not determine the user's home directory.
    #[error("could not determine home directory")]
    NoHomeDir,

    /// `gh repo clone` failed for a cross-repo checkout.
    #[error("failed to clone repo `{slug}`: {reason}")]
    CloneFailed {
        /// The GitHub repo slug (e.g. `owner/repo`).
        slug: String,
        /// Human-readable reason for the failure.
        reason: String,
    },

    /// `git checkout` failed.
    #[error("checkout failed for `{branch}`: {reason}")]
    CheckoutFailed {
        /// The branch that was being checked out.
        branch: String,
        /// Human-readable reason for the failure.
        reason: String,
    },

    /// Failed to resolve a cockpit path (e.g. the worktrees directory).
    #[error(transparent)]
    Config(#[from] crate::config::Error),

    /// Underlying git2 error.
    #[error(transparent)]
    Git2(#[from] git2::Error),
}

/// Derive the git worktree registration name from a branch name.
///
/// Git stores worktree metadata under `.git/worktrees/<name>`, so the name
/// cannot contain a `/`. Realistic branches (e.g. `alejandro/nex-123-fix`)
/// have slashes, so we flatten them to dashes for the registration name only;
/// the branch reference itself keeps its real (slashed) name.
fn worktree_name(branch: &str) -> String {
    branch.replace('/', "-")
}

/// Create a git worktree for a review branch, checked out from `base`.
///
/// If `base` is a parent review's branch (stacked PR), the worktree starts
/// from that branch's tip. Returns the OID of the initial HEAD commit.
///
/// The worktree is registered under [`worktree_name`]`(branch)` (slashes
/// flattened to dashes) since git worktree metadata names cannot contain `/`;
/// the branch reference keeps its real name.
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

    repo.worktree(&worktree_name(branch), path, Some(&opts))?;

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

/// Produce a unified diff of the changes between two revisions in a worktree.
///
/// Resolves `from` and `to` as revisions (SHAs, refs, or any revspec git
/// understands) within `worktree` and returns the `from..to` patch in standard
/// unified-diff format — the same `diff --git` / `@@` shape that `gh pr diff`
/// emits, so it parses with the frontend's unified-diff parser.
///
/// Used for interdiff re-review: showing the reviewer only the changes since
/// their last dispatch rather than the full PR again.
///
/// An unknown revision or invalid SHA yields [`Error::Git2`] rather than
/// panicking. Identical `from` and `to` produce an empty string.
pub fn diff_range(worktree: &Path, from: &str, to: &str) -> Result<String, Error> {
    let repo = Repository::open(worktree)?;
    let from_tree = repo.revparse_single(from)?.peel_to_tree()?;
    let to_tree = repo.revparse_single(to)?.peel_to_tree()?;

    let diff = repo.diff_tree_to_tree(Some(&from_tree), Some(&to_tree), None)?;

    let mut patch = String::new();
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        // Context/added/removed lines carry their prefix in `origin`; file and
        // hunk headers already include their prefix in `content`, so only
        // prepend the marker for the three body-line kinds.
        if matches!(line.origin(), ' ' | '+' | '-') {
            patch.push(line.origin());
        }
        patch.push_str(&String::from_utf8_lossy(line.content()));
        true
    })?;

    Ok(patch)
}

/// Maximum blob size served by [`file_at_rev`], in bytes.
///
/// The full-file view feeds Monaco, which degrades badly on very large
/// documents; a blob past this cap yields `Ok(None)` so the UI stays on the
/// diff view rather than trying to render an enormous file.
pub const MAX_FULL_FILE_BYTES: usize = 512 * 1024;

/// Read the text content of a file at a specific revision.
///
/// Opens the repository at `repo_dir` (a worktree or a main checkout — `git2`
/// handles both via `Repository::open`), resolves `rev` to its tree, and looks
/// up `path` within that tree.
///
/// Returns:
/// - `Ok(Some(text))` when the path exists at `rev` and its blob is UTF-8 text
///   within [`MAX_FULL_FILE_BYTES`].
/// - `Ok(None)` when the path is absent at `rev` (an added or deleted file
///   viewed from the wrong side), when the entry is not a plain file, when the
///   blob is binary (contains a NUL byte or is not valid UTF-8 — the full-file
///   view is text-only), or when the blob exceeds [`MAX_FULL_FILE_BYTES`].
/// - `Err(..)` when `rev` itself cannot be resolved, so a mistyped revision is
///   distinguishable from a legitimately absent file.
pub fn file_at_rev(repo_dir: &Path, rev: &str, path: &str) -> Result<Option<String>, Error> {
    let repo = Repository::open(repo_dir)?;
    let tree = repo.revparse_single(rev)?.peel_to_tree()?;

    let entry = match tree.get_path(Path::new(path)) {
        Ok(entry) => entry,
        // A missing path is a normal outcome (added/deleted file), not an error.
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(e) => return Err(Error::Git2(e)),
    };

    let object = entry.to_object(&repo)?;
    // The entry may resolve to a subtree or submodule rather than a file.
    let Some(blob) = object.as_blob() else {
        return Ok(None);
    };

    let content = blob.content();
    if content.len() > MAX_FULL_FILE_BYTES {
        return Ok(None);
    }
    // Binary blobs are not rendered: the full-file view is text-only. A NUL byte
    // is valid UTF-8, so it is checked separately from the UTF-8 decode below.
    if content.contains(&0) {
        return Ok(None);
    }
    match std::str::from_utf8(content) {
        Ok(text) => Ok(Some(text.to_string())),
        Err(_) => Ok(None),
    }
}

/// Remove a worktree and clean up its directory.
///
/// Accepts the branch name and flattens it with [`worktree_name`] to match
/// the registration name used by [`ensure_worktree`]. Passing an
/// already-flat name is a no-op flatten, so both forms work.
pub fn prune_worktree(repo: &Repository, branch: &str) -> Result<(), Error> {
    let wt = repo.find_worktree(&worktree_name(branch))?;
    let mut prune_opts = WorktreePruneOptions::new();
    prune_opts.valid(true).working_tree(true);
    wt.prune(Some(&mut prune_opts))?;
    Ok(())
}

/// Fetch a remote branch if not available locally, then create an isolated worktree.
///
/// The worktree is placed under `<cockpit_home>/worktrees/<pr_id>` (outside the
/// managed repo) and checked out on `branch`. If the branch does not exist
/// locally, `git fetch origin <branch>` is run first via
/// `tokio::process::Command`.
///
/// Returns the path to the created worktree directory.
pub async fn prepare_worktree(
    repo_path: &Path,
    branch: &str,
    pr_id: &str,
) -> Result<PathBuf, Error> {
    let worktree_path = crate::config::worktrees_dir()?.join(pr_id);

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

    // Ensure the worktrees base directory exists (it lives under the cockpit
    // home, not inside the repo, so it may not exist yet).
    if let Some(parent) = worktree_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| Error::WorktreeAddFailed {
                branch: branch.to_string(),
                reason: e.to_string(),
            })?;
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
    let worktree_path = crate::config::worktrees_dir()?.join(pr_id);

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

/// Ensure a branch is checked out on disk so files can be opened in an editor.
///
/// For **cross-repo** PRs (`repo_slug` is `Some`), the repo is cloned into
/// `~/.cockpit/repos/<slug>/` on first access. Subsequent calls reuse the
/// clone. The requested branch is fetched and checked out via a worktree
/// inside the clone so multiple branches can coexist.
///
/// For **same-repo** PRs (`repo_slug` is `None`), a worktree is created
/// under `<cockpit_home>/worktrees/<branch-id>/` if the branch isn't
/// already checked out there.
///
/// Returns the directory root where `<file_path>` can be joined.
pub async fn ensure_branch_checkout(
    repo_path: &Path,
    branch: &str,
    repo_slug: Option<&str>,
) -> Result<PathBuf, Error> {
    let sanitized = branch.replace('/', "-");

    match repo_slug {
        Some(slug) => {
            let home = dirs::home_dir().ok_or(Error::NoHomeDir)?;
            let slug_dir = slug.replace('/', "-");
            let clone_dir = home.join(".cockpit").join("repos").join(&slug_dir);
            let wt_dir = clone_dir.join(".worktrees").join(&sanitized);

            if wt_dir.exists() {
                return Ok(wt_dir);
            }

            if !clone_dir.join(".git").exists() {
                std::fs::create_dir_all(&clone_dir).map_err(|e| Error::CloneFailed {
                    slug: slug.to_string(),
                    reason: e.to_string(),
                })?;

                let output = Command::new("gh")
                    .args([
                        "repo",
                        "clone",
                        slug,
                        &clone_dir.to_string_lossy(),
                        "--",
                        "--no-checkout",
                    ])
                    .output()
                    .await
                    .map_err(|e| Error::CloneFailed {
                        slug: slug.to_string(),
                        reason: e.to_string(),
                    })?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::CloneFailed {
                        slug: slug.to_string(),
                        reason: stderr.into_owned(),
                    });
                }
            }

            let fetch = Command::new("git")
                .current_dir(&clone_dir)
                .args(["fetch", "origin", branch])
                .output()
                .await
                .map_err(|e| Error::FetchFailed {
                    branch: branch.to_string(),
                    reason: e.to_string(),
                })?;

            if !fetch.status.success() {
                let stderr = String::from_utf8_lossy(&fetch.stderr);
                return Err(Error::FetchFailed {
                    branch: branch.to_string(),
                    reason: stderr.into_owned(),
                });
            }

            std::fs::create_dir_all(clone_dir.join(".worktrees")).map_err(|e| {
                Error::WorktreeAddFailed {
                    branch: branch.to_string(),
                    reason: e.to_string(),
                }
            })?;

            let wt_out = Command::new("git")
                .current_dir(&clone_dir)
                .args([
                    "worktree",
                    "add",
                    &wt_dir.to_string_lossy(),
                    &format!("origin/{branch}"),
                ])
                .output()
                .await
                .map_err(|e| Error::WorktreeAddFailed {
                    branch: branch.to_string(),
                    reason: e.to_string(),
                })?;

            if !wt_out.status.success() {
                let stderr = String::from_utf8_lossy(&wt_out.stderr);
                return Err(Error::WorktreeAddFailed {
                    branch: branch.to_string(),
                    reason: stderr.into_owned(),
                });
            }

            Ok(wt_dir)
        }
        None => {
            let wt_path = crate::config::worktrees_dir()?.join(&sanitized);

            if wt_path.exists() {
                return Ok(wt_path);
            }

            prepare_worktree(repo_path, branch, &sanitized).await
        }
    }
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
    fn ensure_worktree_slashed_branch_name() {
        // Realistic branch names contain slashes (e.g. `alejandro/nex-123`).
        // Git worktree metadata names cannot, so the name must be flattened;
        // creation and prune must both succeed and agree on the name.
        let (repo, dir) = scratch_repo();
        let wt_path = dir.path().join("wt-slashed");

        let oid = ensure_worktree(&repo, &wt_path, "alejandro/nex-123-fix", "main").unwrap();
        assert!(
            wt_path.exists(),
            "worktree with slashed branch should be created"
        );

        // The real (slashed) branch ref exists.
        assert!(
            repo.find_branch("alejandro/nex-123-fix", BranchType::Local)
                .is_ok(),
            "the branch ref should keep its slashed name"
        );

        // The worktree is registered under the flattened name.
        assert!(
            repo.find_worktree("alejandro-nex-123-fix").is_ok(),
            "worktree should be registered under the flattened name"
        );

        // Prune accepts the branch name and flattens it internally.
        prune_worktree(&repo, "alejandro/nex-123-fix").unwrap();

        assert_eq!(oid, base_oid(&repo, "main"));
    }

    /// Peel a local branch to its HEAD commit OID (test helper).
    fn base_oid(repo: &Repository, branch: &str) -> git2::Oid {
        repo.find_branch(branch, BranchType::Local)
            .unwrap()
            .get()
            .peel_to_commit()
            .unwrap()
            .id()
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
    fn diff_range_two_commits() {
        let (repo, dir) = scratch_repo_with_file();

        // scratch_repo_with_file seeds base.txt with "line 1\n" on `main`.
        // Replace line 1 and append a line so the patch has both -/+ lines.
        commit_file(&repo, "base.txt", b"line one\nline 2\n", "edit base");

        let patch = diff_range(dir.path(), "HEAD~1", "HEAD").unwrap();

        assert!(
            patch.contains("@@"),
            "diff should contain a hunk header, got:\n{patch}"
        );
        assert!(
            patch.contains("-line 1"),
            "diff should show the removed line, got:\n{patch}"
        );
        assert!(
            patch.contains("+line one"),
            "diff should show the added line, got:\n{patch}"
        );
        assert!(
            patch.contains("base.txt"),
            "diff should name the changed file, got:\n{patch}"
        );
    }

    #[test]
    fn diff_range_unknown_revision() {
        let (_repo, dir) = scratch_repo_with_file();

        // A well-formed but nonexistent SHA must error, not panic.
        let result = diff_range(
            dir.path(),
            "HEAD",
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        );
        assert!(
            matches!(result, Err(Error::Git2(_))),
            "unknown revision should yield Error::Git2, got {result:?}"
        );
    }

    #[test]
    fn diff_range_identical_is_empty() {
        let (_repo, dir) = scratch_repo_with_file();

        let patch = diff_range(dir.path(), "HEAD", "HEAD").unwrap();
        assert!(
            patch.is_empty(),
            "identical revisions should produce an empty diff, got:\n{patch}"
        );
    }

    #[test]
    fn file_at_rev_reads_content_across_revisions() {
        let (repo, dir) = scratch_repo_with_file();
        // scratch_repo_with_file seeds base.txt with "line 1\n" on the initial
        // commit. Add a second revision that changes the same file.
        commit_file(&repo, "base.txt", b"line two\n", "edit base");

        let at_head = file_at_rev(dir.path(), "HEAD", "base.txt").unwrap();
        assert_eq!(at_head.as_deref(), Some("line two\n"), "HEAD content");

        let at_prev = file_at_rev(dir.path(), "HEAD~1", "base.txt").unwrap();
        assert_eq!(at_prev.as_deref(), Some("line 1\n"), "HEAD~1 content");
    }

    #[test]
    fn file_at_rev_absent_path_is_none() {
        let (_repo, dir) = scratch_repo_with_file();
        let result = file_at_rev(dir.path(), "HEAD", "does-not-exist.txt").unwrap();
        assert_eq!(result, None, "an absent path yields Ok(None), not an error");
    }

    #[test]
    fn file_at_rev_bad_rev_errors() {
        let (_repo, dir) = scratch_repo_with_file();
        let result = file_at_rev(dir.path(), "no-such-rev", "base.txt");
        assert!(
            matches!(result, Err(Error::Git2(_))),
            "a bad revision must error, not return None, got {result:?}"
        );
    }

    #[test]
    fn file_at_rev_binary_blob_is_none() {
        let (repo, dir) = scratch_repo_with_file();
        // A blob with an embedded NUL is binary; the text-only view returns None.
        commit_file(&repo, "bin.dat", b"abc\x00def\n", "add binary blob");
        let result = file_at_rev(dir.path(), "HEAD", "bin.dat").unwrap();
        assert_eq!(result, None, "binary blob should return None");
    }

    #[test]
    fn file_at_rev_oversize_blob_is_none() {
        let (repo, dir) = scratch_repo_with_file();
        let big = vec![b'x'; MAX_FULL_FILE_BYTES + 1];
        commit_file(&repo, "big.txt", &big, "add oversize blob");
        let result = file_at_rev(dir.path(), "HEAD", "big.txt").unwrap();
        assert_eq!(result, None, "blob over the cap should return None");
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
