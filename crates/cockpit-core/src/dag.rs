//! Issue DAG utilities — frontier computation and related graph operations.
//!
//! The DAG maps each issue to its **dependencies** (issues it depends on).
//! An issue is in the **frontier** when its dependency list is empty — it has
//! no unmet prerequisites and is ready for work.

use std::collections::HashMap;

use crate::model::IssueRef;

/// Compute frontier issues: those with no unmet dependencies.
///
/// An issue is in the frontier if its dependency list is empty (it depends
/// on nothing). In later phases this will incorporate issue status to exclude
/// already-done issues and consider partially-resolved dependency chains.
pub fn compute_frontier(dag: &HashMap<IssueRef, Vec<IssueRef>>) -> Vec<IssueRef> {
    let mut frontier: Vec<IssueRef> = dag
        .iter()
        .filter(|(_issue, deps)| deps.is_empty())
        .map(|(issue, _deps)| issue.clone())
        .collect();

    // Sort for deterministic output — makes CLI display and tests predictable.
    frontier.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    frontier
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a DAG from a slice of `(issue, &[dependency])` pairs.
    fn dag_from(entries: &[(&str, &[&str])]) -> HashMap<IssueRef, Vec<IssueRef>> {
        entries
            .iter()
            .map(|(issue, deps)| {
                let issue_ref = IssueRef::new(*issue);
                let dep_refs: Vec<IssueRef> = deps.iter().map(|d| IssueRef::new(*d)).collect();
                (issue_ref, dep_refs)
            })
            .collect()
    }

    #[test]
    fn chain_a_b_c_frontier_is_a() {
        // A has no deps, B depends on A, C depends on B.
        let dag = dag_from(&[("A", &[]), ("B", &["A"]), ("C", &["B"])]);

        let frontier = compute_frontier(&dag);

        assert_eq!(frontier, vec![IssueRef::new("A")]);
    }

    #[test]
    fn no_deps_all_in_frontier() {
        let dag = dag_from(&[("X", &[]), ("Y", &[]), ("Z", &[])]);

        let frontier = compute_frontier(&dag);

        assert_eq!(
            frontier,
            vec![IssueRef::new("X"), IssueRef::new("Y"), IssueRef::new("Z")],
        );
    }

    #[test]
    fn diamond_a_b_both_in_frontier() {
        // A and B have no deps; C depends on both A and B.
        let dag = dag_from(&[("A", &[]), ("B", &[]), ("C", &["A", "B"])]);

        let frontier = compute_frontier(&dag);

        assert_eq!(frontier, vec![IssueRef::new("A"), IssueRef::new("B")]);
    }

    #[test]
    fn isolated_node_in_frontier() {
        // D is isolated (no deps, nobody depends on it).
        let dag = dag_from(&[("A", &[]), ("B", &["A"]), ("D", &[])]);

        let frontier = compute_frontier(&dag);

        assert_eq!(frontier, vec![IssueRef::new("A"), IssueRef::new("D")]);
    }

    #[test]
    fn empty_dag() {
        let dag: HashMap<IssueRef, Vec<IssueRef>> = HashMap::new();

        let frontier = compute_frontier(&dag);

        assert!(frontier.is_empty());
    }

    #[test]
    fn single_issue_no_deps() {
        let dag = dag_from(&[("SOLO", &[])]);

        let frontier = compute_frontier(&dag);

        assert_eq!(frontier, vec![IssueRef::new("SOLO")]);
    }

    #[test]
    fn all_issues_have_deps() {
        // Circular-like: every issue depends on something (frontier is empty).
        let dag = dag_from(&[("A", &["B"]), ("B", &["A"])]);

        let frontier = compute_frontier(&dag);

        assert!(frontier.is_empty());
    }
}
