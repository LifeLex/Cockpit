import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { useAppStore } from "./store";
import { ReviewCard } from "./components/ReviewCard";
import { PlanView } from "./components/PlanView";

function App() {
  const reviews = useAppStore((s) => s.reviews);
  const frontier = useAppStore((s) => s.frontier);
  const plan = useAppStore((s) => s.plan);
  const loading = useAppStore((s) => s.loading);
  const error = useAppStore((s) => s.error);
  const activeView = useAppStore((s) => s.activeView);
  const setActiveView = useAppStore((s) => s.setActiveView);
  const fetchReviews = useAppStore((s) => s.fetchReviews);
  const fetchFrontier = useAppStore((s) => s.fetchFrontier);
  const fetchPlan = useAppStore((s) => s.fetchPlan);
  const openReview = useAppStore((s) => s.openReview);
  const addPlanComment = useAppStore((s) => s.addPlanComment);
  const requestPlanChanges = useAppStore((s) => s.requestPlanChanges);
  const approvePlan = useAppStore((s) => s.approvePlan);
  const openPlan = useAppStore((s) => s.openPlan);

  useEffect(() => {
    void fetchReviews();
    void fetchFrontier();
    void fetchPlan();

    // Listen for agent completion events pushed from the Rust side.
    const unlisten = listen("agent-completed", () => {
      void fetchReviews();
      void fetchFrontier();
      void fetchPlan();
    });

    return () => {
      void unlisten.then((f) => {
        f();
      });
    };
  }, [fetchReviews, fetchFrontier, fetchPlan]);

  return (
    <main style={{ padding: 24, maxWidth: 900, margin: "0 auto" }}>
      <h1 style={{ marginBottom: 16 }}>Cockpit</h1>

      {/* Navigation tabs */}
      <nav
        style={{
          display: "flex",
          gap: 0,
          marginBottom: 24,
          borderBottom: "1px solid #444",
        }}
      >
        <button
          onClick={() => {
            setActiveView("reviews");
          }}
          style={{
            padding: "8px 20px",
            cursor: "pointer",
            border: "1px solid #444",
            borderBottom:
              activeView === "reviews" ? "2px solid #2196F3" : "none",
            backgroundColor:
              activeView === "reviews" ? "#1e1e1e" : "transparent",
            color: activeView === "reviews" ? "#2196F3" : "#888",
            fontWeight: activeView === "reviews" ? "bold" : "normal",
            borderRadius: "4px 4px 0 0",
          }}
        >
          Reviews ({reviews.length})
        </button>
        <button
          onClick={() => {
            setActiveView("plan");
          }}
          style={{
            padding: "8px 20px",
            cursor: "pointer",
            border: "1px solid #444",
            borderBottom:
              activeView === "plan" ? "2px solid #9C27B0" : "none",
            backgroundColor:
              activeView === "plan" ? "#1e1e1e" : "transparent",
            color: activeView === "plan" ? "#9C27B0" : "#888",
            fontWeight: activeView === "plan" ? "bold" : "normal",
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

      {/* Reviews view */}
      {activeView === "reviews" && (
        <>
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
              />
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
        </>
      )}

      {/* Plan view */}
      {activeView === "plan" && (
        <>
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
        </>
      )}
    </main>
  );
}

export default App;
