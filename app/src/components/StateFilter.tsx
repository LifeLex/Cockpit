import { cn } from "@/lib/utils";
import { gateStateLabel } from "./GatePill";
import type { GateState } from "../bindings/GateState";
import type { Review } from "../bindings/Review";

interface StateFilterProps {
  readonly reviews: readonly Review[];
  readonly activeFilter: GateState | null;
  readonly showStale: boolean;
  readonly onFilterChange: (state: GateState | null) => void;
  readonly onToggleStale: () => void;
}

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** All gate states in display order. */
const GATE_STATES = [
  "Pending",
  "InReview",
  "Dispatched",
  "Reworked",
  "Approved",
  "Merged",
] as const satisfies readonly GateState[];

/** Tailwind classes for an active (selected) chip of the given state. */
function activeChipClass(state: GateState): string {
  switch (state) {
    case "Pending":
      return "bg-state-pending text-white";
    case "InReview":
      return "bg-state-in-review text-white";
    case "Dispatched":
      return "bg-state-dispatched text-white";
    case "Reworked":
      return "bg-state-reworked text-white";
    case "Approved":
      return "bg-state-approved text-white";
    // Merged shares the approved tone until it earns its own token.
    case "Merged":
      return "bg-state-approved text-white";
    default:
      return assertNever(state);
  }
}

/** Tailwind classes for an inactive chip of the given state. */
function inactiveChipClass(state: GateState): string {
  switch (state) {
    case "Pending":
      return "bg-state-pending/20 text-state-pending";
    case "InReview":
      return "bg-state-in-review/20 text-state-in-review";
    case "Dispatched":
      return "bg-state-dispatched/20 text-state-dispatched";
    case "Reworked":
      return "bg-state-reworked/20 text-state-reworked";
    case "Approved":
      return "bg-state-approved/20 text-state-approved";
    case "Merged":
      return "bg-state-approved/20 text-state-approved";
    default:
      return assertNever(state);
  }
}

/** Count how many reviews match a given gate state. */
function countByState(
  reviews: readonly Review[],
  state: GateState,
): number {
  return reviews.filter((r) => r.gate_state === state).length;
}

/** Count how many reviews are stale. */
function countStale(reviews: readonly Review[]): number {
  return reviews.filter((r) => r.stale).length;
}

/** A row of filter chips for gate states with a trailing stale toggle. */
export function StateFilter({
  reviews,
  activeFilter,
  showStale,
  onFilterChange,
  onToggleStale,
}: StateFilterProps) {
  const totalCount = reviews.length;
  const staleCount = countStale(reviews);

  return (
    <div className="flex items-center gap-1.5">
      {/* "All" chip */}
      <button
        type="button"
        onClick={() => {
          onFilterChange(null);
        }}
        className={cn(
          "rounded-full px-2.5 py-1 text-xs font-medium transition-colors",
          activeFilter === null
            ? "bg-muted-foreground text-background"
            : "bg-muted text-muted-foreground hover:bg-muted-foreground/20",
        )}
      >
        All ({totalCount})
      </button>

      {/* One chip per gate state */}
      {GATE_STATES.map((state) => {
        const count = countByState(reviews, state);
        const isActive = activeFilter === state;
        return (
          <button
            key={state}
            type="button"
            onClick={() => {
              onFilterChange(state);
            }}
            className={cn(
              "rounded-full px-2.5 py-1 text-xs font-medium transition-colors",
              isActive
                ? activeChipClass(state)
                : inactiveChipClass(state),
              count === 0 && !isActive && "opacity-50",
            )}
          >
            {gateStateLabel(state)} ({count})
          </button>
        );
      })}

      {/* Divider */}
      <div className="mx-1 h-4 w-px bg-border" />

      {/* Stale toggle */}
      <button
        type="button"
        onClick={onToggleStale}
        className={cn(
          "rounded-full px-2.5 py-1 text-xs font-medium transition-colors",
          showStale
            ? "bg-danger text-white"
            : "bg-danger/20 text-danger hover:bg-danger/30",
          staleCount === 0 && !showStale && "opacity-50",
        )}
      >
        Stale ({staleCount})
      </button>
    </div>
  );
}
