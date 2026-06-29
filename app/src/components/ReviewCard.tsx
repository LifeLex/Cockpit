import type { Review } from "../bindings/Review";
import type { GateState } from "../bindings/GateState";

interface ReviewCardProps {
  readonly review: Review;
  readonly onOpen?: (pr: string) => void;
  readonly onViewDiff?: (pr: string) => void;
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

function gateStateBgClass(state: GateState): string {
  switch (state) {
    case "Pending":
      return "bg-state-pending";
    case "InReview":
      return "bg-state-in-review";
    case "Dispatched":
      return "bg-state-dispatched";
    case "Reworked":
      return "bg-state-reworked";
    case "Approved":
      return "bg-state-approved";
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

export function ReviewCard({ review, onOpen, onViewDiff }: ReviewCardProps) {
  const canOpen =
    review.gate_state === "Pending" || review.gate_state === "Reworked";

  return (
    <div
      className={`border border-border rounded-lg p-4 mb-3 border-l-4 hover:bg-surface-1/50 transition-colors ${gateStateBorderClass(review.gate_state)}`}
    >
      <div className="flex justify-between items-center">
        <div>
          <strong className="font-bold text-text-primary">
            PR {review.pr}
          </strong>
          <span className="ml-2 text-text-muted">{review.branch}</span>
        </div>
        <span
          className={`px-2 py-0.5 rounded text-xs font-bold text-white ${gateStateBgClass(review.gate_state)}`}
        >
          {gateStateLabel(review.gate_state)}
        </span>
      </div>

      <div className="mt-2 text-sm text-text-secondary">
        Issue: {review.issue} | Base: {review.base}
        {review.stale && (
          <span className="text-danger ml-2">(stale)</span>
        )}
        {review.agent != null && (
          <span className="ml-2 text-warning">
            Agent running (PID: {review.agent.pid})
          </span>
        )}
      </div>

      <div className="mt-2 flex items-center">
        {review.comments.length > 0 && (
          <span className="text-xs text-text-muted">
            {review.comments.length} comment
            {review.comments.length !== 1 ? "s" : ""}
          </span>
        )}
        {canOpen && onOpen != null && (
          <button
            onClick={() => {
              onOpen(review.pr);
            }}
            className="ml-2 px-3 py-1 rounded bg-accent hover:bg-accent-hover text-white text-sm border-none cursor-pointer"
          >
            Open for Review
          </button>
        )}
        {onViewDiff != null && (
          <button
            onClick={() => {
              onViewDiff(review.pr);
            }}
            className="ml-2 px-3 py-1 rounded bg-surface-2 hover:bg-surface-3 text-text-secondary text-sm border border-border cursor-pointer"
          >
            View Diff
          </button>
        )}
      </div>
    </div>
  );
}
