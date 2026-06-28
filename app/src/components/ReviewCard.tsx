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

function gateStateColor(state: GateState): string {
  switch (state) {
    case "Pending":
      return "#888";
    case "InReview":
      return "#2196F3";
    case "Dispatched":
      return "#FF9800";
    case "Reworked":
      return "#9C27B0";
    case "Approved":
      return "#4CAF50";
    default:
      return assertNever(state);
  }
}

export function ReviewCard({ review, onOpen, onViewDiff }: ReviewCardProps) {
  const canOpen =
    review.gate_state === "Pending" || review.gate_state === "Reworked";

  return (
    <div
      style={{
        border: "1px solid #333",
        borderRadius: 8,
        padding: 16,
        marginBottom: 12,
        borderLeft: `4px solid ${gateStateColor(review.gate_state)}`,
      }}
    >
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
        }}
      >
        <div>
          <strong>PR {review.pr}</strong>
          <span style={{ marginLeft: 8, color: "#888" }}>{review.branch}</span>
        </div>
        <span
          style={{
            padding: "2px 8px",
            borderRadius: 4,
            backgroundColor: gateStateColor(review.gate_state),
            color: "white",
            fontSize: 12,
          }}
        >
          {gateStateLabel(review.gate_state)}
        </span>
      </div>

      <div style={{ marginTop: 8, fontSize: 14, color: "#aaa" }}>
        Issue: {review.issue} | Base: {review.base}
        {review.stale && (
          <span style={{ color: "#FF5722", marginLeft: 8 }}>(stale)</span>
        )}
        {review.agent != null && (
          <span style={{ marginLeft: 8, color: "#FF9800" }}>
            Agent running (PID: {review.agent.pid})
          </span>
        )}
      </div>

      <div style={{ marginTop: 8 }}>
        {review.comments.length > 0 && (
          <span style={{ fontSize: 12, color: "#888" }}>
            {review.comments.length} comment
            {review.comments.length !== 1 ? "s" : ""}
          </span>
        )}
        {canOpen && onOpen != null && (
          <button
            onClick={() => {
              onOpen(review.pr);
            }}
            style={{ marginLeft: 8, cursor: "pointer" }}
          >
            Open for Review
          </button>
        )}
        {onViewDiff != null && (
          <button
            onClick={() => {
              onViewDiff(review.pr);
            }}
            style={{ marginLeft: 8, cursor: "pointer" }}
          >
            View Diff
          </button>
        )}
      </div>
    </div>
  );
}
