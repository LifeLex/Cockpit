import { useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { useAppStore } from "./store";
import { ReviewCard } from "./components/ReviewCard";
import { DiffView } from "./components/DiffView";
import { PlanView } from "./components/PlanView";

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
  const openReview = useAppStore((s) => s.openReview);
  const navigateToDiff = useAppStore((s) => s.navigateToDiff);
  const navigateToPlan = useAppStore((s) => s.navigateToPlan);
  const navigateToFrontier = useAppStore((s) => s.navigateToFrontier);
  const addComment = useAppStore((s) => s.addComment);
  const requestChanges = useAppStore((s) => s.requestChanges);
  const mirrorComments = useAppStore((s) => s.mirrorComments);
  const refreshActiveReview = useAppStore((s) => s.refreshActiveReview);
  const addPlanComment = useAppStore((s) => s.addPlanComment);
  const requestPlanChanges = useAppStore((s) => s.requestPlanChanges);
  const approvePlan = useAppStore((s) => s.approvePlan);
  const openPlan = useAppStore((s) => s.openPlan);

  useEffect(() => {
    void fetchReviews();
    void fetchFrontier();
    void fetchPlan();

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
  }, [fetchReviews, fetchFrontier, fetchPlan, refreshActiveReview]);

  const handleViewDiff = useCallback(
    (pr: string) => {
      void navigateToDiff(pr);
    },
    [navigateToDiff],
  );

  switch (view.kind) {
    case "frontier":
      return (
        <main style={{ padding: 24, maxWidth: 900, margin: "0 auto" }}>
          <h1 style={{ marginBottom: 16 }}>Cockpit</h1>

          <nav
            style={{
              display: "flex",
              gap: 0,
              marginBottom: 24,
              borderBottom: "1px solid #444",
            }}
          >
            <button
              style={{
                padding: "8px 20px",
                cursor: "pointer",
                border: "1px solid #444",
                borderBottom: "2px solid #2196F3",
                backgroundColor: "#1e1e1e",
                color: "#2196F3",
                fontWeight: "bold",
                borderRadius: "4px 4px 0 0",
              }}
            >
              Reviews ({reviews.length})
            </button>
            <button
              onClick={navigateToPlan}
              style={{
                padding: "8px 20px",
                cursor: "pointer",
                border: "1px solid #444",
                borderBottom: "none",
                backgroundColor: "transparent",
                color: "#888",
                fontWeight: "normal",
                borderRadius: "4px 4px 0 0",
              }}
            >
              Plan {plan != null ? "(loaded)" : ""}
            </button>
          </nav>

          {error != null && (
            <div
              style={{
                color: "#f44336",
                padding: 12,
                marginBottom: 16,
                border: "1px solid #f44336",
                borderRadius: 4,
              }}
            >
              {error}
            </div>
          )}

          <section>
            <h2>Frontier ({frontier.length})</h2>
            <p style={{ color: "#888", fontSize: 14 }}>
              Reviews ready for deep-review (not stale)
            </p>
            {loading && <p>Loading...</p>}
            {frontier.map((review) => (
              <ReviewCard
                key={review.id}
                review={review}
                onOpen={openReview}
                onViewDiff={handleViewDiff}
              />
            ))}
            {!loading && frontier.length === 0 && (
              <p style={{ color: "#888" }}>No reviews in the frontier.</p>
            )}
          </section>

          <section style={{ marginTop: 32 }}>
            <h2>All Reviews ({reviews.length})</h2>
            {reviews.map((review) => (
              <ReviewCard
                key={review.id}
                review={review}
                onViewDiff={handleViewDiff}
              />
            ))}
            {reviews.length === 0 && (
              <p style={{ color: "#888" }}>
                No reviews loaded. Use the CLI to ingest PRs.
              </p>
            )}
          </section>
        </main>
      );

    case "diff":
      if (activeReview === null || activeDiff === null) {
        return (
          <main style={{ padding: 24 }}>
            {loading ? (
              <p>Loading diff...</p>
            ) : (
              <p style={{ color: "#f44336" }}>
                Failed to load review.{" "}
                <button onClick={navigateToFrontier} style={{ cursor: "pointer" }}>
                  Back
                </button>
              </p>
            )}
          </main>
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
        <main style={{ padding: 24, maxWidth: 900, margin: "0 auto" }}>
          <h1 style={{ marginBottom: 16 }}>Cockpit</h1>

          <nav
            style={{
              display: "flex",
              gap: 0,
              marginBottom: 24,
              borderBottom: "1px solid #444",
            }}
          >
            <button
              onClick={navigateToFrontier}
              style={{
                padding: "8px 20px",
                cursor: "pointer",
                border: "1px solid #444",
                borderBottom: "none",
                backgroundColor: "transparent",
                color: "#888",
                fontWeight: "normal",
                borderRadius: "4px 4px 0 0",
              }}
            >
              Reviews ({reviews.length})
            </button>
            <button
              style={{
                padding: "8px 20px",
                cursor: "pointer",
                border: "1px solid #444",
                borderBottom: "2px solid #9C27B0",
                backgroundColor: "#1e1e1e",
                color: "#9C27B0",
                fontWeight: "bold",
                borderRadius: "4px 4px 0 0",
              }}
            >
              Plan {plan != null ? "(loaded)" : ""}
            </button>
          </nav>

          {error != null && (
            <div
              style={{
                color: "#f44336",
                padding: 12,
                marginBottom: 16,
                border: "1px solid #f44336",
                borderRadius: 4,
              }}
            >
              {error}
            </div>
          )}

          {plan != null ? (
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
            <div style={{ color: "#888", padding: 24, textAlign: "center" }}>
              <p>No plan loaded.</p>
              <p style={{ fontSize: 13 }}>
                Use the CLI to load a plan file, or invoke <code>load_plan</code>{" "}
                from the command palette.
              </p>
            </div>
          )}
        </main>
      );

    default:
      return assertNever(view);
  }
}

export default App;
