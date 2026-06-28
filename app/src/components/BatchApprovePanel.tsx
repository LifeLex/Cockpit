import type { Review } from "../bindings/Review";
import type { BatchVerdict } from "../bindings/BatchVerdict";
import type { GateState } from "../bindings/GateState";

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

interface BatchApprovePanelProps {
  readonly verdicts: readonly [Review, BatchVerdict][];
  readonly onApprove: (pr: string) => Promise<void>;
  readonly onApproveAll: () => void;
  readonly onClose: () => void;
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

function verdictColor(verdict: BatchVerdict): string {
  switch (verdict.kind) {
    case "Eligible":
      return "#4CAF50";
    case "Ineligible":
      return "#FF9800";
    default:
      return assertNever(verdict);
  }
}

export function BatchApprovePanel({
  verdicts,
  onApprove,
  onApproveAll,
  onClose,
}: BatchApprovePanelProps) {
  const eligibleCount = verdicts.filter(
    ([, v]) => v.kind === "Eligible",
  ).length;

  return (
    <div
      style={{
        border: "1px solid #444",
        borderRadius: 8,
        padding: 16,
        marginBottom: 16,
        backgroundColor: "#1a1a2e",
      }}
    >
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          marginBottom: 12,
        }}
      >
        <h3 style={{ margin: 0 }}>
          Batch Approve Preview ({eligibleCount}/{verdicts.length} eligible)
        </h3>
        <div style={{ display: "flex", gap: 8 }}>
          {eligibleCount > 0 && (
            <button
              onClick={onApproveAll}
              style={{
                padding: "6px 14px",
                cursor: "pointer",
                backgroundColor: "#4CAF50",
                color: "white",
                border: "none",
                borderRadius: 4,
                fontWeight: "bold",
              }}
            >
              Approve All Eligible ({eligibleCount})
            </button>
          )}
          <button
            onClick={onClose}
            style={{
              padding: "6px 14px",
              cursor: "pointer",
              backgroundColor: "transparent",
              color: "#888",
              border: "1px solid #555",
              borderRadius: 4,
            }}
          >
            Close
          </button>
        </div>
      </div>

      {verdicts.length === 0 && (
        <p style={{ color: "#888" }}>No reviews to evaluate.</p>
      )}

      {verdicts.map(([review, verdict]) => (
        <div
          key={review.id}
          style={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            padding: "8px 12px",
            marginBottom: 4,
            borderRadius: 4,
            borderLeft: `3px solid ${verdictColor(verdict)}`,
            backgroundColor: "#16213e",
          }}
        >
          <div style={{ flex: 1 }}>
            <strong>PR {review.pr}</strong>
            <span style={{ marginLeft: 8, color: "#888", fontSize: 13 }}>
              {gateStateLabel(review.gate_state)}
            </span>
            <div style={{ fontSize: 12, color: "#aaa", marginTop: 2 }}>
              {verdict.reasons.join(" | ")}
            </div>
          </div>
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <span
              style={{
                fontSize: 12,
                fontWeight: "bold",
                color: verdictColor(verdict),
              }}
            >
              {verdict.kind === "Eligible" ? "ELIGIBLE" : "INELIGIBLE"}
            </span>
            {verdict.kind === "Eligible" && (
              <button
                onClick={() => {
                  void onApprove(review.pr);
                }}
                style={{
                  padding: "4px 10px",
                  cursor: "pointer",
                  backgroundColor: "#4CAF50",
                  color: "white",
                  border: "none",
                  borderRadius: 4,
                  fontSize: 12,
                }}
              >
                Approve
              </button>
            )}
          </div>
        </div>
      ))}
    </div>
  );
}
