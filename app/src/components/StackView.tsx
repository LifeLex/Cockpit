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

/** Map from GateState to a Tailwind background class for badges. */
function gateStateBgClass(state: GateState): string {
  switch (state) {
    case "Pending":
      return "bg-state-pending";
    case "InReview":
      return "bg-state-in-review";
    case "Dispatched":
      return "bg-state-dispatched";
    case "Reworked":
      return "bg-state-reworked";
    case "Approved":
      return "bg-state-approved";
    default:
      return assertNever(state);
  }
}

/** Map from GateState to a Tailwind border-left class for tree connectors. */
function gateStateBorderClass(state: GateState): string {
  switch (state) {
    case "Pending":
      return "border-l-state-pending";
    case "InReview":
      return "border-l-state-in-review";
    case "Dispatched":
      return "border-l-state-dispatched";
    case "Reworked":
      return "border-l-state-reworked";
    case "Approved":
      return "border-l-state-approved";
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
  const borderClass = gateStateBorderClass(review.gate_state);

  return (
    // Dynamic marginLeft: Tailwind cannot compute depth * 28 at build time.
    <div style={{ marginLeft: depth * 28 }}>
      <div
        className={`flex items-center gap-2 px-2.5 py-1.5 my-0.5 rounded-r border-l-[3px] bg-surface-1 cursor-pointer hover:bg-surface-2 ${borderClass}`}
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
          <span className="inline-block w-3 h-0.5 bg-border-strong shrink-0" />
        )}

        {/* PR reference */}
        <span className="font-bold text-[13px] text-text-primary">
          {review.pr}
        </span>

        {/* Gate state badge */}
        <span
          className={`px-1.5 py-px rounded-sm text-[11px] font-bold text-white shrink-0 ${gateStateBgClass(review.gate_state)}`}
        >
          {gateStateLabel(review.gate_state)}
        </span>

        {/* Stale indicator */}
        {review.stale && (
          <span className="text-danger text-[11px] font-bold shrink-0">
            STALE
          </span>
        )}

        {/* Issue reference */}
        <span className="text-text-muted text-xs ml-auto">{review.issue}</span>
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
    <div className="border border-border rounded-lg p-3 mb-4">
      {/* Stack header with health summary */}
      <div className="flex justify-between items-center mb-2 pb-2 border-b border-border">
        <span className="text-[13px] font-bold text-text-secondary">
          Stack: {root.review.branch}
        </span>
        <span className="text-xs text-text-muted">
          <span
            className={
              health.approved === health.total ? "text-success" : "text-text-muted"
            }
          >
            {health.approved}/{health.total} approved
          </span>
          {health.stale > 0 && (
            <span className="text-danger ml-2">{health.stale} stale</span>
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
      <div className="text-text-muted p-6 text-center">
        <p>No reviews loaded.</p>
        <p className="text-[13px]">
          Use the CLI to ingest PRs, then switch to the Stacks view.
        </p>
      </div>
    );
  }

  if (trees.length === 0) {
    return (
      <div className="text-text-muted p-6 text-center">
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
