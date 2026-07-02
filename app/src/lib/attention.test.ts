import { describe, it, expect } from "vitest";
import {
  sortByAttention,
  attentionRank,
  attentionReasons,
  isFastLane,
} from "./attention";
import { makeReview, ALL_GATE_STATES } from "../test/fixtures";
import type { Review } from "../bindings/Review";
import type { CiSummary } from "../bindings/CiSummary";

/** Read the gate-state order out of a sorted list, for readable assertions. */
function states(reviews: readonly Review[]): readonly string[] {
  return reviews.map((r) => r.gate_state);
}

/** A diff adding `n` lines to `path`. Size = n changed lines. */
function addLines(path: string, n: number): string {
  let s = `diff --git a/${path} b/${path}\n--- a/${path}\n+++ b/${path}\n@@ -0,0 +1,${String(n)} @@\n`;
  for (let i = 0; i < n; i++) {
    s += `+row ${String(i)}\n`;
  }
  return s;
}

const SMALL = addLines("data.txt", 10); // S, non-sensitive
const LARGE = addLines("data.txt", 500); // L
const XL = addLines("data.txt", 700); // Xl
const SENSITIVE = addLines("migrations/001_init.sql", 5); // S, Migration flag
const XL_SENSITIVE = addLines("migrations/big.sql", 700); // Xl + Migration

const CI_GREEN: CiSummary = { passed: 3, total: 3, failed: 0, pending: 0 };
const CI_FAIL: CiSummary = { passed: 1, total: 2, failed: 1, pending: 0 };
const CI_PENDING: CiSummary = { passed: 1, total: 2, failed: 0, pending: 1 };

/** Gate states in attention (rank) order, most-urgent first. */
const RANK_ORDER = [
  "Reworked",
  "InReview",
  "Pending",
  "Dispatched",
  "Approved",
  "Merged",
] as const;

describe("sortByAttention", () => {
  it("orders the reviewer's active queue ahead of settled work", () => {
    const reviews = ALL_GATE_STATES.map((state, i) =>
      makeReview({ id: `rev-${String(i)}`, gate_state: state }),
    );
    const sorted = sortByAttention(reviews);
    expect(states(sorted)).toEqual([
      "Reworked",
      "InReview",
      "Pending",
      "Dispatched",
      "Approved",
      "Merged",
    ]);
  });

  it("sinks stale reviews below every non-stale review", () => {
    const staleInReview = makeReview({
      id: "a",
      gate_state: "InReview",
      stale: true,
    });
    const approvedFresh = makeReview({
      id: "b",
      gate_state: "Approved",
      stale: false,
    });
    const sorted = sortByAttention([staleInReview, approvedFresh]);
    expect(sorted.map((r) => r.id)).toEqual(["b", "a"]);
  });

  it("preserves gate-state order among stale reviews", () => {
    const staleApproved = makeReview({
      id: "a",
      gate_state: "Approved",
      stale: true,
    });
    const staleReworked = makeReview({
      id: "b",
      gate_state: "Reworked",
      stale: true,
    });
    const sorted = sortByAttention([staleApproved, staleReworked]);
    expect(sorted.map((r) => r.id)).toEqual(["b", "a"]);
  });

  it("breaks rank ties deterministically by id", () => {
    const first = makeReview({ id: "aaa", gate_state: "InReview" });
    const second = makeReview({ id: "bbb", gate_state: "InReview" });
    expect(sortByAttention([second, first]).map((r) => r.id)).toEqual([
      "aaa",
      "bbb",
    ]);
  });

  it("does not mutate the input array", () => {
    const input = [
      makeReview({ id: "a", gate_state: "Approved" }),
      makeReview({ id: "b", gate_state: "Reworked" }),
    ];
    const before = input.map((r) => r.id);
    sortByAttention(input);
    expect(input.map((r) => r.id)).toEqual(before);
  });

  it("ranks a stale review strictly higher (larger) than any fresh review", () => {
    const fresh = makeReview({ gate_state: "Approved", stale: false });
    const stale = makeReview({ gate_state: "Reworked", stale: true });
    expect(attentionRank(stale)).toBeGreaterThan(attentionRank(fresh));
  });
});

describe("attentionRank — within-bucket adjustments never cross buckets", () => {
  it("keeps every higher bucket ahead of the next even at worst-case signals", () => {
    // A maximally-sinking review (CI failing + XL + sensitive) in the higher
    // bucket must still outrank a maximally-rising review (CI green + small) in
    // the adjacent lower bucket, for every adjacent pair in rank order.
    RANK_ORDER.forEach((state, i) => {
      const next = RANK_ORDER[i + 1];
      if (next === undefined) return;
      const higher = makeReview({
        gate_state: state,
        ci_summary: CI_FAIL,
        diff: { raw: XL_SENSITIVE },
      });
      const lower = makeReview({
        gate_state: next,
        ci_summary: CI_GREEN,
        diff: { raw: SMALL },
      });
      expect(attentionRank(higher)).toBeLessThan(attentionRank(lower));
    });
  });

  it("within a bucket, CI-green sorts ahead of CI-failing", () => {
    const green = makeReview({ id: "g", gate_state: "Pending", ci_summary: CI_GREEN, diff: { raw: SMALL } });
    const red = makeReview({ id: "r", gate_state: "Pending", ci_summary: CI_FAIL, diff: { raw: SMALL } });
    expect(attentionRank(green)).toBeLessThan(attentionRank(red));
  });

  it("within a bucket, a small diff sorts ahead of an XL diff", () => {
    const small = makeReview({ id: "s", gate_state: "InReview", diff: { raw: SMALL } });
    const xl = makeReview({ id: "x", gate_state: "InReview", diff: { raw: XL } });
    expect(attentionRank(small)).toBeLessThan(attentionRank(xl));
  });

  it("within a bucket, a sensitive path sinks a review", () => {
    const plain = makeReview({ id: "p", gate_state: "InReview", diff: { raw: SMALL } });
    const sensitive = makeReview({ id: "z", gate_state: "InReview", diff: { raw: SENSITIVE } });
    expect(attentionRank(sensitive)).toBeGreaterThan(attentionRank(plain));
  });
});

describe("attentionReasons", () => {
  it("is empty for a clean, small, green review", () => {
    const clean = makeReview({ ci_summary: CI_GREEN, diff: { raw: SMALL } });
    expect(attentionReasons(clean)).toEqual([]);
  });

  it("reports CI failing", () => {
    const red = makeReview({ ci_summary: CI_FAIL, diff: { raw: SMALL } });
    expect(attentionReasons(red)).toContain("CI failing");
  });

  it("does not report CI pending (a mild rank signal, not a headline)", () => {
    const pending = makeReview({ ci_summary: CI_PENDING, diff: { raw: SMALL } });
    expect(attentionReasons(pending)).toEqual([]);
  });

  it("distinguishes large from very large diffs", () => {
    expect(attentionReasons(makeReview({ diff: { raw: LARGE } }))).toContain("Large diff");
    expect(attentionReasons(makeReview({ diff: { raw: XL } }))).toContain("Very large diff");
  });

  it("names the sensitive path touched", () => {
    const sensitive = makeReview({ diff: { raw: SENSITIVE } });
    expect(attentionReasons(sensitive)).toContain("Touches migrations");
  });

  it("orders reasons: CI failing, then size, then sensitive paths", () => {
    const review = makeReview({ ci_summary: CI_FAIL, diff: { raw: XL_SENSITIVE } });
    expect(attentionReasons(review)).toEqual([
      "CI failing",
      "Very large diff",
      "Touches migrations",
    ]);
  });
});

describe("isFastLane", () => {
  it("includes an actionable, small, green, low-risk review", () => {
    const review = makeReview({ gate_state: "Pending", ci_summary: CI_GREEN, diff: { raw: SMALL } });
    expect(isFastLane(review)).toBe(true);
  });

  it("accepts every actionable gate state", () => {
    for (const state of ["Pending", "InReview", "Reworked"] as const) {
      const review = makeReview({ gate_state: state, ci_summary: CI_GREEN, diff: { raw: SMALL } });
      expect(isFastLane(review)).toBe(true);
    }
  });

  it("excludes settled and agent-owned states", () => {
    for (const state of ["Dispatched", "Approved", "Merged"] as const) {
      const review = makeReview({ gate_state: state, ci_summary: CI_GREEN, diff: { raw: SMALL } });
      expect(isFastLane(review)).toBe(false);
    }
  });

  it("excludes a stale review", () => {
    const review = makeReview({ gate_state: "InReview", stale: true, ci_summary: CI_GREEN, diff: { raw: SMALL } });
    expect(isFastLane(review)).toBe(false);
  });

  it("excludes a large diff (>= 200 changed lines)", () => {
    const review = makeReview({ gate_state: "Pending", ci_summary: CI_GREEN, diff: { raw: LARGE } });
    expect(isFastLane(review)).toBe(false);
  });

  it("excludes a failing or pending CI", () => {
    const red = makeReview({ gate_state: "Pending", ci_summary: CI_FAIL, diff: { raw: SMALL } });
    const pending = makeReview({ gate_state: "Pending", ci_summary: CI_PENDING, diff: { raw: SMALL } });
    expect(isFastLane(red)).toBe(false);
    expect(isFastLane(pending)).toBe(false);
  });

  it("excludes a review with no loaded CI (needs positive green evidence)", () => {
    const review = makeReview({ gate_state: "Pending", diff: { raw: SMALL } });
    expect(isFastLane(review)).toBe(false);
  });

  it("excludes a review touching a sensitive path", () => {
    const review = makeReview({ gate_state: "Pending", ci_summary: CI_GREEN, diff: { raw: SENSITIVE } });
    expect(isFastLane(review)).toBe(false);
  });

  it("excludes a parented (non-root) review to keep stacks intact", () => {
    // A child that is otherwise fast-lane-eligible must stay in its stack group
    // so it never surfaces ahead of its unreviewed parent (frontier-root-first).
    const child = makeReview({
      gate_state: "Pending",
      ci_summary: CI_GREEN,
      diff: { raw: SMALL },
      parents: ["rev-parent"],
    });
    expect(isFastLane(child)).toBe(false);
    // The same review with no parents (a stack root) does qualify.
    const root = makeReview({
      gate_state: "Pending",
      ci_summary: CI_GREEN,
      diff: { raw: SMALL },
      parents: [],
    });
    expect(isFastLane(root)).toBe(true);
  });
});
