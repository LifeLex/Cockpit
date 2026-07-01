/**
 * Attention-first ordering for the PRs board.
 *
 * The board leads with what needs the reviewer. `sortByAttention` produces a
 * stable ordering where reviews awaiting a human decision rise and settled or
 * blocked-by-ancestor work sinks. CI status is intentionally *not* an input:
 * the list-item `Review` does not carry loaded checks, so the rank is derived
 * only from fields present on every review (gate state, staleness, comments).
 */

import type { Review } from "../bindings/Review";
import type { GateState } from "../bindings/GateState";

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/**
 * Base rank for a gate state. Lower sorts first (more attention). Reworked and
 * InReview are the reviewer's active queue; Pending is next; Dispatched is
 * agent-owned and waiting; Approved is done and sinks.
 */
function gateStateRank(state: GateState): number {
  switch (state) {
    case "Reworked":
      return 0;
    case "InReview":
      return 1;
    case "Pending":
      return 2;
    case "Dispatched":
      return 3;
    case "Approved":
      return 4;
    // Merged is fully settled — the lowest urgency, below Approved.
    case "Merged":
      return 5;
    default:
      return assertNever(state);
  }
}

/**
 * Full attention rank for a review. Lower sorts first. A stale review is
 * deprioritized past every non-stale one (it is blocked on an ancestor's
 * rework), while preserving relative order among stale reviews by gate state.
 */
export function attentionRank(review: Review): number {
  const stalePenalty = review.stale ? 100 : 0;
  return stalePenalty + gateStateRank(review.gate_state);
}

/**
 * Return a new array of reviews sorted attention-first. Stable within equal
 * ranks (uses `id` as the tiebreaker) so ordering is deterministic. Does not
 * mutate the input.
 */
export function sortByAttention(reviews: readonly Review[]): readonly Review[] {
  return [...reviews].sort((a, b) => {
    const byRank = attentionRank(a) - attentionRank(b);
    if (byRank !== 0) return byRank;
    return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  });
}
