import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { Review } from "./bindings/Review";
import type { ProjectPlan } from "./bindings/ProjectPlan";

/** Which top-level view is active. */
type ActiveView = "reviews" | "plan";

interface AppStore {
  readonly reviews: readonly Review[];
  readonly frontier: readonly Review[];
  readonly plan: ProjectPlan | null;
  readonly loading: boolean;
  readonly error: string | null;
  readonly activeView: ActiveView;

  setActiveView: (view: ActiveView) => void;
  fetchReviews: () => Promise<void>;
  fetchFrontier: () => Promise<void>;
  openReview: (pr: string) => Promise<void>;
  fetchPlan: () => Promise<void>;
  loadPlan: (file: string, project: string) => Promise<void>;
  addPlanComment: (anchor: string, body: string) => Promise<void>;
  requestPlanChanges: () => Promise<void>;
  approvePlan: () => Promise<void>;
  openPlan: () => Promise<void>;
}

export const useAppStore = create<AppStore>((set) => ({
  reviews: [],
  frontier: [],
  plan: null,
  loading: false,
  error: null,
  activeView: "reviews",

  setActiveView: (view: ActiveView) => {
    set({ activeView: view });
  },

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
}));
