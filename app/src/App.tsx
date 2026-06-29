import { useState, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { useAppStore } from "./store";
import type { ViewState } from "./store";
import { Sidebar } from "./components/Sidebar";
import { ReviewCard } from "./components/ReviewCard";
import { DiffView } from "./components/DiffView";
import { PlanView } from "./components/PlanView";
import { BatchApprovePanel } from "./components/BatchApprovePanel";
import { StackView } from "./components/StackView";
import { SettingsView } from "./components/SettingsView";
import { KickoffView } from "./components/KickoffView";

type ReviewTab = "my-prs" | "review-requests" | "frontier";

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
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
  const openReview = useAppStore((s) => s.openReview);
  const navigateToDiff = useAppStore((s) => s.navigateToDiff);
  const navigateToPlan = useAppStore((s) => s.navigateToPlan);
  const navigateToFrontier = useAppStore((s) => s.navigateToFrontier);
  const navigateToStacks = useAppStore((s) => s.navigateToStacks);
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
    });

    return () => {
      void unlisten.then((f) => {
        f();
      });
    };
  }, [fetchReviews, fetchFrontier, fetchPlan, fetchConfig, fetchAuthoredPrs, refreshActiveReview]);

  const handleViewDiff = useCallback(
    (pr: string) => {
      void navigateToDiff(pr);
    },
    [navigateToDiff],
  );

  /** Map sidebar navigation kinds to store navigation actions. */
  const handleNavigate = useCallback(
    (kind: ViewState["kind"]) => {
      switch (kind) {
        case "frontier":
          navigateToFrontier();
          break;
        case "plan":
          navigateToPlan();
          break;
        case "stacks":
          navigateToStacks();
          break;
        case "settings":
          navigateToSettings();
          break;
        case "kickoff":
          navigateToKickoff();
          break;
        case "diff":
          // Diff is navigated via handleViewDiff, not the sidebar.
          break;
        default:
          assertNever(kind);
      }
    },
    [navigateToFrontier, navigateToPlan, navigateToStacks, navigateToSettings, navigateToKickoff],
  );

  /** Error banner shown when a global error is present. */
  const errorBanner =
    error !== null ? (
      <div className="mb-4 rounded-lg border border-danger bg-danger/10 px-4 py-3 text-sm text-danger">
        {error}
      </div>
    ) : null;

  /** Render main content based on the current view. */
  function renderContent() {
    switch (view.kind) {
      case "frontier":
        return (
          <div className="mx-auto max-w-4xl px-6 py-8">
            {errorBanner}

            {/* Tab bar */}
            <div className="mb-6 flex items-center gap-1 border-b border-border">
              {(
                [
                  { key: "my-prs" as const, label: "My PRs", count: authoredPrs.length },
                  { key: "review-requests" as const, label: "Review Requests", count: reviewRequests.length },
                  { key: "frontier" as const, label: "Frontier", count: frontier.length },
                ] as const
              ).map((tab) => (
                <button
                  key={tab.key}
                  onClick={() => {
                    setReviewTab(tab.key);
                    if (tab.key === "my-prs") void fetchAuthoredPrs();
                    if (tab.key === "review-requests") void fetchReviewRequests();
                  }}
                  className={[
                    "relative px-4 py-2.5 text-sm font-medium transition-colors",
                    reviewTab === tab.key
                      ? "text-accent"
                      : "text-text-muted hover:text-text-secondary",
                  ].join(" ")}
                >
                  {tab.label}
                  {tab.count > 0 && (
                    <span className="ml-1.5 rounded-full bg-surface-3 px-1.5 py-0.5 text-xs text-text-muted">
                      {tab.count}
                    </span>
                  )}
                  {reviewTab === tab.key && (
                    <span className="absolute inset-x-0 bottom-0 h-0.5 bg-accent" />
                  )}
                </button>
              ))}

              {/* Batch approve in the frontier tab */}
              {reviewTab === "frontier" && frontier.length > 0 && (
                <button
                  onClick={() => {
                    void fetchBatchApprovePreview();
                  }}
                  className="ml-auto rounded-md bg-success px-4 py-1.5 text-sm font-medium text-white transition-colors hover:opacity-90"
                >
                  Batch Approve
                </button>
              )}

              {/* Refresh button for GitHub tabs */}
              {(reviewTab === "my-prs" || reviewTab === "review-requests") && (
                <button
                  onClick={() => {
                    if (reviewTab === "my-prs") void fetchAuthoredPrs();
                    else void fetchReviewRequests();
                  }}
                  disabled={prFetchLoading}
                  className="ml-auto rounded-md border border-border bg-surface-2 px-3 py-1.5 text-sm text-text-secondary transition-colors hover:bg-surface-3 disabled:opacity-50"
                >
                  {prFetchLoading ? "Fetching..." : "Refresh"}
                </button>
              )}
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

            {/* Tab content */}
            {reviewTab === "my-prs" && (
              <section>
                {prFetchLoading && authoredPrs.length === 0 && (
                  <p className="text-sm text-text-muted">Fetching your PRs from GitHub...</p>
                )}
                {authoredPrs.map((review) => (
                  <ReviewCard
                    key={review.id}
                    review={review}
                    onOpen={openReview}
                    onViewDiff={handleViewDiff}
                  />
                ))}
                {!prFetchLoading && authoredPrs.length === 0 && (
                  <div className="rounded-lg border border-border bg-surface-1 p-8 text-center">
                    <p className="text-text-muted">No open PRs found.</p>
                    <p className="mt-1 text-sm text-text-muted">
                      Click Refresh to fetch your open PRs from GitHub, or make sure your repo path is set in Settings.
                    </p>
                  </div>
                )}
              </section>
            )}

            {reviewTab === "review-requests" && (
              <section>
                {prFetchLoading && reviewRequests.length === 0 && (
                  <p className="text-sm text-text-muted">Fetching review requests from GitHub...</p>
                )}
                {reviewRequests.map((review) => (
                  <ReviewCard
                    key={review.id}
                    review={review}
                    onOpen={openReview}
                    onViewDiff={handleViewDiff}
                  />
                ))}
                {!prFetchLoading && reviewRequests.length === 0 && (
                  <div className="rounded-lg border border-border bg-surface-1 p-8 text-center">
                    <p className="text-text-muted">No review requests found.</p>
                    <p className="mt-1 text-sm text-text-muted">
                      Click Refresh to fetch PRs where your review is requested.
                    </p>
                  </div>
                )}
              </section>
            )}

            {reviewTab === "frontier" && (
              <>
                <section>
                  {loading && (
                    <p className="text-sm text-text-muted">Loading...</p>
                  )}
                  {frontier.map((review) => (
                    <ReviewCard
                      key={review.id}
                      review={review}
                      onOpen={openReview}
                      onViewDiff={handleViewDiff}
                    />
                  ))}
                  {!loading && frontier.length === 0 && (
                    <div className="rounded-lg border border-border bg-surface-1 p-8 text-center">
                      <p className="text-text-muted">No reviews in the frontier.</p>
                      <p className="mt-1 text-sm text-text-muted">
                        Use Kickoff to import a Linear project, or switch to My PRs to review existing GitHub PRs.
                      </p>
                    </div>
                  )}
                </section>

                {reviews.length > 0 && (
                  <section className="mt-8">
                    <h2 className="mb-3 text-lg font-semibold text-text-primary">
                      All Reviews ({reviews.length})
                    </h2>
                    {reviews.map((review) => (
                      <ReviewCard
                        key={review.id}
                        review={review}
                        onViewDiff={handleViewDiff}
                      />
                    ))}
                  </section>
                )}
              </>
            )}
          </div>
        );

      case "diff":
        if (activeReview === null || activeDiff === null) {
          return (
            <div className="px-6 py-8">
              {loading ? (
                <p className="text-sm text-text-muted">Loading diff...</p>
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
              <div className="rounded-lg border border-border bg-surface-1 p-8 text-center">
                <p className="text-text-muted">No plan loaded.</p>
                <p className="mt-1 text-sm text-text-muted">
                  Use the CLI to load a plan file, or invoke{" "}
                  <code className="rounded bg-surface-2 px-1.5 py-0.5 text-xs">
                    load_plan
                  </code>{" "}
                  from the command palette.
                </p>
              </div>
            )}
          </div>
        );

      case "stacks":
        return (
          <div className="mx-auto max-w-4xl px-6 py-8">
            {errorBanner}

            <section>
              <h2 className="mb-1 text-lg font-semibold text-text-primary">
                Stack Dependencies
              </h2>
              <p className="mb-4 text-sm text-text-secondary">
                Review dependency graph — click a node to view its diff
              </p>
              {loading && (
                <p className="text-sm text-text-muted">Loading...</p>
              )}
              <StackView reviews={reviews} onViewDiff={handleViewDiff} />
            </section>
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
    <div className="flex h-screen overflow-hidden bg-surface-0 text-text-primary">
      <Sidebar
        activeView={view.kind}
        reviewCount={reviews.length}
        hasPlan={plan !== null}
        onNavigate={handleNavigate}
      />
      <main className="flex-1 overflow-y-auto">
        {renderContent()}
      </main>
    </div>
  );
}

export default App;
