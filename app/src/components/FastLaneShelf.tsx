/**
 * Fast lane shelf (C2): a visually distinct, teal-framed group of
 * "small + green + low-risk" reviews surfaced above the project groups.
 *
 * The shelf compresses the *decision* — these are the reviews a human can
 * approve in ~2 minutes — but never the *authority*: its cards use the exact
 * same {@link ReviewCard} action path as the rest of the board, so there is
 * deliberately no batch-approve or auto-approve here (roadmap C2 / CLAUDE.md
 * §9). Membership is decided by {@link isFastLane}; App excludes members from
 * the project groups below so nothing renders twice.
 */

import { Zap } from "lucide-react";
import { ReviewCard } from "./ReviewCard";
import type { CardDensity } from "./ReviewCard";
import type { Review } from "../bindings/Review";
import { attentionRank } from "../lib/attention";

interface FastLaneShelfProps {
  /** The fast-lane members to render; the shelf is not rendered when empty. */
  readonly reviews: readonly Review[];
  /** Presentation density, mirroring the rest of the board. */
  readonly density: CardDensity;
  /** Same handler the board uses — no new terminal action is introduced here. */
  readonly onAction: (pr: string) => void;
  /** Restack a stale member; wired through for parity with the board cards. */
  readonly onRestack: (pr: string) => void;
}

/** Order the shelf attention-first, using the board's rank + id tiebreak. */
function orderByAttention(reviews: readonly Review[]): readonly Review[] {
  return [...reviews].sort((a, b) => {
    const byRank = attentionRank(a) - attentionRank(b);
    if (byRank !== 0) return byRank;
    return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  });
}

/** The teal-framed fast-lane shelf. */
export function FastLaneShelf({
  reviews,
  density,
  onAction,
  onRestack,
}: FastLaneShelfProps) {
  const ordered = orderByAttention(reviews);
  const count = ordered.length;

  return (
    <section className="mb-6 rounded-xl border border-brand/40 bg-brand/5 p-3">
      <h2 className="mb-3 flex items-center gap-1.5 font-mono text-xs tracking-wide text-brand">
        <Zap className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
        <span className="font-semibold uppercase">Fast lane</span>
        <span className="text-brand/70">
          · {String(count)} {count === 1 ? "PR" : "PRs"} · small + green +
          low-risk
        </span>
      </h2>
      <div className={density === "compact" ? "space-y-1.5" : "space-y-3"}>
        {ordered.map((review) => (
          <ReviewCard
            key={review.id}
            review={review}
            density={density}
            onAction={onAction}
            onRestack={onRestack}
          />
        ))}
      </div>
    </section>
  );
}
