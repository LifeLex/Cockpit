import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  GitPullRequest,
  MessageSquare,
  AlertTriangle,
  Bot,
  ExternalLink,
} from "lucide-react";
import { cn } from "@/lib/utils";
import type { Review } from "../bindings/Review";
import type { GateState } from "../bindings/GateState";

interface ReviewCardProps {
  readonly review: Review;
  readonly onOpen?: ((pr: string) => void) | undefined;
  readonly onViewDiff?: ((pr: string) => void) | undefined;
}

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

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

function gateStateBadgeClass(state: GateState): string {
  switch (state) {
    case "Pending":
      return "bg-state-pending/20 text-state-pending border-state-pending/30";
    case "InReview":
      return "bg-state-in-review/20 text-state-in-review border-state-in-review/30";
    case "Dispatched":
      return "bg-state-dispatched/20 text-state-dispatched border-state-dispatched/30";
    case "Reworked":
      return "bg-state-reworked/20 text-state-reworked border-state-reworked/30";
    case "Approved":
      return "bg-state-approved/20 text-state-approved border-state-approved/30";
    default:
      return assertNever(state);
  }
}

function parsePrDisplay(pr: string): { repo: string; number: string } {
  const match = /github\.com\/([^/]+\/[^/]+)\/pull\/(\d+)/.exec(pr);
  if (match !== null) {
    const [, repo, num] = match;
    if (repo !== undefined && num !== undefined) {
      return { repo, number: num };
    }
  }
  return { repo: "", number: pr };
}

export function ReviewCard({ review, onOpen, onViewDiff }: ReviewCardProps) {
  const canOpen =
    review.gate_state === "Pending" || review.gate_state === "Reworked";
  const { repo, number: prNumber } = parsePrDisplay(review.pr);

  return (
    <Card
      className={cn(
        "border-l-4 p-0 transition-colors hover:bg-surface-1/50",
        gateStateBorderClass(review.gate_state),
      )}
    >
      <CardContent className="p-4">
        {/* Top row: PR icon + branch + repo slug + gate state badge */}
        <div className="flex items-center justify-between gap-3">
          <div className="flex min-w-0 items-center gap-2.5">
            <GitPullRequest className="h-4 w-4 shrink-0 text-text-muted" />
            <span className="truncate text-sm font-semibold text-text-primary">
              {review.branch}
            </span>
            {repo !== "" && (
              <span className="shrink-0 text-xs text-text-muted">
                {repo}#{prNumber}
              </span>
            )}
          </div>
          <Badge
            variant="outline"
            className={cn(
              "shrink-0",
              gateStateBadgeClass(review.gate_state),
            )}
          >
            {gateStateLabel(review.gate_state)}
          </Badge>
        </div>

        {/* Middle row: issue ref, base branch, comment count, stale, agent */}
        <div className="mt-2.5 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-text-secondary">
          <span>{review.issue}</span>
          <span className="text-text-muted">
            base: {review.base}
          </span>
          {review.comments.length > 0 && (
            <span className="inline-flex items-center gap-1 text-text-muted">
              <MessageSquare className="h-3 w-3" />
              {review.comments.length}
            </span>
          )}
          {review.stale && (
            <span className="inline-flex items-center gap-1 text-danger">
              <AlertTriangle className="h-3 w-3" />
              Stale
            </span>
          )}
          {review.agent != null && (
            <span className="inline-flex items-center gap-1 text-warning">
              <Bot className="h-3 w-3" />
              PID {review.agent.pid}
            </span>
          )}
        </div>

        {/* Bottom row: action buttons */}
        {(canOpen && onOpen != null || onViewDiff != null) && (
          <div className="mt-3 flex items-center gap-2">
            {canOpen && onOpen != null && (
              <Button
                size="sm"
                onClick={() => {
                  onOpen(review.pr);
                }}
              >
                Open for Review
              </Button>
            )}
            {onViewDiff != null && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => {
                  onViewDiff(review.pr);
                }}
              >
                <ExternalLink className="h-3.5 w-3.5" />
                View Diff
              </Button>
            )}
          </div>
        )}
      </CardContent>
    </Card>
  );
}
