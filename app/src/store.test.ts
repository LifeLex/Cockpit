import { describe, it, expect, beforeEach, vi } from "vitest";
import type { Review } from "./bindings/Review";
import type { CiCheck } from "./bindings/CiCheck";
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
      frontier: [stale],
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
    expect(state.frontier[0]?.stale).toBe(false);
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
      frontier: [before],
    });
    mockInvoke("fix_ci", (args) => {
      expect(args.pr).toBe("pr-1");
      return dispatched;
    });

    await useAppStore.getState().fixCi("pr-1");

    const state = useAppStore.getState();
    expect(state.activeReview?.gate_state).toBe("Dispatched");
    expect(state.reviews[0]?.gate_state).toBe("Dispatched");
    expect(state.frontier[0]?.gate_state).toBe("Dispatched");
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
