import { useCallback } from "react";
import {
  Command,
  CommandDialog,
  CommandInput,
  CommandList,
  CommandEmpty,
  CommandGroup,
  CommandItem,
  CommandShortcut,
} from "@/components/ui/command";
import { useAppStore } from "../store";
import type { GateState } from "../bindings/GateState";
import {
  ListChecks,
  FileText,
  Settings,
  RefreshCw,
  MessageSquare,
  Terminal,
  PanelBottom,
} from "lucide-react";

interface CommandPaletteProps {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly onToggleSidebar: () => void;
}

/** Human-readable label for a gate state, used in the "Jump to Review" group. */
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
    default: {
      const _exhaustive: never = state;
      return String(_exhaustive);
    }
  }
}

/** Tailwind badge color classes for gate states. */
function gateStateBadgeClass(state: GateState): string {
  switch (state) {
    case "Pending":
      return "bg-muted text-muted-foreground";
    case "InReview":
      return "bg-blue-500/15 text-blue-600";
    case "Dispatched":
      return "bg-yellow-500/15 text-yellow-600";
    case "Reworked":
      return "bg-green-500/15 text-green-600";
    case "Approved":
      return "bg-emerald-500/15 text-emerald-600";
    default: {
      const _exhaustive: never = state;
      void _exhaustive;
      return "bg-muted text-muted-foreground";
    }
  }
}

/**
 * Global command palette wired to the existing Command UI primitive.
 *
 * Groups:
 * - Navigation: move between the main app views
 * - Jump to Review: lists all current reviews with branch + state badge
 * - Actions: context-aware actions based on the current view
 */
export function CommandPalette({
  open,
  onOpenChange,
  onToggleSidebar,
}: CommandPaletteProps) {
  const reviews = useAppStore((s) => s.reviews);
  const authoredPrs = useAppStore((s) => s.authoredPrs);
  const reviewRequests = useAppStore((s) => s.reviewRequests);
  const view = useAppStore((s) => s.view);
  const activeReview = useAppStore((s) => s.activeReview);
  const navigateToFrontier = useAppStore((s) => s.navigateToFrontier);
  const navigateToPlan = useAppStore((s) => s.navigateToPlan);
  const navigateToSettings = useAppStore((s) => s.navigateToSettings);
  const navigateToDiff = useAppStore((s) => s.navigateToDiff);
  const fetchReviews = useAppStore((s) => s.fetchReviews);
  const fetchFrontier = useAppStore((s) => s.fetchFrontier);
  const fetchAuthoredPrs = useAppStore((s) => s.fetchAuthoredPrs);
  const requestChanges = useAppStore((s) => s.requestChanges);

  const close = useCallback(() => {
    onOpenChange(false);
  }, [onOpenChange]);

  const handleSelect = useCallback(
    (action: () => void) => {
      action();
      close();
    },
    [close],
  );

  // Merge all reviews from the three sources, deduplicating by PR ref.
  const allReviews = (() => {
    const seen = new Set<string>();
    const merged: typeof reviews extends readonly (infer T)[] ? T[] : never[] = [];
    for (const list of [reviews, authoredPrs, reviewRequests]) {
      for (const r of list) {
        if (!seen.has(r.pr)) {
          seen.add(r.pr);
          merged.push(r);
        }
      }
    }
    return merged;
  })();

  const isInDiffView = view.kind === "diff";
  const isInReviewState =
    isInDiffView && activeReview !== null && activeReview.gate_state === "InReview";

  return (
    <CommandDialog open={open} onOpenChange={onOpenChange}>
      <Command>
        <CommandInput placeholder="Type a command or search..." />
        <CommandList>
          <CommandEmpty>No results found.</CommandEmpty>

          <CommandGroup heading="Navigation">
            <CommandItem
              onSelect={() => {
                handleSelect(navigateToFrontier);
              }}
            >
              <ListChecks className="h-4 w-4 shrink-0" />
              <span>Reviews</span>
              <CommandShortcut>&#8984;1</CommandShortcut>
            </CommandItem>
            <CommandItem
              onSelect={() => {
                handleSelect(navigateToPlan);
              }}
            >
              <FileText className="h-4 w-4 shrink-0" />
              <span>Plan</span>
              <CommandShortcut>&#8984;2</CommandShortcut>
            </CommandItem>
            <CommandItem
              onSelect={() => {
                handleSelect(navigateToSettings);
              }}
            >
              <Settings className="h-4 w-4 shrink-0" />
              <span>Settings</span>
              <CommandShortcut>&#8984;,</CommandShortcut>
            </CommandItem>
          </CommandGroup>

          {allReviews.length > 0 && (
            <CommandGroup heading="Jump to Review">
              {allReviews.map((review) => (
                <CommandItem
                  key={review.pr}
                  onSelect={() => {
                    handleSelect(() => {
                      void navigateToDiff(review.pr);
                    });
                  }}
                >
                  <span className="flex-1 truncate">{review.branch}</span>
                  <span
                    className={`ml-2 inline-flex rounded-full px-2 py-0.5 text-xs font-medium ${gateStateBadgeClass(review.gate_state)}`}
                  >
                    {gateStateLabel(review.gate_state)}
                  </span>
                </CommandItem>
              ))}
            </CommandGroup>
          )}

          <CommandGroup heading="Actions">
            <CommandItem
              onSelect={() => {
                handleSelect(() => {
                  void fetchReviews();
                  void fetchFrontier();
                  void fetchAuthoredPrs();
                });
              }}
            >
              <RefreshCw className="h-4 w-4 shrink-0" />
              <span>Refresh PRs</span>
              <CommandShortcut>&#8984;R</CommandShortcut>
            </CommandItem>

            {isInReviewState && (
              <CommandItem
                onSelect={() => {
                  handleSelect(() => {
                    void requestChanges();
                  });
                }}
              >
                <MessageSquare className="h-4 w-4 shrink-0" />
                <span>Request Changes</span>
                <CommandShortcut>&#8984;&#8679;R</CommandShortcut>
              </CommandItem>
            )}

            <CommandItem
              onSelect={() => {
                handleSelect(onToggleSidebar);
              }}
            >
              <PanelBottom className="h-4 w-4 shrink-0" />
              <span>Toggle Sidebar</span>
              <CommandShortcut>&#8984;B</CommandShortcut>
            </CommandItem>

            <CommandItem
              onSelect={() => {
                handleSelect(() => {
                  // Open a terminal shell -- placeholder for future integration.
                  // Currently no-op; the shortcut is registered for discoverability.
                });
              }}
            >
              <Terminal className="h-4 w-4 shrink-0" />
              <span>Open Shell</span>
              <CommandShortcut>&#8984;T</CommandShortcut>
            </CommandItem>
          </CommandGroup>
        </CommandList>
      </Command>
    </CommandDialog>
  );
}
