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
import type { Review } from "../bindings/Review";
import { invoke } from "@tauri-apps/api/core";
import { comboFor, Kbd } from "@/lib/shortcuts";
import type { ShortcutId } from "@/lib/shortcuts";
import {
  ListChecks,
  FolderKanban,
  Sparkles,
  Bot,
  Settings,
  RefreshCw,
  PanelBottom,
  Code2,
} from "lucide-react";

interface CommandPaletteProps {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly onToggleSidebar: () => void;
}

/** Render the registry combo for a shortcut id as a command-row hint. */
function ShortcutHint({ id }: { readonly id: ShortcutId }) {
  const combo = comboFor(id);
  if (combo === undefined) return null;
  return (
    <CommandShortcut>
      <Kbd combo={combo} />
    </CommandShortcut>
  );
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
 *
 * Shortcut hints are read from the single shortcut registry so they never
 * drift from the actual key bindings.
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
  const navigateToPrs = useAppStore((s) => s.navigateToPrs);
  const navigateToProjects = useAppStore((s) => s.navigateToProjects);
  const navigateToSkills = useAppStore((s) => s.navigateToSkills);
  const navigateToAgents = useAppStore((s) => s.navigateToAgents);
  const navigateToSettings = useAppStore((s) => s.navigateToSettings);
  const navigateToDiff = useAppStore((s) => s.navigateToDiff);
  const fetchReviews = useAppStore((s) => s.fetchReviews);
  const fetchFrontier = useAppStore((s) => s.fetchFrontier);
  const fetchAuthoredPrs = useAppStore((s) => s.fetchAuthoredPrs);

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
    const merged: Review[] = [];
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

  return (
    <CommandDialog open={open} onOpenChange={onOpenChange}>
      <Command>
        <CommandInput placeholder="Type a command or search..." />
        <CommandList>
          <CommandEmpty>No results found.</CommandEmpty>

          <CommandGroup heading="Navigation">
            <CommandItem
              onSelect={() => {
                handleSelect(navigateToPrs);
              }}
            >
              <ListChecks className="h-4 w-4 shrink-0" />
              <span>PRs</span>
              <ShortcutHint id="nav-prs" />
            </CommandItem>
            <CommandItem
              onSelect={() => {
                handleSelect(navigateToProjects);
              }}
            >
              <FolderKanban className="h-4 w-4 shrink-0" />
              <span>Projects</span>
              <ShortcutHint id="nav-projects" />
            </CommandItem>
            <CommandItem
              onSelect={() => {
                handleSelect(navigateToSkills);
              }}
            >
              <Sparkles className="h-4 w-4 shrink-0" />
              <span>Skills</span>
              <ShortcutHint id="nav-skills" />
            </CommandItem>
            <CommandItem
              onSelect={() => {
                handleSelect(navigateToAgents);
              }}
            >
              <Bot className="h-4 w-4 shrink-0" />
              <span>Agents</span>
              <ShortcutHint id="nav-agents" />
            </CommandItem>
            <CommandItem
              onSelect={() => {
                handleSelect(navigateToSettings);
              }}
            >
              <Settings className="h-4 w-4 shrink-0" />
              <span>Settings</span>
              <ShortcutHint id="nav-settings" />
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
              <ShortcutHint id="refresh" />
            </CommandItem>

            <CommandItem
              onSelect={() => {
                handleSelect(onToggleSidebar);
              }}
            >
              <PanelBottom className="h-4 w-4 shrink-0" />
              <span>Toggle Sidebar</span>
              <ShortcutHint id="toggle-sidebar" />
            </CommandItem>

            {isInDiffView && activeReview !== null && (
              <CommandItem
                onSelect={() => {
                  handleSelect(() => {
                    void invoke("open_in_editor", {
                      filePath: ".",
                      repoSlug: activeReview.repo_slug,
                      branch: activeReview.branch,
                    });
                  });
                }}
              >
                <Code2 className="h-4 w-4 shrink-0" />
                <span>Open in IDE</span>
                <ShortcutHint id="open-in-ide" />
              </CommandItem>
            )}
          </CommandGroup>
        </CommandList>
      </Command>
    </CommandDialog>
  );
}
