import { useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { useAppStore } from "./store";
import { ReviewCard } from "./components/ReviewCard";
import { DiffView } from "./components/DiffView";

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

function App() {
  const reviews = useAppStore((s) => s.reviews);
  const frontier = useAppStore((s) => s.frontier);
  const loading = useAppStore((s) => s.loading);
  const error = useAppStore((s) => s.error);
  const view = useAppStore((s) => s.view);
  const activeReview = useAppStore((s) => s.activeReview);
  const activeDiff = useAppStore((s) => s.activeDiff);
  const fetchReviews = useAppStore((s) => s.fetchReviews);
  const fetchFrontier = useAppStore((s) => s.fetchFrontier);
  const openReview = useAppStore((s) => s.openReview);
  const navigateToDiff = useAppStore((s) => s.navigateToDiff);
  const navigateToFrontier = useAppStore((s) => s.navigateToFrontier);
  const addComment = useAppStore((s) => s.addComment);
  const requestChanges = useAppStore((s) => s.requestChanges);
  const refreshActiveReview = useAppStore((s) => s.refreshActiveReview);

  useEffect(() => {
    void fetchReviews();
    void fetchFrontier();

    // Listen for agent completion events pushed from the Rust side.
    const unlisten = listen("agent-completed", () => {
      void fetchReviews();
      void fetchFrontier();
      // If we are viewing a diff, refresh it too.
      void refreshActiveReview();
    });

    return () => {
      void unlisten.then((f) => {
        f();
      });
    };
  }, [fetchReviews, fetchFrontier, refreshActiveReview]);

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
          <h1 style={{ marginBottom: 24 }}>Cockpit</h1>

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
        />
      );

    default:
      return assertNever(view);
  }
}

export default App;
