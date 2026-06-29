import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CheckCircle2, XCircle, X } from "lucide-react";
import { cn } from "@/lib/utils";
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

function verdictBorderClass(verdict: BatchVerdict): string {
  switch (verdict.kind) {
    case "Eligible":
      return "border-l-success";
    case "Ineligible":
      return "border-l-warning";
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
    <Card className="mb-4 p-0">
      <CardContent className="p-4">
        <div className="flex items-center justify-between mb-4">
          <div>
            <h3 className="text-sm font-semibold text-text-primary">
              Batch Approve Preview
            </h3>
            <p className="mt-0.5 text-xs text-text-muted">
              {eligibleCount}/{verdicts.length} eligible
            </p>
          </div>
          <div className="flex items-center gap-2">
            {eligibleCount > 0 && (
              <Button
                size="sm"
                className="bg-success text-white hover:bg-success/90"
                onClick={onApproveAll}
              >
                <CheckCircle2 className="h-3.5 w-3.5" />
                Approve All ({eligibleCount})
              </Button>
            )}
            <Button variant="ghost" size="icon" onClick={onClose}>
              <X className="h-4 w-4" />
            </Button>
          </div>
        </div>

        {verdicts.length === 0 && (
          <p className="text-sm text-text-muted py-2">
            No reviews to evaluate.
          </p>
        )}

        <div className="space-y-1.5">
          {verdicts.map(([review, verdict]) => (
            <div
              key={review.id}
              className={cn(
                "flex items-center justify-between rounded-lg border-l-[3px] bg-surface-2 px-3 py-2.5",
                verdictBorderClass(verdict),
              )}
            >
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="text-sm font-medium text-text-primary truncate">
                    {review.branch}
                  </span>
                  <Badge variant="outline" className="shrink-0 text-[10px]">
                    {gateStateLabel(review.gate_state)}
                  </Badge>
                </div>
                <p className="mt-0.5 text-xs text-text-muted truncate">
                  {verdict.reasons.join(" · ")}
                </p>
              </div>
              <div className="ml-3 flex shrink-0 items-center gap-2">
                {verdict.kind === "Eligible" ? (
                  <>
                    <Badge
                      variant="outline"
                      className="bg-success/15 text-success border-success/30"
                    >
                      <CheckCircle2 className="mr-1 h-3 w-3" />
                      Approve
                    </Badge>
                    <Button
                      variant="outline"
                      size="sm"
                      className="border-success/40 text-success hover:bg-success/10"
                      onClick={() => {
                        void onApprove(review.pr);
                      }}
                    >
                      Approve
                    </Button>
                  </>
                ) : (
                  <Badge
                    variant="outline"
                    className="bg-warning/15 text-warning border-warning/30"
                  >
                    <XCircle className="mr-1 h-3 w-3" />
                    Blocked
                  </Badge>
                )}
              </div>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  );
}
