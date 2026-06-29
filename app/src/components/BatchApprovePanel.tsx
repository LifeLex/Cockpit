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

function verdictBgClass(verdict: BatchVerdict): string {
  switch (verdict.kind) {
    case "Eligible":
      return "border-l-success";
    case "Ineligible":
      return "border-l-warning";
    default:
      return assertNever(verdict);
  }
}

function verdictTextClass(verdict: BatchVerdict): string {
  switch (verdict.kind) {
    case "Eligible":
      return "text-success";
    case "Ineligible":
      return "text-warning";
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
    <div className="border border-border rounded-lg p-4 mb-4 bg-surface-1">
      <div className="flex justify-between items-center mb-3">
        <h3 className="m-0 text-text-primary">
          Batch Approve Preview ({eligibleCount}/{verdicts.length} eligible)
        </h3>
        <div className="flex gap-2">
          {eligibleCount > 0 && (
            <button
              onClick={onApproveAll}
              className="px-4 py-1.5 bg-success text-white border-none rounded font-bold cursor-pointer hover:opacity-90"
            >
              Approve All Eligible ({eligibleCount})
            </button>
          )}
          <button
            onClick={onClose}
            className="px-4 py-1.5 bg-transparent text-text-muted border border-border rounded cursor-pointer hover:bg-surface-2"
          >
            Close
          </button>
        </div>
      </div>

      {verdicts.length === 0 && (
        <p className="text-text-muted">No reviews to evaluate.</p>
      )}

      {verdicts.map(([review, verdict]) => (
        <div
          key={review.id}
          className={`flex justify-between items-center px-3 py-2 mb-1 rounded border-l-[3px] bg-surface-2 ${verdictBgClass(verdict)}`}
        >
          <div className="flex-1">
            <strong className="font-bold text-text-primary">
              PR {review.pr}
            </strong>
            <span className="ml-2 text-text-muted text-[13px]">
              {gateStateLabel(review.gate_state)}
            </span>
            <div className="text-xs text-text-secondary mt-0.5">
              {verdict.reasons.join(" | ")}
            </div>
          </div>
          <div className="flex items-center gap-2">
            <span
              className={`text-xs font-bold ${verdictTextClass(verdict)}`}
            >
              {verdict.kind === "Eligible" ? "ELIGIBLE" : "INELIGIBLE"}
            </span>
            {verdict.kind === "Eligible" && (
              <button
                onClick={() => {
                  void onApprove(review.pr);
                }}
                className="px-2.5 py-1 bg-success text-white border-none rounded text-xs cursor-pointer"
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
