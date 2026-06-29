import { useEffect, useCallback } from "react";
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
  const fetchReviews = useAppStore((s) => s.fetchReviews);
  const fetchFrontier = useAppStore((s) => s.fetchFrontier);
  const fetchPlan = useAppStore((s) => s.fetchPlan);
  const fetchConfig = useAppStore((s) => s.fetchConfig);
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
  }, [fetchReviews, fetchFrontier, fetchPlan, fetchConfig, refreshActiveReview]);

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

            <section>
              <div className="mb-4 flex items-center justify-between">
                <div>
                  <h2 className="text-lg font-semibold text-text-primary">
                    Frontier ({frontier.length})
                  </h2>
                  <p className="text-sm text-text-secondary">
                    Reviews ready for deep-review (not stale)
                  </p>
                </div>
                {frontier.length > 0 && (
                  <button
                    onClick={() => {
                      void fetchBatchApprovePreview();
                    }}
                    className="rounded-md bg-success px-4 py-2 text-sm font-medium text-white transition-colors hover:opacity-90"
                  >
                    Batch Approve
                  </button>
                )}
              </div>

              {showBatchPanel && batchVerdicts !== null && (
                <BatchApprovePanel
                  verdicts={batchVerdicts}
                  onApprove={approveReview}
                  onApproveAll={() => {
                    void approveAllEligible();
                  }}
                  onClose={toggleBatchPanel}
                />
              )}

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
                <p className="text-sm text-text-muted">
                  No reviews in the frontier.
                </p>
              )}
            </section>

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
              {reviews.length === 0 && (
                <p className="text-sm text-text-muted">
                  No reviews loaded. Use the CLI to ingest PRs.
                </p>
              )}
            </section>
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
