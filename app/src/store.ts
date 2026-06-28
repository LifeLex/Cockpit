import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { Review } from "./bindings/Review";
import type { DiffData } from "./bindings/DiffData";
import type { ProjectPlan } from "./bindings/ProjectPlan";
import type { BatchVerdict } from "./bindings/BatchVerdict";

/**
 * Navigation state discriminated union.
 *
 * The app is either showing the frontier list, reviewing a specific PR's diff,
 * or viewing the project plan. Using a tagged union (not optional fields)
 * makes exhaustive switching possible.
 */
type ViewState =
  | { readonly kind: "frontier" }
  | { readonly kind: "diff"; readonly pr: string }
  | { readonly kind: "plan" };

interface AppStore {
  readonly reviews: readonly Review[];
  readonly frontier: readonly Review[];
  readonly plan: ProjectPlan | null;
  readonly loading: boolean;
  readonly error: string | null;

  /** Current navigation state. */
  readonly view: ViewState;

  /** The review currently being viewed in the diff gate. */
  readonly activeReview: Review | null;

  /** Diff data for the active review. */
  readonly activeDiff: DiffData | null;

  fetchReviews: () => Promise<void>;
  fetchFrontier: () => Promise<void>;
  openReview: (pr: string) => Promise<void>;

  /** Navigate to the diff view for a specific PR. */
  navigateToDiff: (pr: string) => Promise<void>;

  /** Navigate to the plan view. */
  navigateToPlan: () => void;

  /** Navigate back to the frontier list. */
  navigateToFrontier: () => void;

  /** Add an anchored comment to the active review. */
  addComment: (
    file: string,
    lineStart: number,
    lineEnd: number,
    body: string,
  ) => Promise<void>;

  /** Request changes on the active review (InReview -> Dispatched). */
  requestChanges: () => Promise<void>;

  /** Refresh the active review to pick up state changes. */
  refreshActiveReview: () => Promise<void>;

  fetchPlan: () => Promise<void>;
  loadPlan: (file: string, project: string) => Promise<void>;
  addPlanComment: (anchor: string, body: string) => Promise<void>;
  requestPlanChanges: () => Promise<void>;
  approvePlan: () => Promise<void>;
  openPlan: () => Promise<void>;

  /** Batch-approve preview results. */
  readonly batchVerdicts: readonly [Review, BatchVerdict][] | null;

  /** Whether the batch-approve panel is visible. */
  readonly showBatchPanel: boolean;

  /** Fetch batch-approve preview from the backend. */
  fetchBatchApprovePreview: () => Promise<void>;

  /** Approve a single review by PR ref (explicit user action). */
  approveReview: (pr: string) => Promise<void>;

  /** Approve all eligible reviews in the current batch preview. */
  approveAllEligible: () => Promise<void>;

  /** Toggle visibility of the batch-approve panel. */
  toggleBatchPanel: () => void;
}

export const useAppStore = create<AppStore>((set, get) => ({
  reviews: [],
  frontier: [],
  plan: null,
  loading: false,
  error: null,
  view: { kind: "frontier" },
  activeReview: null,
  activeDiff: null,
  batchVerdicts: null,
  showBatchPanel: false,

  fetchReviews: async () => {
    set({ loading: true, error: null });
    try {
      const reviews = await invoke<Review[]>("list_reviews");
      set({ reviews, loading: false });
    } catch (e: unknown) {
      set({ error: String(e), loading: false });
    }
  },

  fetchFrontier: async () => {
    try {
      const frontier = await invoke<Review[]>("get_frontier");
      set({ frontier });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  openReview: async (pr: string) => {
    try {
      await invoke("open_review", { pr });
      const reviews = await invoke<Review[]>("list_reviews");
      const frontier = await invoke<Review[]>("get_frontier");
      set({ reviews, frontier });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  navigateToDiff: async (pr: string) => {
    set({ loading: true, error: null });
    try {
      const review = await invoke<Review>("open_review", { pr });
      const diff = await invoke<DiffData>("get_review_diff", { pr });
      set({
        view: { kind: "diff", pr },
        activeReview: review,
        activeDiff: diff,
        loading: false,
      });
    } catch (e: unknown) {
      set({ error: String(e), loading: false });
    }
  },

  navigateToPlan: () => {
    set({ view: { kind: "plan" } });
    void get().fetchPlan();
  },

  navigateToFrontier: () => {
    set({
      view: { kind: "frontier" },
      activeReview: null,
      activeDiff: null,
    });
    void get().fetchReviews();
    void get().fetchFrontier();
  },

  addComment: async (
    file: string,
    lineStart: number,
    lineEnd: number,
    body: string,
  ) => {
    const { view } = get();
    if (view.kind !== "diff") return;

    try {
      const review = await invoke<Review>("add_comment", {
        pr: view.pr,
        file,
        lineStart,
        lineEnd,
        body,
      });
      set({ activeReview: review });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  requestChanges: async () => {
    const { view } = get();
    if (view.kind !== "diff") return;

    try {
      const review = await invoke<Review>("request_changes", {
        pr: view.pr,
      });
      set({ activeReview: review });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  refreshActiveReview: async () => {
    const { view } = get();
    if (view.kind !== "diff") return;

    try {
      const review = await invoke<Review>("get_review", { pr: view.pr });
      const diff = await invoke<DiffData>("get_review_diff", { pr: view.pr });
      set({ activeReview: review, activeDiff: diff });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  fetchPlan: async () => {
    try {
      const plan = await invoke<ProjectPlan | null>("get_plan");
      set({ plan });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  loadPlan: async (file: string, project: string) => {
    set({ loading: true, error: null });
    try {
      const plan = await invoke<ProjectPlan>("load_plan", { file, project });
      set({ plan, loading: false });
    } catch (e: unknown) {
      set({ error: String(e), loading: false });
    }
  },

  addPlanComment: async (anchor: string, body: string) => {
    try {
      const plan = await invoke<ProjectPlan>("add_plan_comment", {
        anchor,
        body,
      });
      set({ plan });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  requestPlanChanges: async () => {
    try {
      const plan = await invoke<ProjectPlan>("plan_request_changes");
      set({ plan });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  approvePlan: async () => {
    try {
      const plan = await invoke<ProjectPlan>("plan_approve");
      set({ plan });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  openPlan: async () => {
    try {
      const plan = await invoke<ProjectPlan>("plan_open");
      set({ plan });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  fetchBatchApprovePreview: async () => {
    set({ loading: true, error: null });
    try {
      const results = await invoke<[Review, BatchVerdict][]>(
        "batch_approve_preview",
      );
      set({ batchVerdicts: results, showBatchPanel: true, loading: false });
    } catch (e: unknown) {
      set({ error: String(e), loading: false });
    }
  },

  approveReview: async (pr: string) => {
    try {
      await invoke<Review>("approve_review", { pr });
      // Refresh after approval.
      await get().fetchBatchApprovePreview();
      await get().fetchFrontier();
      await get().fetchReviews();
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  approveAllEligible: async () => {
    const { batchVerdicts } = get();
    if (batchVerdicts === null) return;

    for (const [review, verdict] of batchVerdicts) {
      if (verdict.kind === "Eligible") {
        try {
          await invoke<Review>("approve_review", { pr: review.pr });
        } catch (e: unknown) {
          set({ error: String(e) });
          return;
        }
      }
    }

    // Refresh after all approvals.
    await get().fetchBatchApprovePreview();
    await get().fetchFrontier();
    await get().fetchReviews();
  },

  toggleBatchPanel: () => {
    const { showBatchPanel } = get();
    set({ showBatchPanel: !showBatchPanel });
  },
}));

export type { ViewState };
