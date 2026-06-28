import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { Review } from "./bindings/Review";
import type { DiffData } from "./bindings/DiffData";

/**
 * Navigation state discriminated union.
 *
 * The app is either showing the frontier list or reviewing a specific PR's diff.
 * Using a tagged union (not optional fields) makes exhaustive switching possible.
 */
type ViewState =
  | { readonly kind: "frontier" }
  | { readonly kind: "diff"; readonly pr: string };

interface AppStore {
  readonly reviews: readonly Review[];
  readonly frontier: readonly Review[];
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
}

export const useAppStore = create<AppStore>((set, get) => ({
  reviews: [],
  frontier: [],
  loading: false,
  error: null,
  view: { kind: "frontier" },
  activeReview: null,
  activeDiff: null,

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
      // Refresh after mutation.
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
      // Open the review (transitions Pending/Reworked -> InReview)
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

  navigateToFrontier: () => {
    set({
      view: { kind: "frontier" },
      activeReview: null,
      activeDiff: null,
    });
    // Refresh lists after returning from a diff view.
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
}));

export type { ViewState };
