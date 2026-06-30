import { useState, useEffect, useCallback, useMemo } from "react";
import { listen } from "@tauri-apps/api/event";
import { sendNotification } from "@tauri-apps/plugin-notification";
import { useAppStore } from "./store";
import type { ViewState } from "./store";
import { useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import type { ShortcutMap } from "./hooks/useKeyboardShortcuts";
import { Sidebar } from "./components/Sidebar";
import { ReviewCard } from "./components/ReviewCard";
import { DiffView } from "./components/DiffView";
import { PlanView } from "./components/PlanView";
import { BatchApprovePanel } from "./components/BatchApprovePanel";
import { buildStackTrees, computeHealth } from "./lib/stack-tree";
import type { StackTreeNode } from "./lib/stack-tree";
import { SettingsView } from "./components/SettingsView";
import { KickoffView } from "./components/KickoffView";
import { CommandPalette } from "./components/CommandPalette";
import { SkeletonList } from "./components/SkeletonCard";
import { EmptyState } from "./components/EmptyState";
import { StateFilter } from "./components/StateFilter";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { Search } from "lucide-react";
import type { GateState } from "./bindings/GateState";
import type { Review } from "./bindings/Review";

type ReviewTab = "my-prs" | "review-requests" | "frontier";

const SIDEBAR_COLLAPSED_KEY = "cockpit-sidebar-collapsed";

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

function FrontierStackNode({
  node,
  depth,
  onAction,
}: {
  readonly node: StackTreeNode;
  readonly depth: number;
  readonly onAction: (pr: string) => void;
}) {
  return (
    <div style={{ marginLeft: depth * 20 }}>
      <div
        className={cn(
          "transition-opacity",
          node.review.gate_state === "Approved" && "opacity-60",
        )}
      >
        <ReviewCard review={node.review} onAction={onAction} />
      </div>
      {node.childNodes.map((child) => (
        <FrontierStackNode
          key={child.review.id}
          node={child}
          depth={depth + 1}
          onAction={onAction}
        />
      ))}
    </div>
  );
}

function FrontierStackGroup({
  root,
  onAction,
}: {
  readonly root: StackTreeNode;
  readonly onAction: (pr: string) => void;
}) {
  const health = useMemo(() => computeHealth(root), [root]);

  return (
    <div className="rounded-lg border border-border p-3 mb-4">
      <div className="flex justify-between items-center mb-2 pb-2 border-b border-border">
        <span className="text-[13px] font-bold text-muted-foreground">
          Stack: {root.review.branch}
        </span>
        <span className="text-xs text-muted-foreground">
          <span
            className={
              health.approved === health.total
                ? "text-success"
                : "text-muted-foreground"
            }
          >
            {health.approved}/{health.total} approved
          </span>
          {health.stale > 0 && (
            <span className="text-danger ml-2">{health.stale} stale</span>
          )}
        </span>
      </div>

      <FrontierStackNode node={root} depth={0} onAction={onAction} />
    </div>
  );
}

function App() {
  const reviews = useAppStore((s) => s.reviews);
  const frontier = useAppStore((s) => s.frontier);
  const plan = useAppStore((s) => s.plan);
  const loading = useAppStore((s) => s.loading);
  const error = useAppStore((s) => s.error);
  const view = useAppStore((s) => s.view);
  const activeReview = useAppStore((s) => s.activeReview);
  const activeDiff = useAppStore((s) => s.activeDiff);
  const authoredPrs = useAppStore((s) => s.authoredPrs);
  const reviewRequests = useAppStore((s) => s.reviewRequests);
  const prFetchLoading = useAppStore((s) => s.prFetchLoading);
  const fetchAuthoredPrs = useAppStore((s) => s.fetchAuthoredPrs);
  const fetchReviewRequests = useAppStore((s) => s.fetchReviewRequests);
  const fetchReviews = useAppStore((s) => s.fetchReviews);
  const fetchFrontier = useAppStore((s) => s.fetchFrontier);
  const fetchPlan = useAppStore((s) => s.fetchPlan);
  const fetchConfig = useAppStore((s) => s.fetchConfig);

  const [reviewTab, setReviewTab] = useState<ReviewTab>("my-prs");
  const [stateFilter, setStateFilter] = useState<GateState | null>(null);
  const [showStale, setShowStale] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const openReview = useAppStore((s) => s.openReview);
  const navigateToDiff = useAppStore((s) => s.navigateToDiff);
  const navigateToPlan = useAppStore((s) => s.navigateToPlan);
  const navigateToFrontier = useAppStore((s) => s.navigateToFrontier);
  const navigateToSettings = useAppStore((s) => s.navigateToSettings);
  const navigateToKickoff = useAppStore((s) => s.navigateToKickoff);
  const addComment = useAppStore((s) => s.addComment);
  const requestChanges = useAppStore((s) => s.requestChanges);
  const mirrorComments = useAppStore((s) => s.mirrorComments);
  const refreshActiveReview = useAppStore((s) => s.refreshActiveReview);
  const addPlanComment = useAppStore((s) => s.addPlanComment);
  const requestPlanChanges = useAppStore((s) => s.requestPlanChanges);
  const approvePlan = useAppStore((s) => s.approvePlan);
  const openPlan = useAppStore((s) => s.openPlan);
  const batchVerdicts = useAppStore((s) => s.batchVerdicts);
  const showBatchPanel = useAppStore((s) => s.showBatchPanel);
  const fetchBatchApprovePreview = useAppStore(
    (s) => s.fetchBatchApprovePreview,
  );
  const approveReview = useAppStore((s) => s.approveReview);
  const approveAllEligible = useAppStore((s) => s.approveAllEligible);
  const toggleBatchPanel = useAppStore((s) => s.toggleBatchPanel);

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

  const shortcuts: ShortcutMap = useMemo(
    () => ({
      "meta+k": () => {
        setCommandPaletteOpen(true);
      },
      "meta+1": () => {
        navigateToFrontier();
      },
      "meta+2": () => {
        navigateToPlan();
      },
      "meta+comma": () => {
        navigateToSettings();
      },
      "meta+r": () => {
        void fetchReviews();
        void fetchFrontier();
        void fetchAuthoredPrs();
      },
      "meta+b": () => {
        toggleSidebar();
      },
      escape: () => {
        // Only navigate back when viewing a diff or plan, not from the frontier.
        if (view.kind === "diff" || view.kind === "plan") {
          navigateToFrontier();
        }
      },
    }),
    [
      navigateToFrontier,
      navigateToPlan,
      navigateToSettings,
      fetchReviews,
      fetchFrontier,
      fetchAuthoredPrs,
      toggleSidebar,
      view.kind,
    ],
  );

  useKeyboardShortcuts(shortcuts);

  useEffect(() => {
    void fetchReviews();
    void fetchFrontier();
    void fetchPlan();
    void fetchConfig();
    void fetchAuthoredPrs();

    const unlisten = listen("agent-completed", () => {
      void fetchReviews();
      void fetchFrontier();
      void fetchPlan();
      void refreshActiveReview();

      // Best-effort desktop notification. The active review may be stale
      // at this point, so we read the current value from the store.
      const current = useAppStore.getState().activeReview;
      const branch = current !== null ? current.branch : "a review";
      void sendNotification({
        title: "Rework Complete",
        body: `Agent finished on ${branch}`,
      });
    });

    return () => {
      void unlisten.then((f) => {
        f();
      });
    };
  }, [fetchReviews, fetchFrontier, fetchPlan, fetchConfig, fetchAuthoredPrs, refreshActiveReview]);

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

  const unfilteredReviewsForTab: readonly Review[] = useMemo(() => {
    switch (reviewTab) {
      case "my-prs":
        return authoredPrs;
      case "review-requests":
        return reviewRequests;
      case "frontier":
        return reviews;
      default: {
        const _exhaustive: never = reviewTab;
        throw new Error(`unreachable: ${String(_exhaustive)}`);
      }
    }
  }, [reviewTab, authoredPrs, reviewRequests, reviews]);

  const handleReviewAction = useCallback(
    (pr: string) => {
      const allReviews = [...authoredPrs, ...reviewRequests, ...frontier, ...reviews];
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
    [openReview, navigateToDiff, authoredPrs, reviewRequests, frontier, reviews],
  );

  const handleNavigate = useCallback(
    (kind: ViewState["kind"]) => {
      switch (kind) {
        case "frontier":
          navigateToFrontier();
          break;
        case "plan":
          navigateToPlan();
          break;
        case "settings":
          navigateToSettings();
          break;
        case "kickoff":
          navigateToKickoff();
          break;
        case "diff":
          break;
        default:
          assertNever(kind);
      }
    },
    [navigateToFrontier, navigateToPlan, navigateToSettings, navigateToKickoff],
  );

  // Justified cast: value is constrained to the three TabsTrigger values below
  const handleTabChange = useCallback(
    (value: unknown) => {
      if (value === null || typeof value !== "string") return;
      const tab = value as ReviewTab;
      setReviewTab(tab);
      setStateFilter(null);
      setShowStale(false);
      setSearchQuery("");
      if (tab === "my-prs") void fetchAuthoredPrs();
      if (tab === "review-requests") void fetchReviewRequests();
    },
    [fetchAuthoredPrs, fetchReviewRequests],
  );

  const errorBanner =
    error !== null ? (
      <div className="mb-4 rounded-lg border border-danger bg-danger/10 px-4 py-3 text-sm text-danger">
        {error}
      </div>
    ) : null;

  function renderContent() {
    switch (view.kind) {
      case "frontier":
        return (
          <div className="mx-auto max-w-4xl px-6 py-8">
            {errorBanner}

            <Tabs value={reviewTab} onValueChange={handleTabChange}>
              <div className="flex items-center mb-6">
                <TabsList variant="line">
                  <TabsTrigger value="my-prs">
                    My PRs
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
                  <TabsTrigger value="frontier">
                    Frontier
                    {frontier.length > 0 && (
                      <span className="ml-1.5 rounded-full bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
                        {frontier.length}
                      </span>
                    )}
                  </TabsTrigger>
                </TabsList>

                <div className="ml-auto flex items-center gap-2">
                  {reviewTab === "frontier" && frontier.length > 0 && (
                    <Button
                      onClick={() => {
                        void fetchBatchApprovePreview();
                      }}
                      className="bg-success text-white hover:bg-success/90"
                    >
                      Batch Approve
                    </Button>
                  )}

                  {(reviewTab === "my-prs" || reviewTab === "review-requests") && (
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

              {showBatchPanel && batchVerdicts !== null && reviewTab === "frontier" && (
                <BatchApprovePanel
                  verdicts={batchVerdicts}
                  onApprove={approveReview}
                  onApproveAll={() => {
                    void approveAllEligible();
                  }}
                  onClose={toggleBatchPanel}
                />
              )}

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
                  reviews={unfilteredReviewsForTab}
                  activeFilter={stateFilter}
                  showStale={showStale}
                  onFilterChange={setStateFilter}
                  onToggleStale={() => {
                    setShowStale((prev) => !prev);
                  }}
                />
              </div>

              <TabsContent value="my-prs">
                <section className="space-y-3">
                  {prFetchLoading && authoredPrs.length === 0 && <SkeletonList count={4} />}
                  {filterReviews(authoredPrs).map((review) => (
                    <ReviewCard
                      key={review.id}
                      review={review}
                      onAction={handleReviewAction}
                    />
                  ))}
                  {!prFetchLoading && authoredPrs.length === 0 && (
                    <EmptyState
                      icon="📝"
                      title="No open PRs"
                      description="Click Refresh to fetch your open PRs from GitHub. Make sure your repo path is configured in Settings."
                      actionLabel="Go to Settings"
                      onAction={navigateToSettings}
                    />
                  )}
                </section>
              </TabsContent>

              <TabsContent value="review-requests">
                <section className="space-y-3">
                  {prFetchLoading && reviewRequests.length === 0 && <SkeletonList count={4} />}
                  {filterReviews(reviewRequests).map((review) => (
                    <ReviewCard
                      key={review.id}
                      review={review}
                      onAction={handleReviewAction}
                    />
                  ))}
                  {!prFetchLoading && reviewRequests.length === 0 && (
                    <EmptyState
                      icon="👀"
                      title="No review requests"
                      description="No PRs are waiting for your review. Click Refresh to check again."
                    />
                  )}
                </section>
              </TabsContent>

              <TabsContent value="frontier">
                <section className="space-y-3">
                  {loading && frontier.length === 0 && <SkeletonList count={4} />}
                  {(() => {
                    const filtered = filterReviews(frontier);
                    const trees = buildStackTrees(filtered);

                    if (trees.length === 0 && !loading && filtered.length === 0) {
                      return (
                        <EmptyState
                          icon="🚀"
                          title="No reviews in the frontier"
                          description="Use Kickoff to import a Linear project, or switch to My PRs to review existing GitHub PRs."
                          actionLabel="Go to Kickoff"
                          onAction={navigateToKickoff}
                        />
                      );
                    }

                    return trees.map((root) => (
                      <FrontierStackGroup
                        key={root.review.id}
                        root={root}
                        onAction={handleReviewAction}
                      />
                    ));
                  })()}
                </section>

                {reviews.length > 0 && (
                  <section className="mt-8">
                    <h2 className="mb-3 text-lg font-semibold text-foreground">
                      All Reviews ({filterReviews(reviews).length})
                    </h2>
                    <div className="space-y-3">
                      {filterReviews(reviews).map((review) => (
                        <ReviewCard
                          key={review.id}
                          review={review}
                          onAction={handleReviewAction}
                        />
                      ))}
                    </div>
                  </section>
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
                    onClick={navigateToFrontier}
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
          <DiffView
            review={activeReview}
            diff={activeDiff}
            onBack={navigateToFrontier}
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
                  void requestPlanChanges();
                }}
                onApprove={() => {
                  void approvePlan();
                }}
                onOpen={() => {
                  void openPlan();
                }}
              />
            ) : (
              <EmptyState
                icon="📋"
                title="No plan loaded"
                description="Load a plan file or kick off a project with the plan gate enabled to get started."
                actionLabel="Go to Kickoff"
                onAction={navigateToKickoff}
              />
            )}
          </div>
        );

      case "settings":
        return <SettingsView />;

      case "kickoff":
        return <KickoffView />;

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
          hasPlan={plan !== null}
          onNavigate={handleNavigate}
          collapsed={sidebarCollapsed}
          onToggleCollapse={toggleSidebar}
        />
        <main className="flex-1 overflow-y-auto">
          {renderContent()}
        </main>
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
