/**
 * Read-only "Addressed requests" panel for interdiff re-review (D10).
 *
 * When a review is `Reworked`, this lists the comments that were dispatched to
 * the agent in the most recent cycle (from the review's [`DispatchSnapshot`]),
 * so the reviewer can see exactly what was asked for while inspecting the
 * changes since. It is history, not live state: there are deliberately NO
 * edit / delete / resolve affordances (Invariant §0.4 — comments are ephemeral
 * and single-cycle).
 */

import { ChevronDown, ChevronRight, History } from "lucide-react";
import type { Comment } from "../bindings/Comment";
import type { Anchor } from "../bindings/Anchor";
import { cn } from "@/lib/utils";

interface AddressedRequestsProps {
  /** The comments dispatched in the last review cycle. */
  readonly comments: readonly Comment[];
  /** Whether the panel is expanded. */
  readonly open: boolean;
  /** Toggle the expanded state. */
  readonly onToggle: () => void;
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

/**
 * A collapsible band listing the previous cycle's dispatched review comments.
 */
export function AddressedRequests({
  comments,
  open,
  onToggle,
}: AddressedRequestsProps) {
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
          {String(comments.length)}
        </span>
        <span className="ml-1 font-sans normal-case tracking-normal text-muted-foreground/60">
          from your last review
        </span>
      </button>

      {open && (
        <ul className="space-y-1.5 px-4 pb-2.5 pl-9 text-xs">
          {comments.map((comment) => (
            <li
              key={comment.id}
              className="rounded-md border border-border bg-background/40 px-2.5 py-1.5"
            >
              <div className="mb-0.5 font-mono text-[10px] text-muted-foreground">
                {locationLabel(comment.anchor)}
              </div>
              <div className="whitespace-pre-wrap break-words leading-relaxed text-foreground">
                {comment.body}
              </div>
            </li>
          ))}
          {comments.length === 0 && (
            <li className="italic text-muted-foreground">
              No requests were dispatched last cycle.
            </li>
          )}
        </ul>
      )}
    </div>
  );
}
