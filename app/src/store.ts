import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { Review } from "./bindings/Review";
import type { DiffData } from "./bindings/DiffData";
import type { MirrorResult } from "./bindings/MirrorResult";
import type { ProjectPlan } from "./bindings/ProjectPlan";
import type { BatchVerdict } from "./bindings/BatchVerdict";
import type { Config } from "./bindings/Config";
import type { KickoffResult } from "./bindings/KickoffResult";

/**
 * Navigation state discriminated union.
 *
 * The app is either showing the frontier list, reviewing a specific PR's diff,
 * viewing the project plan, adjusting settings, or running the kickoff flow.
 * Using a tagged union (not optional fields) makes exhaustive switching
 * possible.
 */
type ViewState =
  | { readonly kind: "frontier" }
  | { readonly kind: "diff"; readonly pr: string }
  | { readonly kind: "plan" }
  | { readonly kind: "settings" }
  | { readonly kind: "kickoff" };

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

  /** Navigate to the settings view. */
  navigateToSettings: () => void;

  /** Navigate to the kickoff view. */
  navigateToKickoff: () => void;

  /** Add an anchored comment to the active review. */
  addComment: (
    file: string,
    lineStart: number,
    lineEnd: number,
    body: string,
  ) => Promise<void>;

  /** Request changes on the active review (InReview -> Dispatched). */
  requestChanges: () => Promise<void>;

  /** Mirror local comments for the active review to GitHub (explicit user action). */
  mirrorComments: () => Promise<MirrorResult | null>;

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

  // -------------------------------------------------------------------------
  // Config
  // -------------------------------------------------------------------------

  /** The persisted application config, or null if not yet fetched. */
  readonly config: Config | null;

  /** Whether a config fetch is in progress. */
  readonly configLoading: boolean;

  /** Error from the last config operation, if any. */
  readonly configError: string | null;

  /** Active Monaco editor theme ID, loaded from config. Defaults to "vs-dark". */
  readonly editorTheme: string;

  /** Fetch configuration from the backend. */
  fetchConfig: () => Promise<void>;

  /** Save configuration to the backend. */
  saveConfig: (config: Config) => Promise<void>;

  // -------------------------------------------------------------------------
  // Kickoff
  // -------------------------------------------------------------------------

  /** Whether a kickoff operation is in progress. */
  readonly kickoffLoading: boolean;

  /** Result of the last kickoff run, if any. */
  readonly kickoffResult: KickoffResult | null;

  /** Run the kickoff flow for a Linear project. */
  runKickoff: (projectId: string, skipPlan: boolean) => Promise<void>;

  // -------------------------------------------------------------------------
  // Restack
  // -------------------------------------------------------------------------

  /** Restack a single PR onto its updated base. */
  restackPr: (pr: string) => Promise<void>;

  // -------------------------------------------------------------------------
  // Plan from path
  // -------------------------------------------------------------------------

  /** Load a plan document from a file path on disk. */
  loadPlanFromPath: (path: string, project: string) => Promise<void>;

  // -------------------------------------------------------------------------
  // GitHub PR import
  // -------------------------------------------------------------------------

  /** PRs authored by the current user. */
  readonly authoredPrs: readonly Review[];

  /** PRs where the current user is requested for review. */
  readonly reviewRequests: readonly Review[];

  /** Whether a GitHub PR fetch is in progress. */
  readonly prFetchLoading: boolean;

  /** Fetch PRs authored by the current user from GitHub. */
  fetchAuthoredPrs: () => Promise<void>;

  /** Fetch PRs where the current user is requested for review. */
  fetchReviewRequests: () => Promise<void>;
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
  config: null,
  configLoading: false,
  configError: null,
  editorTheme: "vs-dark",
  kickoffLoading: false,
  kickoffResult: null,
  authoredPrs: [],
  reviewRequests: [],
  prFetchLoading: false,

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

  navigateToDiff: async (pr: string) => {
    set({ loading: true, error: null });
    try {
      const diff = await invoke<DiffData>("get_review_diff", { pr });
      const reviews = get().reviews;
      const review = reviews.find((r) => r.pr === pr) ?? null;
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

  navigateToSettings: () => {
    set({ view: { kind: "settings" } });
    void get().fetchConfig();
  },

  navigateToKickoff: () => {
    set({ view: { kind: "kickoff" }, kickoffResult: null });
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

  mirrorComments: async (): Promise<MirrorResult | null> => {
    const { view } = get();
    if (view.kind !== "diff") return null;

    try {
      const result = await invoke<MirrorResult>("mirror_comments", {
        pr: view.pr,
      });
      return result;
    } catch (e: unknown) {
      set({ error: String(e) });
      return null;
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

  // -------------------------------------------------------------------------
  // Config
  // -------------------------------------------------------------------------

  fetchConfig: async () => {
    set({ configLoading: true, configError: null });
    try {
      const config = await invoke<Config>("get_config");
      const theme = config.app_theme ?? "dark";
      if (theme === "dark") {
        document.documentElement.classList.add("dark");
      } else {
        document.documentElement.classList.remove("dark");
      }
      set({
        config,
        configLoading: false,
        editorTheme: config.editor_theme ?? "vs-dark",
      });
    } catch (e: unknown) {
      set({ configError: String(e), configLoading: false });
    }
  },

  saveConfig: async (config: Config) => {
    set({ configLoading: true, configError: null });
    try {
      await invoke("save_config", { config });
      set({
        config,
        configLoading: false,
        editorTheme: config.editor_theme ?? "vs-dark",
      });
    } catch (e: unknown) {
      set({ configError: String(e), configLoading: false });
    }
  },

  // -------------------------------------------------------------------------
  // Kickoff
  // -------------------------------------------------------------------------

  runKickoff: async (projectId: string, skipPlan: boolean) => {
    set({ kickoffLoading: true, error: null, kickoffResult: null });
    try {
      const result = await invoke<KickoffResult>("kickoff", {
        projectId,
        skipPlan,
      });
      set({ kickoffLoading: false, kickoffResult: result });
      // Refresh reviews and frontier after kickoff completes.
      void get().fetchReviews();
      void get().fetchFrontier();
      void get().fetchPlan();
    } catch (e: unknown) {
      set({ error: String(e), kickoffLoading: false });
    }
  },

  // -------------------------------------------------------------------------
  // Restack
  // -------------------------------------------------------------------------

  restackPr: async (pr: string) => {
    set({ loading: true, error: null });
    try {
      const review = await invoke<Review>("restack_pr", { pr });
      // Update the review in-place in both lists.
      const reviews = get().reviews.map((r) => (r.pr === review.pr ? review : r));
      const frontier = get().frontier.map((r) => (r.pr === review.pr ? review : r));
      set({ reviews, frontier, loading: false });
    } catch (e: unknown) {
      set({ error: String(e), loading: false });
    }
  },

  // -------------------------------------------------------------------------
  // Plan from path
  // -------------------------------------------------------------------------

  loadPlanFromPath: async (path: string, project: string) => {
    set({ loading: true, error: null });
    try {
      const plan = await invoke<ProjectPlan>("load_plan_from_path", {
        path,
        project,
      });
      set({ plan, loading: false });
    } catch (e: unknown) {
      set({ error: String(e), loading: false });
    }
  },

  // -------------------------------------------------------------------------
  // GitHub PR import
  // -------------------------------------------------------------------------

  fetchAuthoredPrs: async () => {
    set({ prFetchLoading: true, error: null });
    try {
      const prs = await invoke<Review[]>("fetch_authored_prs");
      set({ authoredPrs: prs, prFetchLoading: false });
    } catch (e: unknown) {
      set({ error: String(e), prFetchLoading: false });
    }
  },

  fetchReviewRequests: async () => {
    set({ prFetchLoading: true, error: null });
    try {
      const prs = await invoke<Review[]>("fetch_review_requests");
      set({ reviewRequests: prs, prFetchLoading: false });
    } catch (e: unknown) {
      set({ error: String(e), prFetchLoading: false });
    }
  },
}));

export type { ViewState };
