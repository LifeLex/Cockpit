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
import { Search } from "lucide-react";
import type { GateState } from "./bindings/GateState";
import type { Review } from "./bindings/Review";
import type { Project } from "./bindings/Project";
import type { AgentMode } from "./bindings/AgentMode";

/** Payload emitted by the Tauri backend on "agent-completed" events. */
interface CompletionEventPayload {
  readonly session_id: string;
  readonly object_id: string;
  readonly mode: AgentMode;
}

type ReviewTab = "my-prs" | "review-requests" | "all";

const SIDEBAR_COLLAPSED_KEY = "cockpit-sidebar-collapsed";

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** Build a desktop notification title from the agent mode. */
function notificationTitleForMode(mode: AgentMode): string {
  switch (mode) {
    case "Fix":
    case "Restack":
      return "Rework Complete";
    case "Plan":
      return "Plan Rework Complete";
    case "Implement":
      return "Implementation Complete";
    default:
      return assertNever(mode);
  }
}

/** Build a desktop notification body from the agent mode and branch name. */
function notificationBodyForMode(mode: AgentMode, branch: string): string {
  switch (mode) {
    case "Fix":
      return `Fix agent finished on ${branch}`;
    case "Restack":
      return `Restack agent finished on ${branch}`;
    case "Plan":
      return `Plan agent finished reworking`;
    case "Implement":
      return `Implementation agent finished on ${branch}`;
    default:
      return assertNever(mode);
  }
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
  const fetchFrontier = useAppStore((s) => s.fetchFrontier);
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
  const openPlan = useAppStore((s) => s.openPlan);
  const batchStatus = useAppStore((s) => s.batchStatus);

  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => {
    return localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "true";
  });

  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);

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
        void fetchFrontier();
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
      fetchFrontier,
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
    void fetchFrontier();
    void fetchPlan();
    void fetchConfig();
    void fetchAuthoredPrs();
    void listProjects();

    const unlisten = listen<CompletionEventPayload>("agent-completed", (event) => {
      void fetchReviews();
      void fetchFrontier();
      void fetchPlan();
      void refreshActiveReview();

      // Best-effort desktop notification. Use the event payload's mode
      // to differentiate the notification title and body.
      const { mode } = event.payload;
      const current = useAppStore.getState().activeReview;
      const branch = current !== null ? current.branch : "a review";
      void sendNotification({
        title: notificationTitleForMode(mode),
        body: notificationBodyForMode(mode, branch),
      });
    });

    return () => {
      void unlisten.then((f) => {
        f();
      });
    };
  }, [
    fetchReviews,
    fetchFrontier,
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
          void navigateToDiff(pr);
          break;
        default:
          assertNever(review.gate_state);
      }
    },
    [openReview, navigateToDiff, authoredPrs, reviewRequests, reviews],
  );

  // Opening a project routes to its plan gate when a plan exists, otherwise to
  // the project-grouped PRs list.
  const handleOpenProject = useCallback(
    (project: Project) => {
      if (project.plan !== null) {
        navigateToPlan();
      } else {
        navigateToPrs();
      }
    },
    [navigateToPlan, navigateToPrs],
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
        case "plan":
          navigateToPlan();
          break;
        case "settings":
          navigateToSettings();
          break;
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
      navigateToPlan,
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
    const groups = groupReviewsByProject(filtered, projects);

    if (groups.length === 0) {
      return null;
    }

    return groups.map((group) => (
      <section key={group.key} className="mb-6">
        <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-muted-foreground">
          {group.title}{" "}
          <span className="ml-1 text-xs font-normal">
            ({group.reviews.length})
          </span>
        </h2>
        <div className="space-y-3">
          {group.reviews.map((review) => (
            <ReviewCard
              key={review.id}
              review={review}
              onAction={handleReviewAction}
            />
          ))}
        </div>
      </section>
    ));
  }

  function renderPrsContent(items: readonly Review[], emptyIcon: string, emptyTitle: string, emptyDescription: string) {
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
                <div className="relative">
                  <Search className="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
                  <input
                    type="text"
                    value={searchQuery}
                    onChange={(e) => {
                      setSearchQuery(e.target.value);
                    }}
                    placeholder="Search PRs..."
                    className="h-8 w-52 rounded-md border border-border bg-background pl-8 pr-3 text-xs text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
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
              </div>

              <TabsContent value="my-prs">
                {renderPrsContent(
                  authoredPrs,
                  "📝",
                  "No open PRs",
                  "Click Refresh to fetch your open PRs from GitHub. Make sure your repo path is configured in Settings.",
                )}
              </TabsContent>

              <TabsContent value="review-requests">
                {renderPrsContent(
                  reviewRequests,
                  "👀",
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
                      icon="🚀"
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

      case "plan":
        return (
          <div className="mx-auto max-w-4xl px-6 py-8">
            {errorBanner}

            {plan !== null ? (
              <PlanView
                plan={plan}
                onAddComment={(anchor, body) => {
                  void addPlanComment(anchor, body);
                }}
                onRequestChanges={() => {
                  void planRequestChanges();
                }}
                onApprove={() => {
                  void planApprove();
                }}
                onOpen={() => {
                  void openPlan();
                }}
                onFetchBatchStatus={() => batchStatus()}
              />
            ) : (
              <EmptyState
                icon="📋"
                title="No plan loaded"
                description="Create a project with the plan gate enabled to get started."
                actionLabel="Go to Projects"
                onAction={navigateToProjects}
              />
            )}
          </div>
        );

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
                icon="📁"
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
