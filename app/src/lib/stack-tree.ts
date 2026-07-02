import type { Review } from "../bindings/Review";
import type { ReviewId } from "../bindings/ReviewId";
import { attentionRank } from "./attention";

/** A node in the stack tree, wrapping a review with its resolved children. */
export interface StackTreeNode {
  readonly review: Review;
  readonly childNodes: readonly StackTreeNode[];
}

/** A stack node annotated with its render depth (root = 0). */
export interface FlatStackNode {
  readonly review: Review;
  readonly depth: number;
}

/** Summary statistics for a single stack rooted at a given review. */
export interface StackHealth {
  readonly total: number;
  readonly approved: number;
  readonly merged: number;
  readonly stale: number;
}

/**
 * Build stack trees from the flat review list.
 *
 * A review is a root when none of its parents are present in the passed set:
 * that includes true roots (no parents) and reviews whose parents live outside
 * the set (a cross-project or unfetched parent), which degrade to roots rather
 * than vanishing. Each root's subtree is built by recursively resolving
 * children from the lookup map.
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
    // Degrade to a root when no parent is in the passed set.
    if (review.parents.every((p) => !byId.has(p))) {
      const tree = buildSubtree(review.id, new Set<ReviewId>());
      if (tree !== undefined) {
        roots.push(tree);
      }
    }
  }
  return roots;
}

/**
 * Flatten a stack tree into dependency order (parent before children,
 * pre-order DFS) with each review's depth for indentation. Each review appears
 * at most once even if a malformed DAG reaches it twice.
 */
export function flattenStack(root: StackTreeNode): readonly FlatStackNode[] {
  const out: FlatStackNode[] = [];
  const seen = new Set<ReviewId>();
  function walk(node: StackTreeNode, depth: number): void {
    if (seen.has(node.review.id)) {
      return;
    }
    seen.add(node.review.id);
    out.push({ review: node.review, depth });
    for (const child of node.childNodes) {
      walk(child, depth + 1);
    }
  }
  walk(root, 0);
  return out;
}

/**
 * Compute health stats for a stack by walking its (deduplicated) members, so
 * the totals always match the rendered card count.
 */
export function computeHealth(node: StackTreeNode): StackHealth {
  let approved = 0;
  let merged = 0;
  let stale = 0;
  const nodes = flattenStack(node);
  for (const { review } of nodes) {
    if (review.gate_state === "Approved") approved += 1;
    if (review.gate_state === "Merged") merged += 1;
    if (review.stale) stale += 1;
  }
  return { total: nodes.length, approved, merged, stale };
}

/** Semantic tone for a stack's health dot. */
export type StackStatusTone = "warning" | "approved" | "progress";

/** A stack's health rendered as a status dot tone plus a short label. */
export interface StackStatus {
  readonly tone: StackStatusTone;
  readonly label: string;
}

/**
 * Derive a stack's headline status from its health. A stale member (blocked on
 * an ancestor's rework) dominates; otherwise the stack reads as done when every
 * member is approved or merged, else it reports its ready count.
 */
export function stackStatus(health: StackHealth): StackStatus {
  if (health.stale > 0) {
    return {
      tone: "warning",
      label: health.stale === 1 ? "1 stale" : `${String(health.stale)} stale`,
    };
  }
  const done = health.approved + health.merged;
  if (done === health.total) {
    return {
      tone: "approved",
      label: health.merged === health.total ? "All merged" : "All approved",
    };
  }
  return {
    tone: "progress",
    label: `${String(done)}/${String(health.total)} approved`,
  };
}

/** Whether a review is a candidate for the stack's frontier highlight. */
function isActionable(review: Review): boolean {
  if (review.stale) {
    return false;
  }
  return review.gate_state !== "Approved" && review.gate_state !== "Merged";
}

/**
 * The frontier-eligible member of a stack: the most-urgent actionable review
 * (minimum attention rank among non-stale, non-settled members), or `null` when
 * the whole stack is settled or blocked. Ties break by review id for
 * determinism.
 */
export function frontierReviewId(root: StackTreeNode): ReviewId | null {
  let best: { readonly rank: number; readonly id: ReviewId } | null = null;
  for (const { review } of flattenStack(root)) {
    if (!isActionable(review)) {
      continue;
    }
    const rank = attentionRank(review);
    if (
      best === null ||
      rank < best.rank ||
      (rank === best.rank && review.id < best.id)
    ) {
      best = { rank, id: review.id };
    }
  }
  return best === null ? null : best.id;
}

/**
 * A board entry: either a lone review (rendered as a flat card) or a stack of
 * two-or-more connected reviews (rendered as a container).
 */
export type BoardItem =
  | { readonly kind: "single"; readonly review: Review }
  | {
      readonly kind: "stack";
      readonly root: StackTreeNode;
      readonly nodes: readonly FlatStackNode[];
    };

/** The attention sort key (rank, id) for a stack: its most-urgent member. */
function stackSortKey(nodes: readonly FlatStackNode[]): {
  readonly rank: number;
  readonly id: ReviewId;
} {
  const first = nodes[0];
  // INVARIANT: a stack is only built from a non-empty flattened tree.
  if (first === undefined) {
    throw new Error("stackSortKey called with an empty stack");
  }
  let best = { rank: attentionRank(first.review), id: first.review.id };
  for (const { review } of nodes.slice(1)) {
    const rank = attentionRank(review);
    if (rank < best.rank || (rank === best.rank && review.id < best.id)) {
      best = { rank, id: review.id };
    }
  }
  return best;
}

/**
 * Partition a project group's reviews into board items — singletons and stacks
 * — ordered attention-first. Singletons sort by their own attention rank;
 * stacks sort by their most-urgent member (so a stack interleaves among the
 * lone cards). Every input review appears in exactly one item.
 */
export function buildBoardItems(reviews: readonly Review[]): readonly BoardItem[] {
  const trees = buildStackTrees(reviews);
  const covered = new Set<ReviewId>();
  const ranked: {
    readonly item: BoardItem;
    readonly rank: number;
    readonly id: ReviewId;
  }[] = [];

  for (const tree of trees) {
    const nodes = flattenStack(tree);
    for (const { review } of nodes) {
      covered.add(review.id);
    }
    if (nodes.length <= 1) {
      const review = tree.review;
      ranked.push({
        item: { kind: "single", review },
        rank: attentionRank(review),
        id: review.id,
      });
    } else {
      const key = stackSortKey(nodes);
      ranked.push({
        item: { kind: "stack", root: tree, nodes },
        rank: key.rank,
        id: key.id,
      });
    }
  }

  // Safety net: a malformed graph (e.g. a pure cycle) must never drop a review
  // from the board — surface any uncovered review as a lone card.
  for (const review of reviews) {
    if (!covered.has(review.id)) {
      ranked.push({
        item: { kind: "single", review },
        rank: attentionRank(review),
        id: review.id,
      });
    }
  }

  ranked.sort(
    (a, b) => a.rank - b.rank || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0),
  );
  return ranked.map((r) => r.item);
}
