import { describe, it, expect } from "vitest";
import { sortByAttention, attentionRank } from "./attention";
import { makeReview, ALL_GATE_STATES } from "../test/fixtures";
import type { Review } from "../bindings/Review";

/** Read the gate-state order out of a sorted list, for readable assertions. */
function states(reviews: readonly Review[]): readonly string[] {
  return reviews.map((r) => r.gate_state);
}

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
