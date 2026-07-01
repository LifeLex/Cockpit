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
import type { MirrorResult } from "../bindings/MirrorResult";
import { DiffView } from "./DiffView";
import { CiPanel } from "./CiPanel";
import { AgentPanel } from "./AgentPanel";
import { Terminal } from "./Terminal";
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
  return (
    <div className="flex shrink-0 items-center gap-0 border-b border-border bg-card px-2">
      {TABS.map((tab) => (
        <button
          key={tab.key}
          type="button"
          onClick={() => {
            onSelect(tab.key);
          }}
          className={cn(
            "cursor-pointer border-x-0 border-t-0 border-b-2 bg-transparent px-4 py-2 text-xs font-medium transition-colors",
            active === tab.key
              ? "border-b-primary text-foreground"
              : "border-b-transparent text-muted-foreground hover:text-foreground hover:border-b-muted-foreground/40",
          )}
        >
          {tab.label}
        </button>
      ))}
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
            <AgentPanel visible onClose={() => { setActiveTab("diff"); }} />
          </div>
        )}
        {mountedTabs.has("shell") && (
          <div className={cn("flex min-h-0 flex-1 flex-col", activeTab !== "shell" && "hidden")}>
            <div className="flex-1 min-h-0">
              <Terminal id={shellSessionId} cwd={review.worktree} />
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
