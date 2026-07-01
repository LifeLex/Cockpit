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
  };
  return { ...base, ...overrides };
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

/** The five gate states, for exhaustive iteration in tests. */
export const ALL_GATE_STATES = [
  "Pending",
  "InReview",
  "Dispatched",
  "Reworked",
  "Approved",
] as const satisfies readonly GateState[];
