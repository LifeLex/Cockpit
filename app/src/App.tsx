import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { useAppStore } from "./store";
import { ReviewCard } from "./components/ReviewCard";

function App() {
  const reviews = useAppStore((s) => s.reviews);
  const frontier = useAppStore((s) => s.frontier);
  const loading = useAppStore((s) => s.loading);
  const error = useAppStore((s) => s.error);
  const fetchReviews = useAppStore((s) => s.fetchReviews);
  const fetchFrontier = useAppStore((s) => s.fetchFrontier);
  const openReview = useAppStore((s) => s.openReview);

  useEffect(() => {
    void fetchReviews();
    void fetchFrontier();

    // Listen for agent completion events pushed from the Rust side.
    const unlisten = listen("agent-completed", () => {
      void fetchReviews();
      void fetchFrontier();
    });

    return () => {
      void unlisten.then((f) => {
        f();
      });
    };
  }, [fetchReviews, fetchFrontier]);

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
          <ReviewCard key={review.id} review={review} onOpen={openReview} />
        ))}
        {!loading && frontier.length === 0 && (
          <p style={{ color: "#888" }}>No reviews in the frontier.</p>
        )}
      </section>

      <section style={{ marginTop: 32 }}>
        <h2>All Reviews ({reviews.length})</h2>
        {reviews.map((review) => (
          <ReviewCard key={review.id} review={review} />
        ))}
        {reviews.length === 0 && (
          <p style={{ color: "#888" }}>
            No reviews loaded. Use the CLI to ingest PRs.
          </p>
        )}
      </section>
    </main>
  );
}

export default App;
