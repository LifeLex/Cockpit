/**
 * Tabbed workspace that wraps the full review experience.
 *
 * Provides four tabs: Diff (default), CI, Agent, and Shell. The Diff tab
 * renders the existing DiffView component (which also surfaces PR/issue/repo
 * links and the stack strip in its header). The CI tab shows the PR's check
 * pipelines and failed-run logs. The Agent tab shows the AgentPanel in full
 * standalone mode. The Shell tab provides an embedded terminal rooted at the
 * review's worktree.
 */

import { useState, useMemo, useCallback, useEffect, useRef } from "react";
import { useAppStore } from "../store";
import { useKeyboardShortcuts } from "../hooks/useKeyboardShortcuts";
import type { ShortcutMap } from "../hooks/useKeyboardShortcuts";
import type { Review } from "../bindings/Review";
import type { DiffData } from "../bindings/DiffData";
import type { DiffSide } from "../bindings/DiffSide";
import type { MirrorResult } from "../bindings/MirrorResult";
import { DiffView } from "./DiffView";
import { CiPanel } from "./CiPanel";
import { AgentPanel } from "./AgentPanel";
import { Terminal } from "./Terminal";
import { Loader2 } from "lucide-react";
import { cn } from "@/lib/utils";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** The workspace tabs. */
type WorkspaceTab = "diff" | "ci" | "agent" | "shell";

interface ReviewWorkspaceProps {
  readonly review: Review;
  readonly diff: DiffData;
  readonly onBack: () => void;
  readonly onAddComment: (
    file: string,
    lineStart: number,
    lineEnd: number,
    body: string,
    side: DiffSide,
  ) => Promise<void>;
  readonly onRequestChanges: () => Promise<void>;
  readonly onMirrorComments: () => Promise<MirrorResult | null>;
}

// ---------------------------------------------------------------------------
// Tab bar
// ---------------------------------------------------------------------------

const TABS: readonly { readonly key: WorkspaceTab; readonly label: string }[] = [
  { key: "diff", label: "Diff" },
  { key: "ci", label: "CI" },
  { key: "agent", label: "Agent" },
  { key: "shell", label: "Shell" },
];

function WorkspaceTabBar({
  active,
  onSelect,
}: {
  readonly active: WorkspaceTab;
  readonly onSelect: (tab: WorkspaceTab) => void;
}) {
  // Styling mirrors the `ui/Tabs` `line` variant so in-content tabs read the
  // same across the app: a transparent bar with a baseline border and an
  // underline (via the `after` pseudo-element) on the active tab.
  return (
    <div className="flex shrink-0 items-center gap-1 border-b border-border bg-card px-2">
      {TABS.map((tab) => {
        const isActive = active === tab.key;
        return (
          <button
            key={tab.key}
            type="button"
            aria-selected={isActive}
            onClick={() => {
              onSelect(tab.key);
            }}
            className={cn(
              "relative cursor-pointer border-none bg-transparent px-3 py-2 text-sm font-medium transition-colors",
              "after:absolute after:inset-x-0 after:bottom-[-1px] after:h-0.5 after:bg-foreground after:opacity-0 after:transition-opacity",
              "focus-visible:outline-1 focus-visible:outline-ring",
              isActive
                ? "text-foreground after:opacity-100"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            {tab.label}
          </button>
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------------------
// ReviewWorkspace
// ---------------------------------------------------------------------------

export function ReviewWorkspace({
  review,
  diff,
  onBack,
  onAddComment,
  onRequestChanges,
  onMirrorComments,
}: ReviewWorkspaceProps) {
  const [activeTab, setActiveTab] = useState<WorkspaceTab>("diff");
  const error = useAppStore((s) => s.error);
  const clearError = useAppStore((s) => s.clearError);
  const ensureReviewWorktree = useAppStore((s) => s.ensureReviewWorktree);

  const toggleAgentTab = useCallback(() => {
    setActiveTab((prev) => (prev === "agent" ? "diff" : "agent"));
  }, []);

  const openAgentTab = useCallback(() => {
    setActiveTab("agent");
  }, []);

  const switchToShellTab = useCallback(() => {
    setActiveTab("shell");
  }, []);

  // Auto-switch to Agent tab when a review transitions to Dispatched.
  const gateState = review.gate_state;
  const prevGateRef = useRef(gateState);
  useEffect(() => {
    if (prevGateRef.current !== "Dispatched" && gateState === "Dispatched") {
      setActiveTab("agent");
    }
    prevGateRef.current = gateState;
  }, [gateState]);

  const workspaceShortcuts: ShortcutMap = useMemo(
    () => ({
      "meta+j": toggleAgentTab,
      "meta+t": switchToShellTab,
    }),
    [toggleAgentTab, switchToShellTab],
  );

  useKeyboardShortcuts(workspaceShortcuts);

  // Track which tabs have been activated at least once so we can keep
  // them mounted in the DOM (hidden via CSS) after first visit. This
  // prevents Monaco reinitialisation and terminal session loss on
  // tab switch.
  const [mountedTabs, setMountedTabs] = useState<Set<WorkspaceTab>>(
    () => new Set<WorkspaceTab>(["diff", "agent"]),
  );

  useEffect(() => {
    setMountedTabs((prev) => {
      if (prev.has(activeTab)) return prev;
      const next = new Set(prev);
      next.add(activeTab);
      return next;
    });
  }, [activeTab]);

  // Derive a stable session ID for the shell from the PR ref so it
  // persists across tab switches within the same review.
  const shellSessionId = useMemo(
    () => `shell-${review.pr.replace(/[^a-zA-Z0-9]/g, "-")}`,
    [review.pr],
  );

  // -- Shell worktree materialization --
  // The Shell tab must root the terminal at a real checked-out worktree. For an
  // imported same-repo PR that means checking the branch out on first use, which
  // can take a moment (and may clone), so we resolve the cwd lazily when the tab
  // is first opened for a review and show a preparing state until it lands. A
  // failure falls back to the review's recorded worktree (store surfaces error).
  const [shellCwd, setShellCwd] = useState<string | null>(null);
  const preparedShellPrRef = useRef<string | null>(null);
  useEffect(() => {
    if (activeTab !== "shell") return;
    if (preparedShellPrRef.current === review.pr) return;
    preparedShellPrRef.current = review.pr;
    setShellCwd(null);
    let cancelled = false;
    void (async () => {
      const path = await ensureReviewWorktree(review.pr);
      if (cancelled) return;
      setShellCwd(path ?? review.worktree);
    })();
    return () => {
      cancelled = true;
    };
  }, [activeTab, review.pr, review.worktree, ensureReviewWorktree]);

  return (
    <div className="flex h-full flex-col">
      <WorkspaceTabBar active={activeTab} onSelect={setActiveTab} />
      {error !== null && (
        <div className="flex items-center justify-between border-b border-danger bg-danger/10 px-4 py-2 text-xs text-danger">
          <span>{error}</span>
          <button
            type="button"
            onClick={clearError}
            className="cursor-pointer border-none bg-transparent text-danger underline hover:no-underline"
          >
            Dismiss
          </button>
        </div>
      )}
      <div className="flex min-h-0 flex-1 flex-col">
        {mountedTabs.has("diff") && (
          <div className={cn("flex min-h-0 flex-1 flex-col", activeTab !== "diff" && "hidden")}>
            <DiffView
              review={review}
              diff={diff}
              onBack={onBack}
              onAddComment={onAddComment}
              onRequestChanges={onRequestChanges}
              onMirrorComments={onMirrorComments}
              onOpenAgent={openAgentTab}
            />
          </div>
        )}
        {mountedTabs.has("ci") && (
          <div className={cn("flex min-h-0 flex-1 flex-col", activeTab !== "ci" && "hidden")}>
            <CiPanel pr={review.pr} active={activeTab === "ci"} />
          </div>
        )}
        {mountedTabs.has("agent") && (
          <div className={cn("flex min-h-0 flex-1 flex-col", activeTab !== "agent" && "hidden")}>
            <AgentPanel
              visible
              objectId={review.pr}
              onClose={() => { setActiveTab("diff"); }}
            />
          </div>
        )}
        {mountedTabs.has("shell") && (
          <div className={cn("flex min-h-0 flex-1 flex-col", activeTab !== "shell" && "hidden")}>
            <div className="flex-1 min-h-0">
              {shellCwd !== null ? (
                <Terminal id={shellSessionId} cwd={shellCwd} />
              ) : (
                <div className="flex h-full items-center justify-center gap-2 text-xs text-muted-foreground">
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  Preparing worktree…
                </div>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
