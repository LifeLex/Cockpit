import { useMemo } from "react";
import type { Review } from "../bindings/Review";
import type { ReviewId } from "../bindings/ReviewId";
import type { GateState } from "../bindings/GateState";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** A node in the stack tree, wrapping a review with its resolved children. */
interface StackTreeNode {
  readonly review: Review;
  readonly childNodes: readonly StackTreeNode[];
}

/** Summary statistics for a single stack rooted at a given review. */
interface StackHealth {
  readonly total: number;
  readonly approved: number;
  readonly stale: number;
}

interface StackViewProps {
  readonly reviews: readonly Review[];
  readonly onViewDiff: (pr: string) => void;
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** Map from GateState to a display-friendly label. */
function gateStateLabel(state: GateState): string {
  switch (state) {
    case "Pending":
      return "Pending";
    case "InReview":
      return "In Review";
    case "Dispatched":
      return "Dispatched";
    case "Reworked":
      return "Reworked";
    case "Approved":
      return "Approved";
    default:
      return assertNever(state);
  }
}

/** Map from GateState to a color used for badges and tree connectors. */
function gateStateColor(state: GateState): string {
  switch (state) {
    case "Pending":
      return "#888";
    case "InReview":
      return "#2196F3";
    case "Dispatched":
      return "#FF9800";
    case "Reworked":
      return "#E040FB";
    case "Approved":
      return "#4CAF50";
    default:
      return assertNever(state);
  }
}

/**
 * Build stack trees from the flat review list.
 *
 * Roots are reviews with no parents. Each root's subtree is built by
 * recursively resolving children from the lookup map.
 */
function buildStackTrees(reviews: readonly Review[]): readonly StackTreeNode[] {
  const byId = new Map<ReviewId, Review>();
  for (const r of reviews) {
    byId.set(r.id, r);
  }

  // Track visited nodes to avoid cycles in malformed data.
  function buildSubtree(
    reviewId: ReviewId,
    visited: ReadonlySet<ReviewId>,
  ): StackTreeNode | undefined {
    if (visited.has(reviewId)) {
      return undefined;
    }
    const review = byId.get(reviewId);
    if (review === undefined) {
      return undefined;
    }
    const nextVisited = new Set(visited);
    nextVisited.add(reviewId);

    const childNodes: StackTreeNode[] = [];
    for (const childId of review.children) {
      const child = buildSubtree(childId, nextVisited);
      if (child !== undefined) {
        childNodes.push(child);
      }
    }
    return { review, childNodes };
  }

  const roots: StackTreeNode[] = [];
  for (const review of reviews) {
    if (review.parents.length === 0) {
      const tree = buildSubtree(review.id, new Set<ReviewId>());
      if (tree !== undefined) {
        roots.push(tree);
      }
    }
  }
  return roots;
}

/** Compute health stats by walking the tree recursively. */
function computeHealth(node: StackTreeNode): StackHealth {
  let total = 1;
  let approved = node.review.gate_state === "Approved" ? 1 : 0;
  let stale = node.review.stale ? 1 : 0;

  for (const child of node.childNodes) {
    const childHealth = computeHealth(child);
    total += childHealth.total;
    approved += childHealth.approved;
    stale += childHealth.stale;
  }

  return { total, approved, stale };
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/** Renders a single node in the tree and recurses into children. */
function StackNode({
  node,
  depth,
  onViewDiff,
}: {
  readonly node: StackTreeNode;
  readonly depth: number;
  readonly onViewDiff: (pr: string) => void;
}) {
  const review = node.review;
  const color = gateStateColor(review.gate_state);

  return (
    <div style={{ marginLeft: depth * 28 }}>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "6px 10px",
          margin: "2px 0",
          borderLeft: `3px solid ${color}`,
          borderRadius: "0 4px 4px 0",
          backgroundColor: "#1a1a1a",
          cursor: "pointer",
        }}
        onClick={() => {
          onViewDiff(review.pr);
        }}
        role="button"
        tabIndex={0}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onViewDiff(review.pr);
          }
        }}
      >
        {/* Connector line indicator for children */}
        {depth > 0 && (
          <span
            style={{
              display: "inline-block",
              width: 12,
              height: 2,
              backgroundColor: "#555",
              flexShrink: 0,
            }}
          />
        )}

        {/* PR reference */}
        <span style={{ fontWeight: "bold", fontSize: 13 }}>{review.pr}</span>

        {/* Gate state badge */}
        <span
          style={{
            padding: "1px 6px",
            borderRadius: 3,
            backgroundColor: color,
            color: "white",
            fontSize: 11,
            fontWeight: "bold",
            flexShrink: 0,
          }}
        >
          {gateStateLabel(review.gate_state)}
        </span>

        {/* Stale indicator */}
        {review.stale && (
          <span
            style={{
              color: "#FF5722",
              fontSize: 11,
              fontWeight: "bold",
              flexShrink: 0,
            }}
          >
            STALE
          </span>
        )}

        {/* Issue reference */}
        <span style={{ color: "#888", fontSize: 12, marginLeft: "auto" }}>
          {review.issue}
        </span>
      </div>

      {/* Recurse into children */}
      {node.childNodes.map((child) => (
        <StackNode
          key={child.review.id}
          node={child}
          depth={depth + 1}
          onViewDiff={onViewDiff}
        />
      ))}
    </div>
  );
}

/** Renders a complete stack starting from one root node, with a health summary. */
function StackGroup({
  root,
  onViewDiff,
}: {
  readonly root: StackTreeNode;
  readonly onViewDiff: (pr: string) => void;
}) {
  const health = useMemo(() => computeHealth(root), [root]);

  return (
    <div
      style={{
        border: "1px solid #333",
        borderRadius: 8,
        padding: 12,
        marginBottom: 16,
      }}
    >
      {/* Stack header with health summary */}
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          marginBottom: 8,
          paddingBottom: 8,
          borderBottom: "1px solid #333",
        }}
      >
        <span style={{ fontSize: 13, fontWeight: "bold", color: "#ccc" }}>
          Stack: {root.review.branch}
        </span>
        <span style={{ fontSize: 12, color: "#888" }}>
          <span
            style={{
              color: health.approved === health.total ? "#4CAF50" : "#aaa",
            }}
          >
            {health.approved}/{health.total} approved
          </span>
          {health.stale > 0 && (
            <span style={{ color: "#FF5722", marginLeft: 8 }}>
              {health.stale} stale
            </span>
          )}
        </span>
      </div>

      {/* Tree */}
      <StackNode node={root} depth={0} onViewDiff={onViewDiff} />
    </div>
  );
}

/**
 * Multi-stack view showing review dependency relationships as visual trees.
 *
 * Root nodes (reviews with no parents) start separate stacks. Each node
 * shows its PR ref, gate state badge, stale indicator, and issue ref.
 * Clicking a node navigates to the diff view.
 */
export function StackView({ reviews, onViewDiff }: StackViewProps) {
  const trees = useMemo(() => buildStackTrees(reviews), [reviews]);

  // Reviews that have parents but whose parents are not in the review list
  // (orphans that appear as neither roots nor children of any root).
  // These are already captured as roots by buildStackTrees since their
  // parent IDs don't resolve in the lookup map and they effectively
  // become standalone nodes.

  if (reviews.length === 0) {
    return (
      <div style={{ color: "#888", padding: 24, textAlign: "center" }}>
        <p>No reviews loaded.</p>
        <p style={{ fontSize: 13 }}>
          Use the CLI to ingest PRs, then switch to the Stacks view.
        </p>
      </div>
    );
  }

  if (trees.length === 0) {
    return (
      <div style={{ color: "#888", padding: 24, textAlign: "center" }}>
        <p>No stack roots found.</p>
      </div>
    );
  }

  return (
    <div>
      {trees.map((root) => (
        <StackGroup
          key={root.review.id}
          root={root}
          onViewDiff={onViewDiff}
        />
      ))}
    </div>
  );
}
