import type { GateState } from "../bindings/GateState";

import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

/** Fail the build if a new `GateState` variant is added without a case here. */
function assertNever(x: never): never {
  throw new Error(`unreachable gate state: ${String(x)}`);
}

/** Human-readable label for a gate state. */
export function gateStateLabel(state: GateState): string {
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

/**
 * Tinted background + text + border classes keyed off the `--color-state-*`
 * tokens. Exported for callers that compose their own badge (e.g. one that
 * pairs the gate tone with a leading icon) while still sharing the palette.
 */
export function gateToneClass(state: GateState): string {
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

interface GatePillProps {
  /** The gate state to render. */
  readonly state: GateState;
  /** Extra classes appended to the pill (e.g. layout shrink hints). */
  readonly className?: string;
}

/**
 * The single, shared gate-state pill used across the review, diff, plan, and
 * command surfaces. Colors come from the `--color-state-*` design tokens so all
 * gate pills read identically. The `switch`es are exhaustive via `assertNever`.
 */
export function GatePill({ state, className }: GatePillProps) {
  return (
    <Badge
      variant="outline"
      className={cn("shrink-0", gateToneClass(state), className)}
    >
      {gateStateLabel(state)}
    </Badge>
  );
}
