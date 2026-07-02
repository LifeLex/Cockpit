//! Linear GraphQL adapter — read-only access to project issues and dependency relations.
//!
//! Fetches a Linear project's issues and their `blocks` / `is_blocked_by` relations,
//! then builds the dependency DAG that cockpit uses for stacking order and frontier
//! computation.
//!
//! cockpit writes nothing to Linear in v1 (see `SPEC.md` §14).

use std::collections::HashMap;

use serde::Deserialize;

use crate::model::{IssueRef, ProjectRef};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors from Linear API operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The Linear API returned a non-success response or a GraphQL-level error.
    #[error("Linear API request failed: {0}")]
    Api(String),

    /// The response JSON could not be parsed into the expected shape.
    #[error("failed to parse Linear response: {0}")]
    Parse(String),

    /// The requested project was not found on Linear.
    #[error("project not found: {0}")]
    NotFound(ProjectRef),

    /// Underlying HTTP transport error.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

// ---------------------------------------------------------------------------
// GraphQL query
// ---------------------------------------------------------------------------

/// GraphQL query to fetch project issues and their dependency relations.
///
/// Fetches issues through the project's `issues` connection with enough
/// fields for cockpit's DAG, and the `relations` connection on each issue
/// to discover `blocks` / `is_blocked_by` edges.
const GRAPHQL_QUERY: &str = r#"
query ProjectIssues($projectId: String!) {
  project(id: $projectId) {
    issues(first: 250) {
      nodes {
        id
        identifier
        title
        description
        branchName
      }
    }
    issueRelations: issues(first: 250) {
      nodes {
        id
        identifier
        relations(first: 50) {
          nodes {
            type
            issue {
              id
              identifier
            }
            relatedIssue {
              id
              identifier
            }
          }
        }
      }
    }
  }
}
"#;

// ---------------------------------------------------------------------------
// Response deserialization types
// ---------------------------------------------------------------------------

/// A single issue as returned by the Linear GraphQL API.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueNode {
    /// Linear's internal UUID for the issue.
    pub id: String,
    /// Human-readable identifier (e.g. `NEX-123`).
    pub identifier: String,
    /// Issue title.
    pub title: String,
    /// Issue description (markdown). Defaulted so an issue with no description —
    /// and legacy fixtures that predate this field — parse to an empty string.
    #[serde(default)]
    pub description: String,
    /// The git branch name Linear generates for the issue.
    pub branch_name: String,
}

/// A relation between two issues as returned by the Linear GraphQL API.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationNode {
    /// The type of relation: `"blocks"`, `"is_blocked_by"`, etc.
    #[serde(rename = "type")]
    pub relation_type: String,
    /// The "source" issue in the relation.
    pub issue: RelatedIssue,
    /// The "target" issue in the relation.
    pub related_issue: RelatedIssue,
}

/// A minimal issue reference inside a relation edge.
#[derive(Debug, Clone, Deserialize)]
pub struct RelatedIssue {
    /// Linear's internal UUID.
    pub id: String,
    /// Human-readable identifier (e.g. `NEX-123`).
    pub identifier: String,
}

/// Parsed project data: the issues and their dependency relations.
#[derive(Debug, Clone)]
pub struct ProjectData {
    /// All issues in the project.
    pub issues: Vec<IssueNode>,
    /// All inter-issue relations found.
    pub relations: Vec<RelationNode>,
}

// Internal deserialization wrappers that mirror the GraphQL response shape.

#[derive(Debug, Deserialize)]
struct GraphQlResponse {
    data: Option<ProjectDataResponse>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct ProjectDataResponse {
    project: Option<ProjectNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectNode {
    issues: IssueConnection,
    issue_relations: IssueRelationConnection,
}

#[derive(Debug, Deserialize)]
struct IssueConnection {
    nodes: Vec<IssueNode>,
}

#[derive(Debug, Deserialize)]
struct IssueRelationConnection {
    nodes: Vec<IssueWithRelations>,
}

#[derive(Debug, Deserialize)]
struct IssueWithRelations {
    #[expect(dead_code)]
    id: String,
    #[expect(dead_code)]
    identifier: String,
    relations: RelationConnection,
}

#[derive(Debug, Deserialize)]
struct RelationConnection {
    nodes: Vec<RelationNode>,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Fetch a Linear project's issues and dependency relations via GraphQL.
///
/// Sends the [`GRAPHQL_QUERY`] to the Linear API and parses the response
/// into a [`ProjectData`] holding all issues and relations. The caller
/// supplies a pre-configured `reqwest::Client` and API key.
pub async fn fetch_project_issues(
    client: &reqwest::Client,
    api_key: &str,
    project_ref: &ProjectRef,
) -> Result<ProjectData, Error> {
    let body = serde_json::json!({
        "query": GRAPHQL_QUERY,
        "variables": {
            "projectId": project_ref.as_str(),
        }
    });

    let response = client
        .post("https://api.linear.app/graphql")
        .header("Authorization", api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(Error::Api(format!(
            "HTTP {} from Linear API",
            response.status()
        )));
    }

    let gql_response: GraphQlResponse = response
        .json()
        .await
        .map_err(|e| Error::Parse(e.to_string()))?;

    // Surface GraphQL-level errors.
    if let Some(errors) = gql_response.errors {
        let messages: Vec<&str> = errors.iter().map(|e| e.message.as_str()).collect();
        return Err(Error::Api(messages.join("; ")));
    }

    let project = gql_response
        .data
        .and_then(|d| d.project)
        .ok_or_else(|| Error::NotFound(project_ref.clone()))?;

    // Flatten relations from all issues into a single list, deduplicating
    // by identity (the same relation can appear on both sides).
    let relations: Vec<RelationNode> = project
        .issue_relations
        .nodes
        .into_iter()
        .flat_map(|issue| issue.relations.nodes)
        .collect();

    Ok(ProjectData {
        issues: project.issues.nodes,
        relations,
    })
}

/// Build a dependency adjacency list from the project's issue relations.
///
/// A `"blocks"` relation means issue A blocks issue B: A must complete
/// before B. The resulting DAG maps each issue to its **dependencies**
/// (parents it depends on), so the edge direction is `B -> [A]`.
///
/// For `"blocks"`: `issue` blocks `related_issue`, so `related_issue`
/// depends on `issue` (edge: `related_issue -> issue`).
///
/// For `"is_blocked_by"`: `issue` is blocked by `related_issue`, so
/// `issue` depends on `related_issue` (edge: `issue -> related_issue`).
///
/// All issues appear as keys in the map, even those with no dependencies.
pub fn build_issue_dag(data: &ProjectData) -> HashMap<IssueRef, Vec<IssueRef>> {
    let mut dag: HashMap<IssueRef, Vec<IssueRef>> = HashMap::new();

    // Seed every issue as a key so isolated nodes appear.
    for issue in &data.issues {
        dag.entry(IssueRef::new(&issue.identifier)).or_default();
    }

    for relation in &data.relations {
        match relation.relation_type.as_str() {
            "blocks" => {
                // issue blocks related_issue => related_issue depends on issue
                let blocker = IssueRef::new(&relation.issue.identifier);
                let blocked = IssueRef::new(&relation.related_issue.identifier);
                dag.entry(blocked).or_default().push(blocker);
            }
            "is_blocked_by" => {
                // issue is_blocked_by related_issue => issue depends on related_issue
                let dependent = IssueRef::new(&relation.issue.identifier);
                let dependency = IssueRef::new(&relation.related_issue.identifier);
                dag.entry(dependent).or_default().push(dependency);
            }
            _ => {
                // Other relation types (e.g. "related", "duplicate") are
                // not dependency edges; ignore them for the DAG.
            }
        }
    }

    dag
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse the full GraphQL response JSON into the internal response types,
    /// then extract `ProjectData`.
    fn parse_fixture(json: &str) -> ProjectData {
        let gql: GraphQlResponse =
            serde_json::from_str(json).unwrap_or_else(|e| panic!("fixture should parse: {e}"));

        let project = gql
            .data
            .expect("fixture should have data")
            .project
            .expect("fixture should have project");

        let relations: Vec<RelationNode> = project
            .issue_relations
            .nodes
            .into_iter()
            .flat_map(|issue| issue.relations.nodes)
            .collect();

        ProjectData {
            issues: project.issues.nodes,
            relations,
        }
    }

    /// A fixture with 3 issues and 2 "blocks" relations:
    ///   NEX-1 blocks NEX-2, NEX-2 blocks NEX-3 (chain: 1 -> 2 -> 3).
    const FIXTURE_THREE_ISSUES: &str = r#"
    {
        "data": {
            "project": {
                "issues": {
                    "nodes": [
                        { "id": "id-1", "identifier": "NEX-1", "title": "First", "description": "Define the trait and its contract.", "branchName": "alejandro/nex-1-first" },
                        { "id": "id-2", "identifier": "NEX-2", "title": "Second", "branchName": "alejandro/nex-2-second" },
                        { "id": "id-3", "identifier": "NEX-3", "title": "Third", "branchName": "alejandro/nex-3-third" }
                    ]
                },
                "issueRelations": {
                    "nodes": [
                        {
                            "id": "id-1",
                            "identifier": "NEX-1",
                            "relations": {
                                "nodes": [
                                    {
                                        "type": "blocks",
                                        "issue": { "id": "id-1", "identifier": "NEX-1" },
                                        "relatedIssue": { "id": "id-2", "identifier": "NEX-2" }
                                    }
                                ]
                            }
                        },
                        {
                            "id": "id-2",
                            "identifier": "NEX-2",
                            "relations": {
                                "nodes": [
                                    {
                                        "type": "blocks",
                                        "issue": { "id": "id-2", "identifier": "NEX-2" },
                                        "relatedIssue": { "id": "id-3", "identifier": "NEX-3" }
                                    }
                                ]
                            }
                        },
                        {
                            "id": "id-3",
                            "identifier": "NEX-3",
                            "relations": { "nodes": [] }
                        }
                    ]
                }
            }
        }
    }
    "#;

    #[test]
    fn parse_fixture_into_project_data() {
        let data = parse_fixture(FIXTURE_THREE_ISSUES);
        assert_eq!(data.issues.len(), 3);
        assert_eq!(data.relations.len(), 2);

        // Verify deserialized fields.
        let first = &data.issues[0];
        assert_eq!(first.identifier, "NEX-1");
        assert_eq!(first.title, "First");
        assert_eq!(first.description, "Define the trait and its contract.");
        assert_eq!(first.branch_name, "alejandro/nex-1-first");

        // NEX-2 omits `description`; serde default yields an empty string.
        assert_eq!(data.issues[1].description, "");
    }

    #[test]
    fn dag_with_two_blocks_relations() {
        // NEX-1 blocks NEX-2, NEX-2 blocks NEX-3
        // => NEX-2 depends on NEX-1, NEX-3 depends on NEX-2
        let data = parse_fixture(FIXTURE_THREE_ISSUES);
        let dag = build_issue_dag(&data);

        assert_eq!(dag.len(), 3, "all 3 issues should be keys");

        // NEX-1 has no dependencies (root)
        assert!(dag[&IssueRef::new("NEX-1")].is_empty());

        // NEX-2 depends on NEX-1
        assert_eq!(dag[&IssueRef::new("NEX-2")], vec![IssueRef::new("NEX-1")]);

        // NEX-3 depends on NEX-2
        assert_eq!(dag[&IssueRef::new("NEX-3")], vec![IssueRef::new("NEX-2")]);
    }

    #[test]
    fn dag_with_no_relations() {
        let fixture = r#"
        {
            "data": {
                "project": {
                    "issues": {
                        "nodes": [
                            { "id": "id-a", "identifier": "NEX-10", "title": "Alpha", "branchName": "alpha" },
                            { "id": "id-b", "identifier": "NEX-11", "title": "Beta", "branchName": "beta" }
                        ]
                    },
                    "issueRelations": {
                        "nodes": [
                            {
                                "id": "id-a",
                                "identifier": "NEX-10",
                                "relations": { "nodes": [] }
                            },
                            {
                                "id": "id-b",
                                "identifier": "NEX-11",
                                "relations": { "nodes": [] }
                            }
                        ]
                    }
                }
            }
        }
        "#;

        let data = parse_fixture(fixture);
        let dag = build_issue_dag(&data);

        assert_eq!(dag.len(), 2, "both issues should be keys");
        assert!(dag[&IssueRef::new("NEX-10")].is_empty());
        assert!(dag[&IssueRef::new("NEX-11")].is_empty());
    }

    #[test]
    fn dag_chain_a_b_c() {
        // A blocks B, B blocks C => chain: C depends on B depends on A
        let data = parse_fixture(FIXTURE_THREE_ISSUES);
        let dag = build_issue_dag(&data);

        // Walk the chain from leaf to root.
        let c_deps = &dag[&IssueRef::new("NEX-3")];
        assert_eq!(c_deps.len(), 1);
        assert_eq!(c_deps[0], IssueRef::new("NEX-2"));

        let b_deps = &dag[&IssueRef::new("NEX-2")];
        assert_eq!(b_deps.len(), 1);
        assert_eq!(b_deps[0], IssueRef::new("NEX-1"));

        let a_deps = &dag[&IssueRef::new("NEX-1")];
        assert!(a_deps.is_empty(), "root should have no dependencies");
    }

    #[test]
    fn dag_with_is_blocked_by_relation() {
        // NEX-20 is_blocked_by NEX-21 => NEX-20 depends on NEX-21
        let fixture = r#"
        {
            "data": {
                "project": {
                    "issues": {
                        "nodes": [
                            { "id": "id-20", "identifier": "NEX-20", "title": "Dependent", "branchName": "dep" },
                            { "id": "id-21", "identifier": "NEX-21", "title": "Blocker", "branchName": "blocker" }
                        ]
                    },
                    "issueRelations": {
                        "nodes": [
                            {
                                "id": "id-20",
                                "identifier": "NEX-20",
                                "relations": {
                                    "nodes": [
                                        {
                                            "type": "is_blocked_by",
                                            "issue": { "id": "id-20", "identifier": "NEX-20" },
                                            "relatedIssue": { "id": "id-21", "identifier": "NEX-21" }
                                        }
                                    ]
                                }
                            },
                            {
                                "id": "id-21",
                                "identifier": "NEX-21",
                                "relations": { "nodes": [] }
                            }
                        ]
                    }
                }
            }
        }
        "#;

        let data = parse_fixture(fixture);
        let dag = build_issue_dag(&data);

        // NEX-20 depends on NEX-21
        assert_eq!(dag[&IssueRef::new("NEX-20")], vec![IssueRef::new("NEX-21")]);
        assert!(dag[&IssueRef::new("NEX-21")].is_empty());
    }

    #[test]
    fn unknown_relation_types_are_ignored() {
        let fixture = r#"
        {
            "data": {
                "project": {
                    "issues": {
                        "nodes": [
                            { "id": "id-a", "identifier": "NEX-30", "title": "A", "branchName": "a" },
                            { "id": "id-b", "identifier": "NEX-31", "title": "B", "branchName": "b" }
                        ]
                    },
                    "issueRelations": {
                        "nodes": [
                            {
                                "id": "id-a",
                                "identifier": "NEX-30",
                                "relations": {
                                    "nodes": [
                                        {
                                            "type": "related",
                                            "issue": { "id": "id-a", "identifier": "NEX-30" },
                                            "relatedIssue": { "id": "id-b", "identifier": "NEX-31" }
                                        }
                                    ]
                                }
                            },
                            {
                                "id": "id-b",
                                "identifier": "NEX-31",
                                "relations": { "nodes": [] }
                            }
                        ]
                    }
                }
            }
        }
        "#;

        let data = parse_fixture(fixture);
        let dag = build_issue_dag(&data);

        // "related" is not a dependency edge; both should have empty deps.
        assert!(dag[&IssueRef::new("NEX-30")].is_empty());
        assert!(dag[&IssueRef::new("NEX-31")].is_empty());
    }

    #[test]
    fn graphql_errors_are_surfaced() {
        let fixture = r#"
        {
            "errors": [
                { "message": "Authentication required" }
            ]
        }
        "#;

        let gql: GraphQlResponse = serde_json::from_str(fixture).expect("should parse");
        assert!(gql.errors.is_some());
        assert_eq!(
            gql.errors.as_ref().unwrap()[0].message,
            "Authentication required"
        );
        assert!(gql.data.is_none());
    }

    #[test]
    fn missing_project_yields_none() {
        let fixture = r#"
        {
            "data": {
                "project": null
            }
        }
        "#;

        let gql: GraphQlResponse = serde_json::from_str(fixture).expect("should parse");
        let project = gql.data.expect("data present").project;
        assert!(project.is_none(), "null project should parse as None");
    }
}
