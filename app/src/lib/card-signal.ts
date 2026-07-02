/**
 * Derive the review-forward "why" signal for a PR card: the headline reason a
 * review needs the reviewer's attention, plus a semantic tone used to color it.
 *
 * The reason is derived only from fields present on every list-item `Review`
 * (gate state, staleness, running agent). CI status is not an input here — the
 * list-item review does not carry loaded checks — so a "CI failing" headline is
 * surfaced by the card only when checks are separately available.
 */

import type { Review } from "../bindings/Review";
import type { GateState } from "../bindings/GateState";
import { elapsedSince } from "./relative-time";
import { attentionReasons } from "./attention";

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** Semantic tone for the reason line, mapped to `--color-*` in the card. */
export type SignalTone =
  | "attention"
  | "running"
  | "warning"
  | "done"
  | "neutral";

/** The derived headline for a card. */
export interface CardSignal {
  /** Short human-readable reason, e.g. `Needs your review`. */
  readonly reason: string;
  /** Semantic tone driving the reason line color. */
  readonly tone: SignalTone;
  /**
   * Optional secondary risk note (e.g. `CI failing`, `Large diff`) layered
   * *under* the gate reason on the same line. Present only for actionable
   * reviews that carry a risk signal; the gate/stale reason always leads.
   */
  readonly note?: string;
}

/**
 * Attach the sharpest risk note to an actionable review's signal, keeping the
 * gate reason as the loud headline. Precedence follows {@link attentionReasons}
 * (CI failing, then size, then sensitive paths); `undefined` when the review is
 * clean, so `note` is omitted rather than set empty.
 */
function withRiskNote(signal: CardSignal, review: Review): CardSignal {
  const note = attentionReasons(review)[0];
  return note === undefined ? signal : { ...signal, note };
}

/**
 * Compute the card's headline signal. `now` is injected for deterministic tests
 * and defaults to the wall clock (used for the running-agent elapsed time).
 */
export function cardSignal(review: Review, now: number = Date.now()): CardSignal {
  // A stale review is blocked on an ancestor's rework; that supersedes the
  // gate-state reason regardless of where it sits in the loop.
  if (review.stale) {
    return { reason: "Restack needed", tone: "warning" };
  }

  const state: GateState = review.gate_state;
  switch (state) {
    case "Pending":
      return withRiskNote(
        { reason: "Needs your review", tone: "attention" },
        review,
      );
    case "InReview":
      return withRiskNote(
        { reason: "Needs your review", tone: "attention" },
        review,
      );
    case "Reworked":
      return withRiskNote(
        { reason: "Agent reworked — re-review", tone: "attention" },
        review,
      );
    case "Dispatched": {
      const elapsed =
        review.agent !== null
          ? elapsedSince(review.agent.started_at, now)
          : null;
      return {
        reason:
          elapsed !== null ? `Agent working · ${elapsed}` : "Agent working",
        tone: "running",
      };
    }
    case "Approved":
      return { reason: "Approved", tone: "done" };
    case "Merged":
      return { reason: "Merged", tone: "done" };
    default:
      return assertNever(state);
  }
}
