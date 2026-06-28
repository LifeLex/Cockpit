import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { Review } from "./bindings/Review";

interface AppStore {
  readonly reviews: readonly Review[];
  readonly frontier: readonly Review[];
  readonly loading: boolean;
  readonly error: string | null;

  fetchReviews: () => Promise<void>;
  fetchFrontier: () => Promise<void>;
  openReview: (pr: string) => Promise<void>;
}

export const useAppStore = create<AppStore>((set) => ({
  reviews: [],
  frontier: [],
  loading: false,
  error: null,

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
}));
