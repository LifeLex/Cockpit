import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  GitPullRequest,
  MessageSquare,
  AlertTriangle,
  Bot,
  Layers,
} from "lucide-react";
import { cn } from "@/lib/utils";
import type { Review } from "../bindings/Review";
import type { GateState } from "../bindings/GateState";

interface ReviewCardProps {
  readonly review: Review;
  readonly onAction: (pr: string) => void;
  /**
   * Restack a stale review onto its parent's new head. Wired only for stale
   * reviews; explicit user action operating on the review's own branch.
   */
  readonly onRestack: (pr: string) => void;
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

/** Button variant type matching the Button component's variant prop. */
type ButtonVariant = "default" | "outline" | "ghost";

interface ActionConfig {
  readonly label: string;
  readonly variant: ButtonVariant;
  readonly muted: boolean;
}

/** Determines the action button label, variant, and muted state from the gate state. */
function actionConfigForState(state: GateState): ActionConfig {
  switch (state) {
    case "Pending":
      return { label: "Review", variant: "default", muted: false };
    case "InReview":
      return { label: "Continue", variant: "default", muted: false };
    case "Dispatched":
      return { label: "Waiting…", variant: "outline", muted: true };
    case "Reworked":
      return { label: "Re-review", variant: "default", muted: false };
    case "Approved":
      return { label: "View", variant: "ghost", muted: false };
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

export function ReviewCard({ review, onAction, onRestack }: ReviewCardProps) {
  const { repo, number: prNumber } = parsePrDisplay(review.pr);
  const action = actionConfigForState(review.gate_state);

  return (
    <Card
      className={cn(
        "border-l-4 p-0 transition-colors hover:bg-card/50",
        gateStateBorderClass(review.gate_state),
      )}
    >
      <CardContent className="p-4">
        {/* Top row: PR icon + branch + repo slug + gate state badge */}
        <div className="flex items-center justify-between gap-3">
          <div className="flex min-w-0 items-center gap-2.5">
            <GitPullRequest className="h-4 w-4 shrink-0 text-muted-foreground" />
            <span className="truncate text-sm font-semibold text-foreground">
              {review.branch}
            </span>
            {repo !== "" && (
              <span className="shrink-0 text-xs text-muted-foreground">
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
        <div className="mt-2.5 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
          <span>{review.issue}</span>
          <span className="text-muted-foreground">
            base: {review.base}
          </span>
          {review.comments.length > 0 && (
            <span className="inline-flex items-center gap-1 text-muted-foreground">
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

        {/* Bottom row: context-aware action button */}
        <div className="mt-3 flex items-center gap-2">
          <Button
            variant={action.variant}
            size="sm"
            className={action.muted ? "opacity-60" : undefined}
            onClick={() => {
              onAction(review.pr);
            }}
          >
            {action.label}
          </Button>

          {/* Restack — only for stale reviews; explicit user action on the
              review's own branch. Disabled while the conflict-resolver runs. */}
          {review.stale && (
            <Button
              variant="outline"
              size="sm"
              className="border-danger/40 text-danger hover:bg-danger/10"
              disabled={review.agent != null}
              onClick={() => {
                onRestack(review.pr);
              }}
              title="Rebase this review onto its parent's new head"
            >
              <Layers className="h-3.5 w-3.5" />
              {review.agent != null ? "Restacking…" : "Restack"}
            </Button>
          )}
        </div>
      </CardContent>
    </Card>
  );
}
