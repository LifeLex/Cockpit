import { useCallback, type ReactNode } from "react";
import { Layers, Loader2 } from "lucide-react";
import { cn } from "@/lib/utils";
import { ReviewCard } from "./ReviewCard";
import type { CardDensity } from "./ReviewCard";
import { useAppStore } from "../store";
import {
  computeHealth,
  flattenStack,
  frontierReviewId,
  stackStatus,
} from "../lib/stack-tree";
import type { FlatStackNode, StackStatusTone, StackTreeNode } from "../lib/stack-tree";

interface StackContainerProps {
  /** The stack's root, used to derive health and the frontier highlight. */
  readonly root: StackTreeNode;
  /** The stack's members in dependency order (parent first) with depth. */
  readonly nodes: readonly FlatStackNode[];
  /** Board density; forwarded to each member card. */
  readonly density: CardDensity;
  readonly onAction: (pr: string) => void;
  readonly onRestack: (pr: string) => void;
}

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** Status-dot background class for a stack health tone. */
function dotClass(tone: StackStatusTone): string {
  switch (tone) {
    case "warning":
      return "bg-warning";
    case "approved":
      return "bg-state-approved";
    case "progress":
      return "bg-state-in-review";
    default:
      return assertNever(tone);
  }
}

/** Text class paired with the status dot (so the label never reads by color alone). */
function statusTextClass(tone: StackStatusTone): string {
  switch (tone) {
    case "warning":
      return "text-warning";
    case "approved":
      return "text-state-approved";
    case "progress":
      return "text-state-in-review";
    default:
      return assertNever(tone);
  }
}

/** Depth indent in pixels, capped at three visual levels. */
function indentFor(depth: number): number {
  return Math.min(depth, 3) * 16;
}

/**
 * A stack of two-or-more connected reviews rendered as one unit: a header with
 * a mono "STACK · N PRs" label and a health summary (status dot + short label),
 * then the members in dependency order down a left rail. The rail segment beside
 * the frontier-eligible member is tinted with the brand teal.
 */
export function StackContainer({
  root,
  nodes,
  density,
  onAction,
  onRestack,
}: StackContainerProps) {
  const health = computeHealth(root);
  const status = stackStatus(health);
  const frontierId = frontierReviewId(root);

  // -- D3: whole-stack restack control --
  const rootPr = root.review.pr;
  const restackStack = useAppStore((s) => s.restackStack);
  const progress = useAppStore((s) => s.restackProgress[rootPr]);
  const staleCount = health.stale;
  // Refuse the offer while any member is mid-rework: restacking a branch out from
  // under a running agent would clobber its work (the backend guards this too).
  const anyAgent = flattenStack(root).some((m) => m.review.agent != null);

  const handleRestackStack = useCallback(() => {
    // Explicit, confirmed side effect (§9). Spawns a conflict-resolver agent on
    // conflict, so the confirm text says so.
    const confirmed = window.confirm(
      `Restack ${String(staleCount)} stale PR${
        staleCount === 1 ? "" : "s"
      } onto their parents in dependency order?\n\nOn a conflict, an agent is dispatched to resolve it and the remaining PRs stay stale until it lands.`,
    );
    if (!confirmed) return;
    void restackStack(rootPr);
  }, [restackStack, rootPr, staleCount]);

  let restackControl: ReactNode = null;
  if (
    progress !== undefined &&
    (progress.status === "restacking" || progress.status === "clean")
  ) {
    restackControl = (
      <span className="inline-flex items-center gap-1.5 text-xs text-state-in-review">
        <Loader2 className="h-3 w-3 animate-spin" aria-hidden="true" />
        Restacking {progress.current}/{progress.total}
        {progress.current_pr !== "" ? ` — ${progress.current_pr}` : ""}
      </span>
    );
  } else if (progress?.status === "conflict") {
    restackControl = (
      <span className="inline-flex items-center gap-1.5 text-xs text-warning">
        <span
          className="h-1.5 w-1.5 shrink-0 rounded-full bg-warning"
          aria-hidden="true"
        />
        conflict on {progress.current_pr} — resolver dispatched, remaining PRs
        still stale
      </span>
    );
  } else if (progress?.status === "error") {
    restackControl = (
      <span className="inline-flex items-center gap-1.5 text-xs text-danger">
        <span
          className="h-1.5 w-1.5 shrink-0 rounded-full bg-danger"
          aria-hidden="true"
        />
        restack failed
        {progress.current_pr !== "" ? ` on ${progress.current_pr}` : ""}
      </span>
    );
  } else if (staleCount > 0 && !anyAgent) {
    restackControl = (
      <button
        type="button"
        onClick={handleRestackStack}
        className="inline-flex cursor-pointer items-center gap-1.5 rounded-md border border-warning/40 bg-transparent px-2 py-1 text-xs font-medium text-warning transition-colors hover:bg-warning/10"
        title="Rebase every stale member onto its parent, in dependency order"
      >
        <Layers className="h-3 w-3" />
        Restack stack ({staleCount} stale, in order)
      </button>
    );
  }

  return (
    <div className="rounded-xl border border-border bg-card/30">
      <div className="flex items-center gap-2 px-3 pb-1.5 pt-2.5">
        <span className="font-mono text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
          STACK · {health.total} PRs
        </span>
        <span className="ml-auto inline-flex items-center gap-1.5">
          <span
            className={cn("h-2 w-2 rounded-full", dotClass(status.tone))}
            aria-hidden="true"
          />
          <span className={cn("text-xs font-medium", statusTextClass(status.tone))}>
            {status.label}
          </span>
        </span>
      </div>

      {restackControl !== null && (
        <div className="px-3 pb-1.5">{restackControl}</div>
      )}

      <div className={cn("px-2 pb-2", density === "compact" ? "space-y-1" : "space-y-2")}>
        {nodes.map(({ review, depth }) => {
          const isFrontier = review.id === frontierId;
          return (
            <div key={review.id} className="flex items-stretch">
              <div
                className={cn(
                  "w-0.5 shrink-0 rounded-full",
                  isFrontier ? "bg-brand" : "bg-border",
                )}
                aria-hidden="true"
              />
              <div
                className="min-w-0 flex-1 pl-2.5"
                style={{ marginLeft: `${String(indentFor(depth))}px` }}
              >
                <ReviewCard
                  review={review}
                  density={density}
                  onAction={onAction}
                  onRestack={onRestack}
                  inStack
                />
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
