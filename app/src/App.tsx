import { useState, useEffect, useCallback, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { sendNotification } from "@tauri-apps/plugin-notification";
import { useAppStore } from "./store";
import type { ViewState } from "./store";
import { useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import type { ShortcutMap } from "./hooks/useKeyboardShortcuts";
import { SHORTCUTS } from "./lib/shortcuts";
import type { ShortcutId } from "./lib/shortcuts";
import { Sidebar } from "./components/Sidebar";
import { ReviewCard } from "./components/ReviewCard";
import { StackContainer } from "./components/StackContainer";
import { FastLaneShelf } from "./components/FastLaneShelf";
import { ProjectCard } from "./components/ProjectCard";
import { ReviewWorkspace } from "./components/ReviewWorkspace";
import { PlanView } from "./components/PlanView";
import { NewProjectView } from "./components/NewProjectView";
import { SkillsView } from "./components/SkillsView";
import { AgentEditor } from "./components/AgentEditor";
import { SettingsView } from "./components/SettingsView";
import { CommandPalette } from "./components/CommandPalette";
import { SkeletonList } from "./components/SkeletonCard";
import { EmptyState } from "./components/EmptyState";
import { StateFilter } from "./components/StateFilter";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { LucideIcon } from "lucide-react";
import {
  Search,
  LayoutGrid,
  Rows3,
  FileText,
  Eye,
  Rocket,
  FolderOpen,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { buildBoardItems } from "./lib/stack-tree";
import { isFastLane } from "./lib/attention";
import type { CardDensity } from "./components/ReviewCard";
import type { GateState } from "./bindings/GateState";
import type { Review } from "./bindings/Review";
import type { Project } from "./bindings/Project";
import type { AgentMode } from "./bindings/AgentMode";

/** Payload emitted by the Tauri backend on "agent-completed" events. */
interface CompletionEventPayload {
  readonly session_id: string;
  readonly object_id: string;
  readonly mode: AgentMode;
  /**
   * git-HEAD-authoritative outcome of the completed run: `"reworked"` when a
   * commit landed, `"failed"` for a no-op/failed run, `"completed"` for a
   * non-gate-advancing artifact fill. Optional — hand-typed, tolerant of older
   * payloads.
   */
  readonly outcome?: "reworked" | "failed" | "completed";
}

type ReviewTab = "my-prs" | "review-requests" | "all";

const SIDEBAR_COLLAPSED_KEY = "cockpit-sidebar-collapsed";
const PRS_DENSITY_KEY = "cockpit-prs-density";

/** Read the persisted board density, defaulting to the roomy card layout. */
function loadDensity(): CardDensity {
  return localStorage.getItem(PRS_DENSITY_KEY) === "compact"
    ? "compact"
    : "cards";
}

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** The git-HEAD-authoritative outcome label carried on a completion event. */
type CompletionOutcome = CompletionEventPayload["outcome"];

/**
 * Build an outcome-aware desktop notification for a completed agent run.
 *
 * The diff-gate rework loop (Fix/Restack) distinguishes a run that actually
 * landed a commit (`"reworked"` — ready to re-review) from a no-op/failed run
 * (`"failed"` — comments are preserved for another cycle), since those demand
 * very different follow-up from the reviewer.
 */
function completionNotification(
  mode: AgentMode,
  outcome: CompletionOutcome,
  branch: string,
): { readonly title: string; readonly body: string } {
  switch (mode) {
    case "Fix":
    case "Restack":
      return outcome === "failed"
        ? {
            title: "Agent Failed",
            body: `Agent failed on ${branch} — comments preserved`,
          }
        : {
            title: "Rework Complete",
            body: `PR reworked on ${branch} — ready to re-review`,
          };
    case "Implement":
      return {
        title: "Implementation Complete",
        body: `Implementation agent finished on ${branch}`,
      };
    case "Plan":
      return {
        title: "Plan Rework Complete",
        body: "Plan agent finished reworking",
      };
    case "Review":
      return {
        title: "Pre-review Complete",
        body: `Pre-review finished on ${branch}`,
      };
    default:
      return assertNever(mode);
  }
}

/** Whether a mode's completion is keyed by a review's PR ref (not a project). */
function isReviewMode(mode: AgentMode): boolean {
  return (
    mode === "Fix" ||
    mode === "Restack" ||
    mode === "Implement" ||
    mode === "Review"
  );
}

/**
 * Whether a review-mode completion was already reconciled locally before the
 * event arrived — i.e. the user stopped the agent (`killAgent` cleared the
 * review's `agent` handle) or a duplicate stream-end already settled it.
 *
 * The frontend store is only refreshed asynchronously, so a *genuine*
 * completion still shows the agent attached at event time; a stopped one shows
 * it already cleared. Used to suppress a double-toast after a user Stop.
 */
function alreadySettledLocally(objectId: string): boolean {
  const s = useAppStore.getState();
  const review =
    [...s.reviews, ...s.authoredPrs, ...s.reviewRequests].find(
      (r) => r.pr === objectId,
    ) ?? (s.activeReview?.pr === objectId ? s.activeReview : null);
  return review !== null && review !== undefined && review.agent === null;
}

/** A named group of reviews for the grouped-by-project PRs list. */
interface ReviewGroup {
  readonly key: string;
  readonly title: string;
  readonly reviews: readonly Review[];
}

/**
 * Group reviews by their project, preserving project order from `projects`
 * and collecting reviews with no project into a trailing "Ungrouped" section.
 * Empty groups are omitted.
 */
function groupReviewsByProject(
  reviews: readonly Review[],
  projects: readonly Project[],
): readonly ReviewGroup[] {
  const groups: ReviewGroup[] = [];
  for (const project of projects) {
    const members = reviews.filter((r) => r.project === project.id);
    if (members.length > 0) {
      groups.push({ key: project.id, title: project.name, reviews: members });
    }
  }
  const ungrouped = reviews.filter((r) => r.project === null);
  if (ungrouped.length > 0) {
    groups.push({ key: "__ungrouped__", title: "Ungrouped", reviews: ungrouped });
  }
  return groups;
}

function App() {
  const reviews = useAppStore((s) => s.reviews);
  const plan = useAppStore((s) => s.plan);
  const loading = useAppStore((s) => s.loading);
  const error = useAppStore((s) => s.error);
  const view = useAppStore((s) => s.view);
  const activeReview = useAppStore((s) => s.activeReview);
  const activeDiff = useAppStore((s) => s.activeDiff);
  const authoredPrs = useAppStore((s) => s.authoredPrs);
  const reviewRequests = useAppStore((s) => s.reviewRequests);
  const projects = useAppStore((s) => s.projects);
  const prFetchLoading = useAppStore((s) => s.prFetchLoading);
  const fetchAuthoredPrs = useAppStore((s) => s.fetchAuthoredPrs);
  const fetchReviewRequests = useAppStore((s) => s.fetchReviewRequests);
  const fetchReviews = useAppStore((s) => s.fetchReviews);
  const fetchPlan = useAppStore((s) => s.fetchPlan);
  const fetchConfig = useAppStore((s) => s.fetchConfig);
  const listProjects = useAppStore((s) => s.listProjects);

  const [reviewTab, setReviewTab] = useState<ReviewTab>("my-prs");
  const [stateFilter, setStateFilter] = useState<GateState | null>(null);
  const [showStale, setShowStale] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const openReview = useAppStore((s) => s.openReview);
  const navigateToDiff = useAppStore((s) => s.navigateToDiff);
  const navigateToPlan = useAppStore((s) => s.navigateToPlan);
  const navigateToPrs = useAppStore((s) => s.navigateToPrs);
  const navigateToProjects = useAppStore((s) => s.navigateToProjects);
  const navigateToNewProject = useAppStore((s) => s.navigateToNewProject);
  const navigateToSkills = useAppStore((s) => s.navigateToSkills);
  const navigateToAgents = useAppStore((s) => s.navigateToAgents);
  const navigateToSettings = useAppStore((s) => s.navigateToSettings);
  const addComment = useAppStore((s) => s.addComment);
  const requestChanges = useAppStore((s) => s.requestChanges);
  const mirrorComments = useAppStore((s) => s.mirrorComments);
  const refreshActiveReview = useAppStore((s) => s.refreshActiveReview);
  const addPlanComment = useAppStore((s) => s.addPlanComment);
  const planRequestChanges = useAppStore((s) => s.planRequestChanges);
  const planApprove = useAppStore((s) => s.planApprove);
  const planOpen = useAppStore((s) => s.planOpen);
  const generatePlan = useAppStore((s) => s.generatePlan);
  const batchStatus = useAppStore((s) => s.batchStatus);
  const restackPr = useAppStore((s) => s.restackPr);

  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => {
    return localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "true";
  });

  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);

  const [prsDensity, setPrsDensity] = useState<CardDensity>(loadDensity);

  const setDensity = useCallback((density: CardDensity) => {
    setPrsDensity(density);
    localStorage.setItem(PRS_DENSITY_KEY, density);
  }, []);

  const toggleSidebar = useCallback(() => {
    setSidebarCollapsed((prev) => {
      const next = !prev;
      localStorage.setItem(SIDEBAR_COLLAPSED_KEY, String(next));
      return next;
    });
  }, []);

  // Handlers keyed by shortcut id; the registry supplies the key bindings so
  // there is exactly one source of truth for both the combo and the handler.
  const shortcutHandlers = useMemo<Readonly<Record<ShortcutId, () => void>>>(
    () => ({
      "command-palette": () => {
        setCommandPaletteOpen(true);
      },
      "nav-prs": () => {
        navigateToPrs();
      },
      "nav-projects": () => {
        navigateToProjects();
      },
      "nav-skills": () => {
        navigateToSkills();
      },
      "nav-agents": () => {
        navigateToAgents();
      },
      "nav-settings": () => {
        navigateToSettings();
      },
      refresh: () => {
        void fetchReviews();
        void fetchAuthoredPrs();
      },
      "toggle-sidebar": () => {
        toggleSidebar();
      },
      "open-in-ide": () => {
        if (view.kind === "diff" && activeReview !== null) {
          void invoke("open_in_editor", {
            filePath: ".",
            repoSlug: activeReview.repo_slug,
            branch: activeReview.branch,
          });
        }
      },
      escape: () => {
        // Only navigate back when viewing a diff or plan, not from the list.
        if (view.kind === "diff" || view.kind === "plan") {
          navigateToPrs();
        }
      },
    }),
    [
      navigateToPrs,
      navigateToProjects,
      navigateToSkills,
      navigateToAgents,
      navigateToSettings,
      fetchReviews,
      fetchAuthoredPrs,
      toggleSidebar,
      view.kind,
      activeReview,
    ],
  );

  const shortcuts: ShortcutMap = useMemo(() => {
    const map: Record<string, () => void> = {};
    for (const shortcut of SHORTCUTS) {
      map[shortcut.combo] = shortcutHandlers[shortcut.id];
    }
    return map;
  }, [shortcutHandlers]);

  useKeyboardShortcuts(shortcuts);

  useEffect(() => {
    void fetchReviews();
    void fetchConfig();
    void fetchAuthoredPrs();
    void listProjects();

    const unlisten = listen<CompletionEventPayload>("agent-completed", (event) => {
      const { object_id, mode, outcome } = event.payload;

      // Snapshot BEFORE the async refreshes so a genuine completion (agent still
      // attached) is distinguished from one the user already stopped (agent
      // cleared by killAgent). The refreshes below then reconcile local state.
      const settledLocally =
        isReviewMode(mode) && alreadySettledLocally(object_id);

      void fetchReviews();
      void listProjects();
      // Refresh the open plan (if any) so a completed Plan agent updates it.
      const currentView = useAppStore.getState().view;
      if (currentView.kind === "plan") {
        void fetchPlan(currentView.project);
      }
      void refreshActiveReview();

      // Suppress the notification for a run the user already stopped — otherwise
      // the killed process's straggler completion would double-toast.
      if (settledLocally) return;

      // Best-effort desktop notification, outcome-aware so a failed rework reads
      // differently from one that landed a commit.
      const current = useAppStore.getState().activeReview;
      const branch = current !== null ? current.branch : "a review";
      const { title, body } = completionNotification(mode, outcome, branch);
      void sendNotification({ title, body });
    });

    return () => {
      void unlisten.then((f) => {
        f();
      });
    };
  }, [
    fetchReviews,
    fetchPlan,
    fetchConfig,
    fetchAuthoredPrs,
    listProjects,
    refreshActiveReview,
  ]);

  const filterReviews = useCallback(
    (items: readonly Review[]): readonly Review[] => {
      let filtered = items;
      if (searchQuery !== "") {
        const q = searchQuery.toLowerCase();
        filtered = filtered.filter(
          (r) =>
            r.branch.toLowerCase().includes(q) ||
            r.pr.toLowerCase().includes(q) ||
            r.issue.toLowerCase().includes(q) ||
            r.base.toLowerCase().includes(q),
        );
      }
      if (stateFilter !== null) {
        filtered = filtered.filter((r) => r.gate_state === stateFilter);
      }
      if (showStale) {
        filtered = filtered.filter((r) => r.stale);
      }
      return filtered;
    },
    [stateFilter, showStale, searchQuery],
  );

  const reviewsForTab: readonly Review[] = useMemo(() => {
    switch (reviewTab) {
      case "my-prs":
        return authoredPrs;
      case "review-requests":
        return reviewRequests;
      case "all":
        return reviews;
      default:
        return assertNever(reviewTab);
    }
  }, [reviewTab, authoredPrs, reviewRequests, reviews]);

  const handleReviewAction = useCallback(
    (pr: string) => {
      const allReviews = [...authoredPrs, ...reviewRequests, ...reviews];
      const review = allReviews.find((r) => r.pr === pr);
      if (review === undefined) return;

      switch (review.gate_state) {
        case "Pending":
        case "Reworked":
          void openReview(pr);
          break;
        case "InReview":
        case "Dispatched":
        case "Approved":
        case "Merged":
          void navigateToDiff(pr);
          break;
        default:
          assertNever(review.gate_state);
      }
    },
    [openReview, navigateToDiff, authoredPrs, reviewRequests, reviews],
  );

  const handleRestack = useCallback(
    (pr: string) => {
      void restackPr(pr);
    },
    [restackPr],
  );

  // Opening a project always routes to its plan gate. When the project has no
  // plan yet, PlanView surfaces a "Generate plan" affordance.
  const handleOpenProject = useCallback(
    (project: Project) => {
      navigateToPlan(project.id);
    },
    [navigateToPlan],
  );

  const handleNavigate = useCallback(
    (kind: ViewState["kind"]) => {
      switch (kind) {
        case "prs":
          navigateToPrs();
          break;
        case "projects":
          navigateToProjects();
          break;
        case "new-project":
          navigateToNewProject();
          break;
        case "skills":
          navigateToSkills();
          break;
        case "agents":
          navigateToAgents();
          break;
        case "settings":
          navigateToSettings();
          break;
        // "plan" and "diff" are drill-in views that require an id, so they are
        // not reachable from the generic (id-less) sidebar navigation.
        case "plan":
        case "diff":
          break;
        default:
          assertNever(kind);
      }
    },
    [
      navigateToPrs,
      navigateToProjects,
      navigateToNewProject,
      navigateToSkills,
      navigateToAgents,
      navigateToSettings,
    ],
  );

  const handleTabChange = useCallback(
    (value: unknown) => {
      if (value === null || typeof value !== "string") return;
      let tab: ReviewTab;
      switch (value) {
        case "my-prs":
        case "review-requests":
        case "all":
          tab = value;
          break;
        default:
          return;
      }
      setReviewTab(tab);
      setStateFilter(null);
      setShowStale(false);
      setSearchQuery("");
      if (tab === "my-prs") void fetchAuthoredPrs();
      if (tab === "review-requests") void fetchReviewRequests();
      if (tab === "all") void fetchReviews();
    },
    [fetchAuthoredPrs, fetchReviewRequests, fetchReviews],
  );

  const errorBanner =
    error !== null ? (
      <div className="mb-4 rounded-lg border border-danger bg-danger/10 px-4 py-3 text-sm text-danger">
        {error}
      </div>
    ) : null;

  function renderProjectGroupedList(items: readonly Review[]) {
    const filtered = filterReviews(items);
    // Fast lane filters AFTER search/state filters, then its members are
    // excluded from the project groups below so nothing renders twice.
    const fastLane = filtered.filter(isFastLane);
    const fastLaneIds = new Set(fastLane.map((r) => r.id));
    const rest = filtered.filter((r) => !fastLaneIds.has(r.id));
    const groups = groupReviewsByProject(rest, projects);

    if (fastLane.length === 0 && groups.length === 0) {
      return null;
    }

    const projectSections = groups.map((group) => (
      <section key={group.key} className="mb-6">
        <h2 className="mb-3 font-display text-sm font-semibold uppercase tracking-wide text-muted-foreground">
          {group.title}{" "}
          <span className="ml-1 text-xs font-normal">
            ({group.reviews.length})
          </span>
        </h2>
        <div className={prsDensity === "compact" ? "space-y-1.5" : "space-y-3"}>
          {buildBoardItems(group.reviews).map((item) =>
            item.kind === "single" ? (
              <ReviewCard
                key={item.review.id}
                review={item.review}
                density={prsDensity}
                onAction={handleReviewAction}
                onRestack={handleRestack}
              />
            ) : (
              <StackContainer
                key={item.root.review.id}
                root={item.root}
                nodes={item.nodes}
                density={prsDensity}
                onAction={handleReviewAction}
                onRestack={handleRestack}
              />
            ),
          )}
        </div>
      </section>
    ));

    return (
      <>
        {fastLane.length > 0 && (
          <FastLaneShelf
            reviews={fastLane}
            density={prsDensity}
            onAction={handleReviewAction}
            onRestack={handleRestack}
          />
        )}
        {projectSections}
      </>
    );
  }

  function renderPrsContent(items: readonly Review[], emptyIcon: LucideIcon, emptyTitle: string, emptyDescription: string) {
    if (prFetchLoading && items.length === 0) {
      return <SkeletonList count={4} />;
    }
    const grouped = renderProjectGroupedList(items);
    if (grouped === null) {
      return (
        <EmptyState
          icon={emptyIcon}
          title={emptyTitle}
          description={emptyDescription}
        />
      );
    }
    return grouped;
  }

  function renderContent() {
    switch (view.kind) {
      case "prs":
        return (
          <div className="mx-auto max-w-4xl px-6 py-8">
            {errorBanner}

            <Tabs value={reviewTab} onValueChange={handleTabChange}>
              <div className="flex items-center mb-6">
                <TabsList variant="line">
                  <TabsTrigger value="my-prs">
                    Mine
                    {authoredPrs.length > 0 && (
                      <span className="ml-1.5 rounded-full bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
                        {authoredPrs.length}
                      </span>
                    )}
                  </TabsTrigger>
                  <TabsTrigger value="review-requests">
                    Review Requests
                    {reviewRequests.length > 0 && (
                      <span className="ml-1.5 rounded-full bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
                        {reviewRequests.length}
                      </span>
                    )}
                  </TabsTrigger>
                  <TabsTrigger value="all">
                    All
                    {reviews.length > 0 && (
                      <span className="ml-1.5 rounded-full bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
                        {reviews.length}
                      </span>
                    )}
                  </TabsTrigger>
                </TabsList>

                <div className="ml-auto flex items-center gap-2">
                  {(reviewTab === "my-prs" ||
                    reviewTab === "review-requests") && (
                    <Button
                      variant="outline"
                      onClick={() => {
                        if (reviewTab === "my-prs") void fetchAuthoredPrs();
                        else void fetchReviewRequests();
                      }}
                      disabled={prFetchLoading}
                    >
                      {prFetchLoading ? "Fetching..." : "Refresh"}
                    </Button>
                  )}
                </div>
              </div>

              <div className="mb-4 flex items-center gap-3">
                <div className="relative w-52">
                  <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
                  <Input
                    type="text"
                    value={searchQuery}
                    onChange={(e) => {
                      setSearchQuery(e.target.value);
                    }}
                    placeholder="Search PRs..."
                    aria-label="Search PRs"
                    className="pl-8 text-xs"
                  />
                </div>
                <StateFilter
                  reviews={reviewsForTab}
                  activeFilter={stateFilter}
                  showStale={showStale}
                  onFilterChange={setStateFilter}
                  onToggleStale={() => {
                    setShowStale((prev) => !prev);
                  }}
                />

                {/* Density toggle: roomy cards vs. dense telemetry rows. */}
                <div
                  className="ml-auto flex shrink-0 items-center rounded-lg border border-border p-0.5"
                  role="group"
                  aria-label="Board density"
                >
                  <button
                    type="button"
                    onClick={() => {
                      setDensity("cards");
                    }}
                    aria-pressed={prsDensity === "cards"}
                    title="Card view"
                    className={cn(
                      "rounded-md p-1.5 transition-colors",
                      prsDensity === "cards"
                        ? "bg-muted text-foreground"
                        : "text-muted-foreground hover:text-foreground",
                    )}
                  >
                    <LayoutGrid className="h-3.5 w-3.5" />
                  </button>
                  <button
                    type="button"
                    onClick={() => {
                      setDensity("compact");
                    }}
                    aria-pressed={prsDensity === "compact"}
                    title="Compact view"
                    className={cn(
                      "rounded-md p-1.5 transition-colors",
                      prsDensity === "compact"
                        ? "bg-muted text-foreground"
                        : "text-muted-foreground hover:text-foreground",
                    )}
                  >
                    <Rows3 className="h-3.5 w-3.5" />
                  </button>
                </div>
              </div>

              <TabsContent value="my-prs">
                {renderPrsContent(
                  authoredPrs,
                  FileText,
                  "No open PRs",
                  "Click Refresh to fetch your open PRs from GitHub. Make sure your repo path is configured in Settings.",
                )}
              </TabsContent>

              <TabsContent value="review-requests">
                {renderPrsContent(
                  reviewRequests,
                  Eye,
                  "No review requests",
                  "No PRs are waiting for your review. Click Refresh to check again.",
                )}
              </TabsContent>

              <TabsContent value="all">
                {loading && reviews.length === 0 ? (
                  <SkeletonList count={4} />
                ) : (
                  (renderProjectGroupedList(reviews) ?? (
                    <EmptyState
                      icon={Rocket}
                      title="No reviews yet"
                      description="Create a project or import from Linear under Projects, or switch to Mine to review existing GitHub PRs."
                      actionLabel="Go to Projects"
                      onAction={navigateToProjects}
                    />
                  ))
                )}
              </TabsContent>
            </Tabs>
          </div>
        );

      case "diff":
        if (activeReview === null || activeDiff === null) {
          return (
            <div className="px-6 py-8">
              {loading ? (
                <p className="text-sm text-muted-foreground">Loading diff...</p>
              ) : (
                <div className="rounded-lg border border-danger bg-danger/10 px-4 py-3 text-sm text-danger">
                  Failed to load review.{" "}
                  <button
                    onClick={navigateToPrs}
                    className="underline hover:no-underline"
                  >
                    Back
                  </button>
                </div>
              )}
            </div>
          );
        }

        return (
          <ReviewWorkspace
            review={activeReview}
            diff={activeDiff}
            onBack={navigateToPrs}
            onAddComment={addComment}
            onRequestChanges={requestChanges}
            onMirrorComments={mirrorComments}
          />
        );

      case "plan": {
        const projectId = view.project;
        return (
          <div className="mx-auto max-w-4xl px-6 py-8">
            {errorBanner}

            <PlanView
              projectId={projectId}
              plan={plan}
              onGenerate={() => {
                void generatePlan(projectId);
              }}
              onAddComment={(anchor, body) => {
                void addPlanComment(projectId, anchor, body);
              }}
              onRequestChanges={() => {
                void planRequestChanges(projectId);
              }}
              onApprove={() => {
                void planApprove(projectId);
              }}
              onOpen={() => {
                void planOpen(projectId);
              }}
              onFetchBatchStatus={() => batchStatus(projectId)}
            />
          </div>
        );
      }

      case "projects":
        return (
          <div className="mx-auto max-w-4xl px-6 py-8">
            {errorBanner}
            <div className="mb-6 flex items-center justify-between">
              <h1 className="text-lg font-semibold text-foreground">Projects</h1>
              <Button onClick={navigateToNewProject}>New Project</Button>
            </div>
            {projects.length === 0 ? (
              <EmptyState
                icon={FolderOpen}
                title="No projects yet"
                description="Create an ad-hoc project or import one from Linear."
                actionLabel="New Project"
                onAction={navigateToNewProject}
              />
            ) : (
              <div className="space-y-3">
                {projects.map((project) => (
                  <ProjectCard
                    key={project.id}
                    project={project}
                    prCount={
                      reviews.filter((r) => r.project === project.id).length
                    }
                    onOpen={handleOpenProject}
                  />
                ))}
              </div>
            )}
          </div>
        );

      case "new-project":
        return <NewProjectView onDone={navigateToProjects} />;

      case "skills":
        return <SkillsView />;

      case "agents":
        return <AgentEditor />;

      case "settings":
        return <SettingsView />;

      default:
        return assertNever(view);
    }
  }

  return (
    <TooltipProvider>
      <div className="flex h-screen overflow-hidden bg-background text-foreground">
        <Sidebar
          activeView={view.kind}
          reviewCount={reviews.length}
          onNavigate={handleNavigate}
          collapsed={sidebarCollapsed}
          onToggleCollapse={toggleSidebar}
        />
        <main className="flex-1 overflow-y-auto">{renderContent()}</main>
      </div>
      <CommandPalette
        open={commandPaletteOpen}
        onOpenChange={setCommandPaletteOpen}
        onToggleSidebar={toggleSidebar}
      />
    </TooltipProvider>
  );
}

export default App;
