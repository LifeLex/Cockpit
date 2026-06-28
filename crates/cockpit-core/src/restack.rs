//! Restack logic — mark descendants stale and rebase them in dependency order.
//!
//! When a base `Review` enters `Dispatched`, its descendants are marked `stale`
//! so the UI knows not to deep-review them yet. After the base reaches
//! `Reworked`, each descendant is rebased onto the new base in dependency
//! order. See `SPEC.md` §13.
//!
//! The git-level cherry-pick lives in `adapters::git::restack`; this module
//! provides the graph-level helpers that operate on `&mut [Review]`.

use git2::{Oid, Repository};

use crate::adapters::git;
use crate::model::{Review, ReviewId};

/// Mark all descendants of a review as stale.
///
/// Called when a review enters `Dispatched` — its descendants should not be
/// deep-reviewed until the restack completes.
///
/// Walks the `children` edges transitively (breadth-first) and sets
/// `stale = true` on every descendant. The parent itself is *not* marked.
pub fn mark_descendants_stale(reviews: &mut [Review], parent_id: &ReviewId) {
    // Collect children IDs from the parent to seed the traversal.
    let mut queue: Vec<ReviewId> = reviews
        .iter()
        .filter(|r| r.id == *parent_id)
        .flat_map(|r| r.children.clone())
        .collect();

    while let Some(id) = queue.pop() {
        for review in reviews.iter_mut() {
            if review.id == id {
                review.mark_stale();
                // Enqueue this review's children for transitive marking.
                queue.extend(review.children.clone());
                break;
            }
        }
    }
}

/// Attempt to restack a single review onto its parent's current branch head.
///
/// Uses the review's `base_sha` as the fork point to identify which commits
/// belong to this branch. On success, updates `base_sha` to the new base
/// tip and clears `stale`.
///
/// Returns `Ok(true)` if the rebase completed cleanly (clears `stale`),
/// `Ok(false)` if there were conflicts (`stale` remains; the caller should
/// dispatch the conflict-resolver agent).
pub fn restack_review(
    repo: &Repository,
    review: &mut Review,
    parent_branch: &str,
) -> Result<bool, git::Error> {
    let fork_point = Oid::from_str(&review.base_sha).ok();
    let clean = git::restack(repo, &review.branch, parent_branch, fork_point)?;
    if clean {
        // Update base_sha to the current tip of the parent branch so future
        // restacks use the correct fork point.
        let base = repo
            .find_branch(parent_branch, git2::BranchType::Local)
            .map_err(|_| git::Error::BranchNotFound(parent_branch.to_string()))?;
        let base_tip = base.get().peel_to_commit()?;
        review.base_sha = base_tip.id().to_string();
        review.clear_stale();
    }
    Ok(clean)
}

/// Restack all stale descendants of a parent in dependency order.
///
/// Walks the `children` edges transitively (breadth-first from `parent_id`)
/// and restacks each stale descendant onto its parent's branch.
///
/// Returns a list of `(ReviewId, bool)` pairs — `true` if the restack was
/// clean, `false` if there were conflicts for that review.
pub fn restack_descendants(
    repo: &Repository,
    reviews: &mut [Review],
    parent_id: &ReviewId,
) -> Result<Vec<(ReviewId, bool)>, git::Error> {
    // Build the ordered list of (child_id, parent_branch) pairs to process.
    // We traverse breadth-first from parent_id so parents are restacked before
    // their own children.
    let order = dependency_order(reviews, parent_id);
    let mut results = Vec::new();

    for (child_id, parent_branch) in order {
        // Find the child review and restack it.
        // We need to work with indices because we borrow `reviews` mutably.
        let idx = reviews.iter().position(|r| r.id == child_id);
        if let Some(i) = idx {
            if reviews[i].stale {
                let fork_point = Oid::from_str(&reviews[i].base_sha).ok();
                let clean = git::restack(repo, &reviews[i].branch, &parent_branch, fork_point)?;
                if clean {
                    // Update base_sha so future restacks use the correct fork point.
                    if let Ok(base) = repo.find_branch(&parent_branch, git2::BranchType::Local) {
                        if let Ok(tip) = base.get().peel_to_commit() {
                            reviews[i].base_sha = tip.id().to_string();
                        }
                    }
                    reviews[i].clear_stale();
                }
                results.push((child_id, clean));
            }
        }
    }

    Ok(results)
}

/// Build a breadth-first traversal order of descendants with their parent branch names.
///
/// Returns `(child_id, parent_branch)` pairs in BFS order, guaranteeing that
/// every review's parent is processed before the review itself.
fn dependency_order(reviews: &[Review], root_id: &ReviewId) -> Vec<(ReviewId, String)> {
    let mut order = Vec::new();

    // Find root's branch and children.
    let root = reviews.iter().find(|r| r.id == *root_id);
    let Some(root) = root else {
        return order;
    };

    let mut queue: Vec<(ReviewId, String)> = root
        .children
        .iter()
        .map(|child_id| (child_id.clone(), root.branch.clone()))
        .collect();

    while let Some((child_id, parent_branch)) = queue.first().cloned() {
        queue.remove(0);
        order.push((child_id.clone(), parent_branch));

        // Find this child and enqueue its children.
        if let Some(child) = reviews.iter().find(|r| r.id == child_id) {
            for grandchild_id in &child.children {
                queue.push((grandchild_id.clone(), child.branch.clone()));
            }
        }
    }

    order
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::{DiffData, GateState, IssueRef, PrRef, ReviewId};

    /// Build a minimal `Review` with the given id, parents, children, and branch.
    fn make_review(
        id: &str,
        branch: &str,
        base: &str,
        parents: &[&str],
        children: &[&str],
    ) -> Review {
        Review {
            id: ReviewId::new(id),
            issue: IssueRef::new(format!("ISSUE-{id}")),
            pr: PrRef::new(format!("owner/repo#{id}")),
            branch: branch.to_string(),
            base: base.to_string(),
            base_sha: "000".into(),
            worktree: PathBuf::from(format!("/tmp/wt-{id}")),
            gate_state: GateState::Pending,
            diff: DiffData { raw: String::new() },
            head_sha: "aaa".into(),
            comments: vec![],
            parents: parents.iter().map(|s| ReviewId::new(*s)).collect(),
            children: children.iter().map(|s| ReviewId::new(*s)).collect(),
            stale: false,
            agent: None,
        }
    }

    #[test]
    fn mark_descendants_stale_linear_chain() {
        //  A → B → C  (A is the root)
        let mut reviews = vec![
            make_review("a", "branch-a", "main", &[], &["b"]),
            make_review("b", "branch-b", "branch-a", &["a"], &["c"]),
            make_review("c", "branch-c", "branch-b", &["b"], &[]),
        ];

        mark_descendants_stale(&mut reviews, &ReviewId::new("a"));

        assert!(!reviews[0].stale, "root should not be stale");
        assert!(reviews[1].stale, "child b should be stale");
        assert!(reviews[2].stale, "grandchild c should be stale");
    }

    #[test]
    fn mark_descendants_stale_diamond() {
        //    A
        //   / \
        //  B   C
        //   \ /
        //    D
        let mut reviews = vec![
            make_review("a", "branch-a", "main", &[], &["b", "c"]),
            make_review("b", "branch-b", "branch-a", &["a"], &["d"]),
            make_review("c", "branch-c", "branch-a", &["a"], &["d"]),
            make_review("d", "branch-d", "branch-b", &["b", "c"], &[]),
        ];

        mark_descendants_stale(&mut reviews, &ReviewId::new("a"));

        assert!(!reviews[0].stale, "root a should not be stale");
        assert!(reviews[1].stale, "b should be stale");
        assert!(reviews[2].stale, "c should be stale");
        assert!(reviews[3].stale, "d should be stale");
    }

    #[test]
    fn mark_descendants_stale_from_middle() {
        //  A → B → C
        let mut reviews = vec![
            make_review("a", "branch-a", "main", &[], &["b"]),
            make_review("b", "branch-b", "branch-a", &["a"], &["c"]),
            make_review("c", "branch-c", "branch-b", &["b"], &[]),
        ];

        mark_descendants_stale(&mut reviews, &ReviewId::new("b"));

        assert!(!reviews[0].stale, "a should not be stale");
        assert!(!reviews[1].stale, "b (the parent) should not be stale");
        assert!(reviews[2].stale, "c should be stale");
    }

    #[test]
    fn mark_descendants_stale_no_children() {
        let mut reviews = vec![make_review("a", "branch-a", "main", &[], &[])];

        mark_descendants_stale(&mut reviews, &ReviewId::new("a"));

        assert!(!reviews[0].stale, "lone review should not be stale");
    }

    #[test]
    fn mark_descendants_stale_unknown_parent() {
        let mut reviews = vec![make_review("a", "branch-a", "main", &[], &[])];

        // Should not panic on unknown parent ID.
        mark_descendants_stale(&mut reviews, &ReviewId::new("nonexistent"));

        assert!(!reviews[0].stale);
    }

    /// Helper: create a file, stage it, and commit in a repo.
    fn commit_file(
        repo: &git2::Repository,
        path: &str,
        content: &[u8],
        message: &str,
    ) -> git2::Oid {
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

    /// Helper: create a scratch repo with an initial commit on main.
    fn scratch_repo() -> (git2::Repository, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
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
    fn create_branch(repo: &git2::Repository, name: &str) {
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch(name, &head, false).unwrap();
    }

    /// Helper: switch HEAD to a branch.
    fn checkout(repo: &git2::Repository, name: &str) {
        let refname = format!("refs/heads/{name}");
        repo.set_head(&refname).unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
    }

    /// Helper: get the tip OID of a branch.
    fn branch_tip(repo: &git2::Repository, name: &str) -> git2::Oid {
        repo.find_branch(name, git2::BranchType::Local)
            .unwrap()
            .get()
            .peel_to_commit()
            .unwrap()
            .id()
    }

    #[test]
    fn clear_stale_on_clean_restack() {
        let (repo, _dir) = scratch_repo();

        // Create branch-a from main.
        create_branch(&repo, "branch-a");
        checkout(&repo, "branch-a");
        commit_file(&repo, "a.txt", b"a content\n", "commit on a");

        // Record fork point for branch-b.
        let b_fork = branch_tip(&repo, "branch-a");

        // Create branch-b from branch-a.
        create_branch(&repo, "branch-b");
        checkout(&repo, "branch-b");
        commit_file(&repo, "b.txt", b"b content\n", "commit on b");

        // Rework branch-a.
        checkout(&repo, "branch-a");
        commit_file(&repo, "a2.txt", b"a2\n", "rework on a");

        // Create a stale review for branch-b with the correct base_sha.
        let mut review = make_review("b", "branch-b", "branch-a", &["a"], &[]);
        review.base_sha = b_fork.to_string();
        review.mark_stale();
        assert!(review.stale);

        // Restack and verify stale is cleared.
        let clean = restack_review(&repo, &mut review, "branch-a").unwrap();
        assert!(clean, "rebase should be clean");
        assert!(!review.stale, "stale should be cleared after clean restack");

        // base_sha should be updated to the new branch-a tip.
        let a_tip = branch_tip(&repo, "branch-a");
        assert_eq!(
            review.base_sha,
            a_tip.to_string(),
            "base_sha should be updated after restack"
        );
    }

    #[test]
    fn dependency_order_linear() {
        let reviews = vec![
            make_review("a", "branch-a", "main", &[], &["b"]),
            make_review("b", "branch-b", "branch-a", &["a"], &["c"]),
            make_review("c", "branch-c", "branch-b", &["b"], &[]),
        ];

        let order = dependency_order(&reviews, &ReviewId::new("a"));

        assert_eq!(order.len(), 2);
        assert_eq!(order[0].0, ReviewId::new("b"));
        assert_eq!(order[0].1, "branch-a");
        assert_eq!(order[1].0, ReviewId::new("c"));
        assert_eq!(order[1].1, "branch-b");
    }

    #[test]
    fn dependency_order_empty() {
        let reviews = vec![make_review("a", "branch-a", "main", &[], &[])];

        let order = dependency_order(&reviews, &ReviewId::new("a"));

        assert!(order.is_empty());
    }
}
