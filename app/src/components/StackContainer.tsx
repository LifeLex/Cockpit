import { cn } from "@/lib/utils";
import { ReviewCard } from "./ReviewCard";
import type { CardDensity } from "./ReviewCard";
import {
  computeHealth,
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
