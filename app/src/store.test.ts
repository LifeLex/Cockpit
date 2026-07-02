import { describe, it, expect, beforeEach, vi } from "vitest";
import type { Review } from "./bindings/Review";
import type { CiCheck } from "./bindings/CiCheck";
import type { SubmitReviewResult } from "./bindings/SubmitReviewResult";
import type { EvidenceSummary } from "./bindings/EvidenceSummary";
import type { FilePair } from "./bindings/FilePair";
import {
  mockInvoke,
  mockInvokeReject,
  callsFor,
} from "./test/tauri-mock";
import { makeReview, makeCheck, makeAgentRun } from "./test/fixtures";

// Route the store's Tauri imports through the typed mock. The factory returns
// the singleton `invoke`/`listen` from the shared mock module so tests can
// register handlers and inspect calls.
vi.mock("@tauri-apps/api/core", async () => {
  const mock = await import("./test/tauri-mock");
  return { invoke: mock.invoke };
});
vi.mock("@tauri-apps/api/event", async () => {
  const mock = await import("./test/tauri-mock");
  return { listen: mock.listen };
});

// Import after the mocks are registered.
const { useAppStore } = await import("./store");

// Snapshot the pristine store so each test starts clean (Zustand's store is a
// module singleton).
const pristine = useAppStore.getState();

beforeEach(() => {
  useAppStore.setState(pristine, true);
});

describe("fetchReviews", () => {
  it("populates reviews and clears loading on success", async () => {
    const reviews: Review[] = [makeReview({ pr: "pr-a" })];
    mockInvoke("list_reviews", () => reviews);

    await useAppStore.getState().fetchReviews();

    const state = useAppStore.getState();
    expect(state.reviews).toEqual(reviews);
    expect(state.loading).toBe(false);
    expect(state.error).toBeNull();
  });

  it("sets error (non-fatal) and clears loading on rejection", async () => {
    mockInvokeReject("list_reviews", "boom");

    await useAppStore.getState().fetchReviews();

    const state = useAppStore.getState();
    expect(state.error).toContain("boom");
    expect(state.loading).toBe(false);
    expect(state.reviews).toEqual([]);
  });
});

describe("restackPr", () => {
  it("replaces the review across every list and clears stale on success", async () => {
    const stale = makeReview({ pr: "pr-1", stale: true });
    const restacked = makeReview({ pr: "pr-1", stale: false });
    useAppStore.setState({
      reviews: [stale],
      authoredPrs: [stale],
      reviewRequests: [],
      activeReview: stale,
    });
    mockInvoke("restack_pr", (args) => {
      expect(args.pr).toBe("pr-1");
      return restacked;
    });

    await useAppStore.getState().restackPr("pr-1");

    const state = useAppStore.getState();
    expect(state.reviews[0]?.stale).toBe(false);
    expect(state.authoredPrs[0]?.stale).toBe(false);
    expect(state.activeReview?.stale).toBe(false);
    expect(state.error).toBeNull();
  });

  it("keeps a conflict-resolver agent on the returned review", async () => {
    const stale = makeReview({ pr: "pr-1", stale: true, agent: null });
    const conflicted = makeReview({
      pr: "pr-1",
      stale: true,
      agent: makeAgentRun({ mode: "Restack" }),
    });
    useAppStore.setState({ reviews: [stale] });
    mockInvoke("restack_pr", () => conflicted);

    await useAppStore.getState().restackPr("pr-1");

    // Real narrowing on the nullable agent union, not a cast.
    const agent = useAppStore.getState().reviews[0]?.agent;
    expect(agent).not.toBeNull();
    if (agent == null) throw new Error("expected an active agent");
    expect(agent.mode).toBe("Restack");
  });

  it("sets error and does not throw when the backend rejects", async () => {
    const stale = makeReview({ pr: "pr-1", stale: true });
    useAppStore.setState({ reviews: [stale] });
    mockInvokeReject("restack_pr", "rebase exploded");

    await useAppStore.getState().restackPr("pr-1");

    const state = useAppStore.getState();
    expect(state.error).toContain("rebase exploded");
    // Non-fatal: the loop is not blocked, the stale review is untouched.
    expect(state.reviews[0]?.stale).toBe(true);
  });
});

describe("listCiChecks", () => {
  it("returns the checks from the backend", async () => {
    const checks: CiCheck[] = [makeCheck({ name: "lint" })];
    mockInvoke("list_ci_checks", () => checks);

    const result = await useAppStore.getState().listCiChecks("pr-1");

    expect(result).toEqual(checks);
    expect(callsFor("list_ci_checks")[0]?.args.pr).toBe("pr-1");
  });

  it("returns [] and never throws on gh error (Invariant 1)", async () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    mockInvokeReject("list_ci_checks", "gh not found");

    const result = await useAppStore.getState().listCiChecks("pr-1");

    expect(result).toEqual([]);
    // Non-fatal: a CI query must not set the blocking store error.
    expect(useAppStore.getState().error).toBeNull();
    spy.mockRestore();
  });
});

describe("fixCi", () => {
  it("updates activeReview + lists to the Dispatched review on success", async () => {
    const before = makeReview({ pr: "pr-1", gate_state: "InReview" });
    const dispatched = makeReview({
      pr: "pr-1",
      gate_state: "Dispatched",
      agent: makeAgentRun({ mode: "Fix" }),
    });
    useAppStore.setState({
      activeReview: before,
      reviews: [before],
    });
    mockInvoke("fix_ci", (args) => {
      expect(args.pr).toBe("pr-1");
      return dispatched;
    });

    await useAppStore.getState().fixCi("pr-1");

    const state = useAppStore.getState();
    expect(state.activeReview?.gate_state).toBe("Dispatched");
    expect(state.reviews[0]?.gate_state).toBe("Dispatched");
    expect(state.error).toBeNull();
  });

  it("sets error (non-fatal) when dispatch rejects", async () => {
    const before = makeReview({ pr: "pr-1", gate_state: "InReview" });
    useAppStore.setState({ activeReview: before, reviews: [before] });
    mockInvokeReject("fix_ci", "spawn failed");

    await useAppStore.getState().fixCi("pr-1");

    const state = useAppStore.getState();
    expect(state.error).toContain("spawn failed");
    expect(state.activeReview?.gate_state).toBe("InReview");
  });
});

describe("requestChanges", () => {
  it("no-ops when not on the diff view", async () => {
    useAppStore.setState({ view: { kind: "prs" } });
    mockInvokeReject("request_changes", "should not be called");

    await useAppStore.getState().requestChanges();

    expect(callsFor("request_changes")).toHaveLength(0);
    expect(useAppStore.getState().error).toBeNull();
  });

  it("transitions the active review when on the diff view", async () => {
    const before = makeReview({ pr: "pr-1", gate_state: "InReview" });
    const dispatched = makeReview({ pr: "pr-1", gate_state: "Dispatched" });
    useAppStore.setState({
      view: { kind: "diff", pr: "pr-1" },
      activeReview: before,
      reviews: [before],
    });
    mockInvoke("request_changes", () => dispatched);

    await useAppStore.getState().requestChanges();

    expect(useAppStore.getState().activeReview?.gate_state).toBe("Dispatched");
  });
});

describe("approveReview", () => {
  it("advances the review across lists + activeReview on success (D2)", async () => {
    const before = makeReview({ pr: "pr-1", gate_state: "InReview" });
    const approved = makeReview({ pr: "pr-1", gate_state: "Approved" });
    useAppStore.setState({
      activeReview: before,
      reviews: [before],
    });
    mockInvoke("approve_review", (args) => {
      expect(args.pr).toBe("pr-1");
      return approved;
    });

    await useAppStore.getState().approveReview("pr-1");

    const state = useAppStore.getState();
    expect(state.activeReview?.gate_state).toBe("Approved");
    expect(state.reviews[0]?.gate_state).toBe("Approved");
    expect(state.error).toBeNull();
  });

  it("sets error (non-fatal) on rejection", async () => {
    const before = makeReview({ pr: "pr-1", gate_state: "InReview" });
    useAppStore.setState({ activeReview: before, reviews: [before] });
    mockInvokeReject("approve_review", "cannot approve");

    await useAppStore.getState().approveReview("pr-1");

    expect(useAppStore.getState().error).toContain("cannot approve");
    expect(useAppStore.getState().activeReview?.gate_state).toBe("InReview");
  });
});

describe("mergeReview", () => {
  it("advances the active review to Merged on success (D2)", async () => {
    const before = makeReview({ pr: "pr-1", gate_state: "Approved" });
    const merged = makeReview({ pr: "pr-1", gate_state: "Merged" });
    useAppStore.setState({
      activeReview: before,
      reviews: [before],
    });
    mockInvoke("merge_review", (args) => {
      expect(args.pr).toBe("pr-1");
      return merged;
    });

    await useAppStore.getState().mergeReview("pr-1");

    const state = useAppStore.getState();
    expect(state.activeReview?.gate_state).toBe("Merged");
    expect(state.reviews[0]?.gate_state).toBe("Merged");
    expect(state.error).toBeNull();
  });

  it("sets error (non-fatal) on rejection", async () => {
    const before = makeReview({ pr: "pr-1", gate_state: "Approved" });
    useAppStore.setState({ activeReview: before, reviews: [before] });
    mockInvokeReject("merge_review", "merge conflict");

    await useAppStore.getState().mergeReview("pr-1");

    expect(useAppStore.getState().error).toContain("merge conflict");
    expect(useAppStore.getState().activeReview?.gate_state).toBe("Approved");
  });
});

describe("submitGithubReview", () => {
  it("returns the result and refreshes the active review on success (D9)", async () => {
    const before = makeReview({ pr: "pr-1", gate_state: "InReview" });
    const refreshed = makeReview({ pr: "pr-1", gate_state: "Approved" });
    useAppStore.setState({
      view: { kind: "diff", pr: "pr-1" },
      activeReview: before,
      reviews: [before],
    });
    const result: SubmitReviewResult = { submitted: 2, skipped: [] };
    mockInvoke("submit_github_review", (args) => {
      expect(args.pr).toBe("pr-1");
      expect(args.event).toBe("Approve");
      return result;
    });
    mockInvoke("get_review", () => refreshed);
    mockInvoke("get_review_diff", () => ({ raw: "" }));

    const returned = await useAppStore
      .getState()
      .submitGithubReview("pr-1", "Approve", "");

    expect(returned).toEqual(result);
    expect(useAppStore.getState().activeReview?.gate_state).toBe("Approved");
    expect(useAppStore.getState().error).toBeNull();
  });

  it("maps an empty body to null and passes a non-empty body through", async () => {
    useAppStore.setState({
      view: { kind: "diff", pr: "pr-1" },
      activeReview: makeReview({ pr: "pr-1" }),
      reviews: [],
    });
    const result: SubmitReviewResult = { submitted: 0, skipped: [] };
    let seenBody: unknown = "unset";
    mockInvoke("submit_github_review", (args) => {
      seenBody = args.body;
      return result;
    });
    mockInvoke("get_review", () => makeReview({ pr: "pr-1" }));
    mockInvoke("get_review_diff", () => ({ raw: "" }));

    await useAppStore.getState().submitGithubReview("pr-1", "Comment", "   ");
    expect(seenBody).toBeNull();

    await useAppStore.getState().submitGithubReview("pr-1", "Comment", "looks good");
    expect(seenBody).toBe("looks good");
  });

  it("sets a store error listing reasons when comments are skipped", async () => {
    const before = makeReview({ pr: "pr-1", gate_state: "InReview" });
    useAppStore.setState({
      view: { kind: "diff", pr: "pr-1" },
      activeReview: before,
      reviews: [before],
    });
    const result: SubmitReviewResult = {
      submitted: 1,
      skipped: [["c-1", "line not in diff"]],
    };
    mockInvoke("submit_github_review", () => result);
    mockInvoke("get_review", () => before);
    mockInvoke("get_review_diff", () => ({ raw: "" }));

    const returned = await useAppStore
      .getState()
      .submitGithubReview("pr-1", "Comment", "hi");

    expect(returned).toEqual(result);
    expect(useAppStore.getState().error).toContain("line not in diff");
  });

  it("returns null and sets error on rejection", async () => {
    useAppStore.setState({ view: { kind: "diff", pr: "pr-1" } });
    mockInvokeReject("submit_github_review", "gh failed");

    const returned = await useAppStore
      .getState()
      .submitGithubReview("pr-1", "Comment", "");

    expect(returned).toBeNull();
    expect(useAppStore.getState().error).toContain("gh failed");
  });
});

describe("killAgent", () => {
  it("applies the reconciled review across lists + activeReview on success (D12)", async () => {
    const running = makeReview({
      pr: "pr-1",
      gate_state: "Dispatched",
      agent: makeAgentRun({ mode: "Fix" }),
    });
    const settled = makeReview({
      pr: "pr-1",
      gate_state: "InReview",
      agent: null,
    });
    useAppStore.setState({
      activeReview: running,
      reviews: [running],
      authoredPrs: [running],
    });
    mockInvoke("kill_agent", (args) => {
      expect(args.pr).toBe("pr-1");
      return settled;
    });

    await useAppStore.getState().killAgent("pr-1");

    const state = useAppStore.getState();
    expect(state.activeReview?.gate_state).toBe("InReview");
    expect(state.activeReview?.agent).toBeNull();
    expect(state.reviews[0]?.agent).toBeNull();
    expect(state.authoredPrs[0]?.agent).toBeNull();
    expect(state.error).toBeNull();
  });

  it("sets error (non-fatal) and leaves the review untouched on rejection", async () => {
    const running = makeReview({
      pr: "pr-1",
      gate_state: "Dispatched",
      agent: makeAgentRun({ mode: "Fix" }),
    });
    useAppStore.setState({ activeReview: running, reviews: [running] });
    mockInvokeReject("kill_agent", "no such process");

    await useAppStore.getState().killAgent("pr-1");

    const state = useAppStore.getState();
    expect(state.error).toContain("no such process");
    // Non-fatal: the agent handle is untouched so the loop is not blocked.
    expect(state.activeReview?.agent).not.toBeNull();
  });
});

describe("ensureReviewWorktree", () => {
  it("returns the materialized worktree path on success (D12)", async () => {
    mockInvoke("ensure_review_worktree", (args) => {
      expect(args.pr).toBe("pr-1");
      return "/tmp/wt/pr-1";
    });

    const path = await useAppStore.getState().ensureReviewWorktree("pr-1");

    expect(path).toBe("/tmp/wt/pr-1");
    expect(useAppStore.getState().error).toBeNull();
  });

  it("returns null and sets error on failure", async () => {
    mockInvokeReject("ensure_review_worktree", "checkout failed");

    const path = await useAppStore.getState().ensureReviewWorktree("pr-1");

    expect(path).toBeNull();
    expect(useAppStore.getState().error).toContain("checkout failed");
  });
});

describe("fetchInterdiff", () => {
  it("returns the interdiff on success (D10)", async () => {
    const diff = { raw: "diff --git a/x b/x" };
    mockInvoke("get_interdiff", (args) => {
      expect(args.pr).toBe("pr-1");
      return diff;
    });

    const result = await useAppStore.getState().fetchInterdiff("pr-1");

    expect(result).toEqual(diff);
    expect(useAppStore.getState().error).toBeNull();
  });

  it("returns null and sets error on failure", async () => {
    mockInvokeReject("get_interdiff", "no snapshot");

    const result = await useAppStore.getState().fetchInterdiff("pr-1");

    expect(result).toBeNull();
    expect(useAppStore.getState().error).toContain("no snapshot");
  });
});

/** Minimal evidence bundle for the store-action tests. */
function makeEvidence(): EvidenceSummary {
  return {
    signals: {
      additions: 1,
      deletions: 0,
      files_changed: 1,
      size_class: "S",
      test_delta: {
        test_files_changed: 0,
        assertions_added: 0,
        assertions_removed: 0,
      },
      risk_paths: [],
      weakening: [],
    },
    ci: null,
    agent_ran: [],
  };
}

describe("fetchEvidence", () => {
  it("returns the evidence bundle on success (B1)", async () => {
    const evidence = makeEvidence();
    mockInvoke("get_evidence", (args) => {
      expect(args.pr).toBe("pr-1");
      return evidence;
    });

    const result = await useAppStore.getState().fetchEvidence("pr-1");

    expect(result).toEqual(evidence);
    expect(useAppStore.getState().error).toBeNull();
  });

  it("returns null and never sets the blocking error on failure (Invariant 1)", async () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    mockInvokeReject("get_evidence", "boom");

    const result = await useAppStore.getState().fetchEvidence("pr-1");

    expect(result).toBeNull();
    expect(useAppStore.getState().error).toBeNull();
    spy.mockRestore();
  });
});

describe("preReview", () => {
  it("applies the returned review with its running Review agent across lists (B2)", async () => {
    const before = makeReview({ pr: "pr-1", gate_state: "InReview", agent: null });
    const running = makeReview({
      pr: "pr-1",
      gate_state: "InReview",
      agent: makeAgentRun({ mode: "Review" }),
    });
    useAppStore.setState({ activeReview: before, reviews: [before] });
    mockInvoke("pre_review", (args) => {
      expect(args.pr).toBe("pr-1");
      return running;
    });

    await useAppStore.getState().preReview("pr-1");

    const agent = useAppStore.getState().activeReview?.agent;
    expect(agent).not.toBeNull();
    if (agent == null) throw new Error("expected a running review agent");
    expect(agent.mode).toBe("Review");
    expect(useAppStore.getState().reviews[0]?.agent).not.toBeNull();
    expect(useAppStore.getState().error).toBeNull();
  });

  it("sets error (non-fatal) when the pre-pass refuses to start", async () => {
    const before = makeReview({ pr: "pr-1", gate_state: "InReview" });
    useAppStore.setState({ activeReview: before, reviews: [before] });
    mockInvokeReject("pre_review", "already has a running agent");

    await useAppStore.getState().preReview("pr-1");

    expect(useAppStore.getState().error).toContain("already has a running agent");
  });
});

describe("fetchFilePair", () => {
  it("returns the pair and memoizes by pr:path:head (B4)", async () => {
    useAppStore.getState().filePairCache.clear();
    useAppStore.setState({
      activeReview: makeReview({ pr: "pr-1", head_sha: "h1" }),
    });
    const pair: FilePair = { original: "a", modified: "b", full: true };
    let calls = 0;
    mockInvoke("get_file_pair", (args) => {
      calls += 1;
      expect(args.pr).toBe("pr-1");
      expect(args.path).toBe("src/x.ts");
      return pair;
    });

    const first = await useAppStore.getState().fetchFilePair("pr-1", "src/x.ts");
    const second = await useAppStore
      .getState()
      .fetchFilePair("pr-1", "src/x.ts");

    expect(first).toEqual(pair);
    expect(second).toEqual(pair);
    // Second call is served from the cache, so the backend is hit only once.
    expect(calls).toBe(1);
  });

  it("returns null and never sets the blocking error on failure (Invariant 1)", async () => {
    useAppStore.getState().filePairCache.clear();
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    useAppStore.setState({
      activeReview: makeReview({ pr: "pr-2", head_sha: "h2" }),
    });
    mockInvokeReject("get_file_pair", "no such file");

    const result = await useAppStore
      .getState()
      .fetchFilePair("pr-2", "missing.ts");

    expect(result).toBeNull();
    expect(useAppStore.getState().error).toBeNull();
    spy.mockRestore();
  });
});
