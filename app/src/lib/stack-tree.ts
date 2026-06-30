import type { Review } from "../bindings/Review";
import type { ReviewId } from "../bindings/ReviewId";

/** A node in the stack tree, wrapping a review with its resolved children. */
export interface StackTreeNode {
  readonly review: Review;
  readonly childNodes: readonly StackTreeNode[];
}

/** Summary statistics for a single stack rooted at a given review. */
export interface StackHealth {
  readonly total: number;
  readonly approved: number;
  readonly stale: number;
}

/**
 * Build stack trees from the flat review list.
 *
 * Roots are reviews with no parents. Each root's subtree is built by
 * recursively resolving children from the lookup map.
 */
export function buildStackTrees(reviews: readonly Review[]): readonly StackTreeNode[] {
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
export function computeHealth(node: StackTreeNode): StackHealth {
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
