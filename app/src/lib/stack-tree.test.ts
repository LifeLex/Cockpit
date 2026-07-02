import { describe, it, expect } from "vitest";
import { makeReview } from "../test/fixtures";
import {
  buildStackTrees,
  flattenStack,
  computeHealth,
  stackStatus,
  frontierReviewId,
  buildBoardItems,
} from "./stack-tree";
import type { StackHealth, StackTreeNode } from "./stack-tree";

/** The single root of a set, failing loudly if there is not exactly one. */
function onlyRoot(nodes: readonly StackTreeNode[]): StackTreeNode {
  expect(nodes).toHaveLength(1);
  const root = nodes[0];
  if (root === undefined) throw new Error("expected a root");
  return root;
}

describe("buildStackTrees", () => {
  it("builds one tree for a linear parent->child->grandchild stack", () => {
    const r1 = makeReview({ id: "r1", children: ["r2"] });
    const r2 = makeReview({ id: "r2", parents: ["r1"], children: ["r3"] });
    const r3 = makeReview({ id: "r3", parents: ["r2"] });

    const root = onlyRoot(buildStackTrees([r1, r2, r3]));

    expect(root.review.id).toBe("r1");
    expect(root.childNodes.map((c) => c.review.id)).toEqual(["r2"]);
    expect(root.childNodes[0]?.childNodes.map((c) => c.review.id)).toEqual([
      "r3",
    ]);
  });

  it("does not re-emit a child that is reachable from its parent as a root", () => {
    const r1 = makeReview({ id: "r1", children: ["r2"] });
    const r2 = makeReview({ id: "r2", parents: ["r1"] });

    const trees = buildStackTrees([r1, r2]);

    expect(trees.map((t) => t.review.id)).toEqual(["r1"]);
  });

  it("treats each parentless review as its own root", () => {
    const a = makeReview({ id: "a" });
    const b = makeReview({ id: "b" });

    const trees = buildStackTrees([a, b]);

    expect(trees.map((t) => t.review.id).sort()).toEqual(["a", "b"]);
  });

  it("degrades a review whose parent is outside the set to a root (cross-set parent)", () => {
    // r2's parent r1 is NOT in the passed set (a cross-project / unfetched
    // parent). It must degrade to a root rather than vanish from the board.
    const r2 = makeReview({ id: "r2", parents: ["r1"] });

    const trees = buildStackTrees([r2]);

    expect(trees.map((t) => t.review.id)).toEqual(["r2"]);
  });
});

describe("flattenStack", () => {
  it("emits members parent-first (pre-order) with increasing depth", () => {
    const r1 = makeReview({ id: "r1", children: ["r2"] });
    const r2 = makeReview({ id: "r2", parents: ["r1"], children: ["r3"] });
    const r3 = makeReview({ id: "r3", parents: ["r2"] });
    const root = onlyRoot(buildStackTrees([r1, r2, r3]));

    const flat = flattenStack(root);

    expect(flat.map((n) => n.review.id)).toEqual(["r1", "r2", "r3"]);
    expect(flat.map((n) => n.depth)).toEqual([0, 1, 2]);
  });

  it("visits a diamond member only once", () => {
    // r4 is reachable via both r2 and r3.
    const r1 = makeReview({ id: "r1", children: ["r2", "r3"] });
    const r2 = makeReview({ id: "r2", parents: ["r1"], children: ["r4"] });
    const r3 = makeReview({ id: "r3", parents: ["r1"], children: ["r4"] });
    const r4 = makeReview({ id: "r4", parents: ["r2", "r3"] });
    const root = onlyRoot(buildStackTrees([r1, r2, r3, r4]));

    const ids = flattenStack(root).map((n) => n.review.id);

    expect(ids.filter((id) => id === "r4")).toHaveLength(1);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe("computeHealth", () => {
  it("counts total, approved, merged, and stale across the (deduped) stack", () => {
    const r1 = makeReview({ id: "r1", gate_state: "Merged", children: ["r2"] });
    const r2 = makeReview({
      id: "r2",
      gate_state: "Approved",
      parents: ["r1"],
      children: ["r3"],
    });
    const r3 = makeReview({
      id: "r3",
      gate_state: "InReview",
      parents: ["r2"],
      stale: true,
    });
    const root = onlyRoot(buildStackTrees([r1, r2, r3]));

    expect(computeHealth(root)).toEqual({
      total: 3,
      approved: 1,
      merged: 1,
      stale: 1,
    });
  });
});

describe("stackStatus", () => {
  it("reports a stale member with a warning tone", () => {
    const health: StackHealth = { total: 3, approved: 1, merged: 0, stale: 2 };
    expect(stackStatus(health)).toEqual({ tone: "warning", label: "2 stale" });
  });

  it("singularizes a single stale member", () => {
    const health: StackHealth = { total: 3, approved: 0, merged: 0, stale: 1 };
    expect(stackStatus(health).label).toBe("1 stale");
  });

  it("reads as all approved when every member is approved or merged", () => {
    const health: StackHealth = { total: 3, approved: 2, merged: 1, stale: 0 };
    expect(stackStatus(health)).toEqual({
      tone: "approved",
      label: "All approved",
    });
  });

  it("reads as all merged when the whole stack is merged", () => {
    const health: StackHealth = { total: 2, approved: 0, merged: 2, stale: 0 };
    expect(stackStatus(health).label).toBe("All merged");
  });

  it("reports the ready count while work remains", () => {
    const health: StackHealth = { total: 3, approved: 1, merged: 0, stale: 0 };
    expect(stackStatus(health)).toEqual({
      tone: "progress",
      label: "1/3 approved",
    });
  });
});

describe("frontierReviewId", () => {
  it("selects the most-urgent actionable member (approved parent, in-review child)", () => {
    const r1 = makeReview({
      id: "r1",
      gate_state: "Approved",
      children: ["r2"],
    });
    const r2 = makeReview({
      id: "r2",
      gate_state: "InReview",
      parents: ["r1"],
    });
    const root = onlyRoot(buildStackTrees([r1, r2]));

    expect(frontierReviewId(root)).toBe("r2");
  });

  it("skips a stale member in favor of an actionable ancestor", () => {
    const r1 = makeReview({
      id: "r1",
      gate_state: "InReview",
      children: ["r2"],
    });
    const r2 = makeReview({
      id: "r2",
      gate_state: "InReview",
      parents: ["r1"],
      stale: true,
    });
    const root = onlyRoot(buildStackTrees([r1, r2]));

    expect(frontierReviewId(root)).toBe("r1");
  });

  it("returns null when every member is settled", () => {
    const r1 = makeReview({
      id: "r1",
      gate_state: "Approved",
      children: ["r2"],
    });
    const r2 = makeReview({ id: "r2", gate_state: "Merged", parents: ["r1"] });
    const root = onlyRoot(buildStackTrees([r1, r2]));

    expect(frontierReviewId(root)).toBeNull();
  });
});

describe("buildBoardItems", () => {
  it("classifies lone reviews as singles and multi-node trees as stacks", () => {
    const single = makeReview({ id: "s1", gate_state: "InReview" });
    const p = makeReview({ id: "p", gate_state: "Approved", children: ["c"] });
    const c = makeReview({ id: "c", gate_state: "Approved", parents: ["p"] });

    const items = buildBoardItems([single, p, c]);

    const kinds = items.map((i) => i.kind).sort();
    expect(kinds).toEqual(["single", "stack"]);
    const stack = items.find((i) => i.kind === "stack");
    if (stack?.kind !== "stack") throw new Error("expected a stack item");
    expect(stack.nodes.map((n) => n.review.id)).toEqual(["p", "c"]);
  });

  it("interleaves a stack among singles by its most-urgent member", () => {
    // Stack's most-urgent member is Reworked (rank 0) — it must sort ahead of a
    // lone InReview (rank 1) and behind nothing.
    const lone = makeReview({ id: "lone", gate_state: "InReview" });
    const p = makeReview({ id: "p", gate_state: "Approved", children: ["c"] });
    const c = makeReview({ id: "c", gate_state: "Reworked", parents: ["p"] });

    const items = buildBoardItems([lone, p, c]);

    expect(items[0]?.kind).toBe("stack");
    expect(items[1]?.kind).toBe("single");
  });

  it("never drops a review, even in a malformed cycle", () => {
    // A pure cycle has no root; both members must still surface as singles.
    const r1 = makeReview({ id: "r1", parents: ["r2"], children: ["r2"] });
    const r2 = makeReview({ id: "r2", parents: ["r1"], children: ["r1"] });

    const items = buildBoardItems([r1, r2]);

    const ids = items
      .filter((i) => i.kind === "single")
      .map((i) => (i.kind === "single" ? i.review.id : ""));
    expect(ids.sort()).toEqual(["r1", "r2"]);
  });

  it("keeps a cross-set-parent review on the board as a lone card", () => {
    const orphan = makeReview({ id: "orphan", parents: ["missing"] });

    const items = buildBoardItems([orphan]);

    expect(items).toHaveLength(1);
    expect(items[0]?.kind).toBe("single");
  });
});
