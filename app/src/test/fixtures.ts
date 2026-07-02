/**
 * Fully-typed fixtures for the domain types crossing the IPC boundary. Built
 * against the generated `bindings/*` types so a schema change breaks the tests
 * at compile time (the point of ts-rs codegen). Each factory takes a
 * `Partial<T>` of overrides so a test states only the fields it cares about.
 */
import type { Review } from "../bindings/Review";
import type { GateState } from "../bindings/GateState";
import type { CiCheck } from "../bindings/CiCheck";
import type { AgentRun } from "../bindings/AgentRun";
import type { ConversationItem } from "../bindings/ConversationItem";
import type { ReviewFinding } from "../bindings/ReviewFinding";

/** A minimal running agent, for the stale/agent-active edges. */
export function makeAgentRun(overrides: Partial<AgentRun> = {}): AgentRun {
  return {
    pid: 4242,
    mode: "Fix",
    started_at: { secs_since_epoch: 1_700_000_000, nanos_since_epoch: 0 },
    prompt_hash: "deadbeef",
    log_path: "/tmp/agent.log",
    ...overrides,
  };
}

/**
 * A valid `Review`. `pr`/`id`/`issue` are branded newtypes in Rust but plain
 * strings on the wire; the fixture supplies concrete strings and the factory
 * signature keeps them typed as `Review`.
 */
export function makeReview(overrides: Partial<Review> = {}): Review {
  const base: Review = {
    id: "rev-1",
    issue: "NEX-1",
    pr: "https://github.com/o/r/pull/1",
    title: "Do the thing",
    body: "",
    branch: "alejandro/nex-1-do-thing",
    base: "main",
    base_sha: "abc123",
    source: "Authored",
    worktree: "/tmp/wt/rev-1",
    gate_state: "InReview",
    diff: { raw: "" },
    head_sha: "def456",
    comments: [],
    parents: [],
    children: [],
    stale: false,
    agent: null,
    repo_slug: "o/r",
    project: null,
    review_findings: [],
    conversation: [],
    // Serde emits `Option::None` as `null` with the key present — fixtures
    // must mirror the wire shape, not omit the fields (omission hid a
    // null-deref crash the type system couldn't see).
    dispatch_snapshot: null,
    ci_summary: null,
    last_reviewed_sha: null,
  };
  return { ...base, ...overrides };
}

/** A valid read-only `ConversationItem` (E1), an issue comment by default. */
export function makeConversationItem(
  overrides: Partial<ConversationItem> = {},
): ConversationItem {
  return {
    id: "conv-1",
    kind: "IssueComment",
    author: "octocat",
    body: "Looks reasonable to me.",
    path: null,
    line: null,
    side: null,
    state: null,
    created_at: "2026-07-01T12:00:00Z",
    url: "https://github.com/o/r/pull/1#issuecomment-1",
    ...overrides,
  };
}

/**
 * A valid advisory `ReviewFinding` — a New-side Warning on lines 20–21 by
 * default. Pass `range: [0, 0]` for a file-level finding.
 */
export function makeFinding(
  overrides: Partial<ReviewFinding> = {},
): ReviewFinding {
  return {
    id: "finding-1",
    severity: "Warning",
    path: "src/lib.rs",
    range: [20, 21],
    side: "New",
    title: "Possible off-by-one",
    rationale: "The loop bound looks inclusive where an exclusive one is meant.",
    ...overrides,
  };
}

/** A valid `CiCheck` with pass defaults. */
export function makeCheck(overrides: Partial<CiCheck> = {}): CiCheck {
  return {
    name: "build",
    state: "SUCCESS",
    bucket: "pass",
    link: "https://github.com/o/r/runs/1",
    workflow: "CI",
    ...overrides,
  };
}

/** Every gate state, for exhaustive iteration in tests. */
export const ALL_GATE_STATES = [
  "Pending",
  "InReview",
  "Dispatched",
  "Reworked",
  "Approved",
  "Merged",
] as const satisfies readonly GateState[];
