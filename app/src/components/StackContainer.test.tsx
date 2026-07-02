import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { StackContainer } from "./StackContainer";
import { buildStackTrees, flattenStack } from "../lib/stack-tree";
import type { StackTreeNode } from "../lib/stack-tree";
import { makeReview } from "../test/fixtures";

/** Build the (single) stack root from a set of reviews for the render tests. */
function stackRoot(): StackTreeNode {
  const r1 = makeReview({
    id: "r1",
    branch: "parent-branch",
    gate_state: "Approved",
    children: ["r2"],
  });
  const r2 = makeReview({
    id: "r2",
    branch: "child-branch",
    gate_state: "InReview",
    parents: ["r1"],
  });
  const root = buildStackTrees([r1, r2])[0];
  if (root === undefined) throw new Error("expected a stack root");
  return root;
}

describe("StackContainer", () => {
  it("renders a header with the member count and a health summary", () => {
    const root = stackRoot();
    render(
      <StackContainer
        root={root}
        nodes={flattenStack(root)}
        density="cards"
        onAction={vi.fn()}
        onRestack={vi.fn()}
      />,
    );

    expect(
      screen.getByText(
        (_, el) => el?.textContent === "STACK · 2 PRs",
      ),
    ).toBeInTheDocument();
    // One approved, one in review -> progress summary.
    expect(screen.getByText("1/2 approved")).toBeInTheDocument();
  });

  it("renders every member, parent before child", () => {
    const root = stackRoot();
    render(
      <StackContainer
        root={root}
        nodes={flattenStack(root)}
        density="cards"
        onAction={vi.fn()}
        onRestack={vi.fn()}
      />,
    );

    const parent = screen.getByText("parent-branch");
    const child = screen.getByText("child-branch");
    expect(parent).toBeInTheDocument();
    expect(child).toBeInTheDocument();
    // Parent-first: the parent node precedes the child in document order.
    expect(
      parent.compareDocumentPosition(child) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });
});
