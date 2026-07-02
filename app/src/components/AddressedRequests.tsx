/**
 * "Addressed requests" checklist for interdiff re-review (D1/D10).
 *
 * Lists the requests the reviewer dispatched to the agent in the most recent
 * cycle (from the review's [`DispatchSnapshot`]), each paired to the interdiff
 * region that answers it via {@link pairRequests}. A matched request shows an
 * "answered in path:line" marker and scrolls the diff there on click; an
 * unmatched one shows a "no matching change detected" marker so the reviewer
 * knows to look before approving.
 *
 * It is history, not live state: there are deliberately NO edit / delete /
 * resolve affordances (Invariant §0.4 — comments are ephemeral and
 * single-cycle). The pairing is advisory only — it never blocks the gate.
 */

import { Check, ChevronDown, ChevronRight, History, Search } from "lucide-react";
import type { Anchor } from "../bindings/Anchor";
import type { DiffSide } from "../bindings/DiffSide";
import type { PairingResult } from "@/lib/request-pairing";
import { cn } from "@/lib/utils";

interface AddressedRequestsProps {
  /** The dispatched requests paired against the interdiff. */
  readonly pairings: readonly PairingResult[];
  /** Whether the panel is expanded. */
  readonly open: boolean;
  /** Toggle the expanded state. */
  readonly onToggle: () => void;
  /** Scroll the diff to a matched request's interdiff region. */
  readonly onJumpTo: (path: string, side: DiffSide, line: number) => void;
}

/** The file path an anchor points at, or null for non-diff anchors. */
function anchorPath(anchor: Anchor): string | null {
  if ("DiffLine" in anchor) {
    return anchor.DiffLine.path;
  }
  return null;
}

/** The inclusive line range an anchor points at, or null for non-diff anchors. */
function anchorRange(anchor: Anchor): readonly [number, number] | null {
  if ("DiffLine" in anchor) {
    return anchor.DiffLine.range;
  }
  return null;
}

/** Which diff side an anchor refers to; `New` for non-diff/legacy anchors. */
function anchorSide(anchor: Anchor): DiffSide {
  if ("DiffLine" in anchor) {
    return anchor.DiffLine.side;
  }
  return "New";
}

/** Format an anchor as a compact `path:Lstart–Lend` location label. */
function locationLabel(anchor: Anchor): string {
  const path = anchorPath(anchor);
  const range = anchorRange(anchor);
  if (path === null) return "—";
  if (range === null) return path;
  const [start, end] = range;
  const lines = start === end ? `L${String(start)}` : `L${String(start)}–${String(end)}`;
  return `${path}:${lines}`;
}

/** The answered / unanswered marker beneath a request body. */
function StatusMarker({ match }: { readonly match: PairingResult["match"] }) {
  if (match !== null) {
    return (
      <span className="inline-flex items-center gap-1 text-[10px] font-medium text-state-approved">
        <Check className="h-3 w-3" />
        answered in {match.path}:{String(match.realLine)}
      </span>
    );
  }
  return (
    <span className="inline-flex items-center gap-1 text-[10px] font-medium text-warning">
      <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-warning" aria-hidden="true" />
      no matching change detected
    </span>
  );
}

/**
 * A collapsible checklist of the previous cycle's dispatched requests, each
 * paired to the interdiff change that answers it.
 */
export function AddressedRequests({
  pairings,
  open,
  onToggle,
  onJumpTo,
}: AddressedRequestsProps) {
  const answered = pairings.filter((p) => p.match !== null).length;

  return (
    <div className="shrink-0 border-b border-border bg-card/50">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        className={cn(
          "flex w-full cursor-pointer items-center gap-1.5 border-none bg-transparent px-4 py-1.5",
          "text-[10px] font-semibold uppercase tracking-wide text-muted-foreground hover:text-foreground",
        )}
        title="Toggle addressed requests"
      >
        {open ? (
          <ChevronDown className="h-3 w-3" />
        ) : (
          <ChevronRight className="h-3 w-3" />
        )}
        <History className="h-3 w-3" />
        Addressed requests
        <span className="font-mono tabular-nums text-muted-foreground/70">
          {String(answered)}/{String(pairings.length)}
        </span>
        <span className="ml-1 font-sans normal-case tracking-normal text-muted-foreground/60">
          answered — click one to jump to its change
        </span>
      </button>

      {open && (
        <ul className="space-y-1.5 px-4 pb-2.5 pl-9 text-xs">
          {pairings.map(({ comment, match }) => {
            const location = locationLabel(comment.anchor);
            const side = anchorSide(comment.anchor);

            // Matched requests are clickable: clicking scrolls the diff to the
            // interdiff region (reusing DiffView's onJumpTo reveal machinery).
            if (match !== null) {
              return (
                <li key={comment.id}>
                  <button
                    type="button"
                    onClick={() => {
                      onJumpTo(match.path, side, match.realLine);
                    }}
                    className="flex w-full cursor-pointer flex-col items-start gap-0.5 rounded-md border border-border bg-background/40 px-2.5 py-1.5 text-left transition-colors hover:border-state-approved/50 hover:bg-state-approved/5"
                    title={`Jump to ${match.path}:${String(match.realLine)}`}
                  >
                    <span className="font-mono text-[10px] text-muted-foreground">
                      {location}
                    </span>
                    <span className="whitespace-pre-wrap break-words leading-relaxed text-foreground">
                      {comment.body}
                    </span>
                    <StatusMarker match={match} />
                  </button>
                </li>
              );
            }

            return (
              <li
                key={comment.id}
                className="rounded-md border border-warning/30 bg-warning/5 px-2.5 py-1.5"
              >
                <div className="mb-0.5 font-mono text-[10px] text-muted-foreground">
                  {location}
                </div>
                <div className="whitespace-pre-wrap break-words leading-relaxed text-foreground">
                  {comment.body}
                </div>
                <div className="mt-0.5">
                  <StatusMarker match={match} />
                </div>
              </li>
            );
          })}
          {pairings.length === 0 && (
            <li className="inline-flex items-center gap-1 italic text-muted-foreground">
              <Search className="h-3 w-3" />
              No requests were dispatched last cycle.
            </li>
          )}
        </ul>
      )}
    </div>
  );
}
