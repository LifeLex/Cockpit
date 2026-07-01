import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { Review } from "./bindings/Review";
import type { DiffData } from "./bindings/DiffData";
import type { MirrorResult } from "./bindings/MirrorResult";
import type { ProjectPlan } from "./bindings/ProjectPlan";
import type { Config } from "./bindings/Config";
import type { KickoffResult } from "./bindings/KickoffResult";
import type { Project } from "./bindings/Project";
import type { BatchStatus } from "./bindings/BatchStatus";
import type { Skill } from "./bindings/Skill";
import type { SyncReport } from "./bindings/SyncReport";
import type { AgentMode } from "./bindings/AgentMode";
import type { CiCheck } from "./bindings/CiCheck";

/**
 * Navigation state discriminated union.
 *
 * Top-level views mirror the sidebar: PRs (default), Projects, Skills, Agents,
 * and Settings, plus the drill-in views (diff, plan, new-project). Using a
 * tagged union (not optional fields) makes exhaustive switching possible.
 */
type ViewState =
  | { readonly kind: "prs" }
  | { readonly kind: "diff"; readonly pr: string }
  | { readonly kind: "plan" }
  | { readonly kind: "projects" }
  | { readonly kind: "new-project" }
  | { readonly kind: "skills" }
  | { readonly kind: "agents" }
  | { readonly kind: "settings" };

interface AppStore {
  readonly reviews: readonly Review[];
  readonly frontier: readonly Review[];
  readonly plan: ProjectPlan | null;
  readonly loading: boolean;
  readonly error: string | null;

  /** Dismiss the current error. */
  clearError: () => void;

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

  /** Navigate to the PRs list (the default view). */
  navigateToPrs: () => void;

  /** Navigate to the Projects list. */
  navigateToProjects: () => void;

  /** Navigate to the New Project flow. */
  navigateToNewProject: () => void;

  /** Navigate to the Skills view. */
  navigateToSkills: () => void;

  /** Navigate to the Agents view. */
  navigateToAgents: () => void;

  /** Navigate to the settings view. */
  navigateToSettings: () => void;

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

  /** Request changes on the project plan (spawns the plan agent). */
  planRequestChanges: () => Promise<void>;

  /** Approve the project plan (explicit user action; fans out the batch). */
  planApprove: () => Promise<void>;

  /** Open the plan for review (`Pending | Reworked` -> `InReview`). */
  openPlan: () => Promise<void>;

  /**
   * Generate the plan document via the plan agent for the loaded plan (or an
   * explicit project, if wired later by the caller).
   */
  generatePlan: (projectId?: string) => Promise<void>;

  /** Approve a single review by PR ref (explicit user action). */
  approveReview: (pr: string) => Promise<void>;

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
  // Projects
  // -------------------------------------------------------------------------

  /** All first-class projects. */
  readonly projects: readonly Project[];

  /** Whether a project operation is in progress. */
  readonly projectsLoading: boolean;

  /** Fetch the list of projects from the backend. */
  listProjects: () => Promise<void>;

  /** Create a new ad-hoc project with the given name (explicit user action). */
  createProject: (name: string) => Promise<Project | null>;

  /**
   * Create a project from a Linear project by running the kickoff import
   * (Linear is one optional source, not the entry point).
   */
  createProjectFromLinear: (
    projectId: string,
    skipPlan: boolean,
  ) => Promise<void>;

  /** Attach an existing review (by PR ref) to a project. */
  attachReview: (pr: string, projectId: string) => Promise<void>;

  /** Fetch aggregate batch progress for a project (or all reviews). */
  batchStatus: (projectId?: string) => Promise<BatchStatus | null>;

  // -------------------------------------------------------------------------
  // Kickoff (Linear import)
  // -------------------------------------------------------------------------

  /** Whether a kickoff operation is in progress. */
  readonly kickoffLoading: boolean;

  /** Result of the last kickoff run, if any. */
  readonly kickoffResult: KickoffResult | null;

  // -------------------------------------------------------------------------
  // Skills
  // -------------------------------------------------------------------------

  /** All locally-known skills. */
  readonly skills: readonly Skill[];

  /** Whether a skills operation is in progress. */
  readonly skillsLoading: boolean;

  /** Fetch the list of skills from the backend. */
  listSkills: () => Promise<void>;

  /** Create or overwrite a skill file with the given contents. */
  saveSkill: (name: string, contents: string) => Promise<void>;

  /** Delete a skill by name. */
  deleteSkill: (name: string) => Promise<void>;

  /** Sync skills from the configured GitHub source. */
  syncSkills: () => Promise<SyncReport | null>;

  // -------------------------------------------------------------------------
  // Agent prompts
  // -------------------------------------------------------------------------

  /** Fetch the custom prompt override for a mode (null if none set). */
  getAgentPrompt: (mode: AgentMode) => Promise<string | null>;

  /** Fetch the builtin prompt fragment for a mode. */
  getBuiltinAgentPrompt: (mode: AgentMode) => Promise<string | null>;

  /** Save (or clear, via empty text) the custom prompt override for a mode. */
  saveAgentPrompt: (mode: AgentMode, text: string) => Promise<void>;

  // -------------------------------------------------------------------------
  // CI (best-effort UI queries; never block the loop)
  // -------------------------------------------------------------------------

  /** List the CI checks for a PR (empty on gh error). */
  listCiChecks: (pr: string) => Promise<CiCheck[]>;

  /**
   * Fetch the failed-job logs for a single CI run of a PR, identified by a
   * check `link` (empty string on gh error). Used for per-pipeline logs.
   */
  ciRunLogsByLink: (pr: string, link: string) => Promise<string>;

  /**
   * Dispatch the Fix loop to address a PR's CI failures (explicit user action,
   * Invariant 5). Transitions the review to Dispatched and spawns the fixer.
   */
  fixCi: (pr: string) => Promise<void>;

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
  clearError: () => {
    set({ error: null });
  },
  view: { kind: "prs" },
  activeReview: null,
  activeDiff: null,
  config: null,
  configLoading: false,
  configError: null,
  editorTheme: "vs-dark",
  projects: [],
  projectsLoading: false,
  kickoffLoading: false,
  kickoffResult: null,
  skills: [],
  skillsLoading: false,
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
      const replace = (r: Review) => (r.pr === review.pr ? review : r);
      set({
        view: { kind: "diff", pr },
        activeReview: review,
        activeDiff: diff,
        loading: false,
        authoredPrs: get().authoredPrs.map(replace),
        reviewRequests: get().reviewRequests.map(replace),
        frontier: get().frontier.map(replace),
        reviews: get().reviews.map(replace),
      });
    } catch (e: unknown) {
      set({ error: String(e), loading: false });
    }
  },

  navigateToDiff: async (pr: string) => {
    set({ loading: true, error: null });
    try {
      const [review, diff] = await Promise.all([
        invoke<Review>("get_review", { pr }),
        invoke<DiffData>("get_review_diff", { pr }),
      ]);
      const replace = (r: Review) => (r.pr === review.pr ? review : r);
      set({
        view: { kind: "diff", pr },
        activeReview: review,
        activeDiff: diff,
        loading: false,
        authoredPrs: get().authoredPrs.map(replace),
        reviewRequests: get().reviewRequests.map(replace),
        frontier: get().frontier.map(replace),
        reviews: get().reviews.map(replace),
      });
    } catch (e: unknown) {
      set({ error: String(e), loading: false });
    }
  },

  navigateToPlan: () => {
    set({ view: { kind: "plan" } });
    void get().fetchPlan();
  },

  navigateToPrs: () => {
    set({
      view: { kind: "prs" },
      activeReview: null,
      activeDiff: null,
    });
    void get().fetchReviews();
    void get().fetchFrontier();
    void get().fetchAuthoredPrs();
  },

  navigateToProjects: () => {
    set({ view: { kind: "projects" } });
    void get().listProjects();
  },

  navigateToNewProject: () => {
    set({ view: { kind: "new-project" }, kickoffResult: null });
  },

  navigateToSkills: () => {
    set({ view: { kind: "skills" } });
    void get().listSkills();
  },

  navigateToAgents: () => {
    set({ view: { kind: "agents" } });
  },

  navigateToSettings: () => {
    set({ view: { kind: "settings" } });
    void get().fetchConfig();
  },

  addComment: async (
    file: string,
    lineStart: number,
    lineEnd: number,
    body: string,
  ) => {
    const { view } = get();
    if (view.kind !== "diff") return;

    const review = await invoke<Review>("add_comment", {
      pr: view.pr,
      file,
      lineStart,
      lineEnd,
      body,
    });
    const replace = (r: Review) => (r.pr === review.pr ? review : r);
    set({
      activeReview: review,
      authoredPrs: get().authoredPrs.map(replace),
      reviewRequests: get().reviewRequests.map(replace),
      frontier: get().frontier.map(replace),
      reviews: get().reviews.map(replace),
    });
  },

  requestChanges: async () => {
    const { view } = get();
    if (view.kind !== "diff") return;

    try {
      const review = await invoke<Review>("request_changes", {
        pr: view.pr,
      });
      const replace = (r: Review) => (r.pr === review.pr ? review : r);
      set({
        activeReview: review,
        authoredPrs: get().authoredPrs.map(replace),
        reviewRequests: get().reviewRequests.map(replace),
        frontier: get().frontier.map(replace),
        reviews: get().reviews.map(replace),
      });
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
      const replace = (r: Review) => (r.pr === review.pr ? review : r);
      set({
        activeReview: review,
        activeDiff: diff,
        authoredPrs: get().authoredPrs.map(replace),
        reviewRequests: get().reviewRequests.map(replace),
        frontier: get().frontier.map(replace),
        reviews: get().reviews.map(replace),
      });
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

  planRequestChanges: async () => {
    try {
      const plan = await invoke<ProjectPlan>("plan_request_changes");
      set({ plan });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  planApprove: async () => {
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

  generatePlan: async (_projectId?: string) => {
    set({ loading: true, error: null });
    try {
      const plan = await invoke<ProjectPlan>("generate_plan");
      set({ plan, loading: false });
    } catch (e: unknown) {
      set({ error: String(e), loading: false });
    }
  },

  approveReview: async (pr: string) => {
    try {
      await invoke<Review>("approve_review", { pr });
      await get().fetchFrontier();
      await get().fetchReviews();
    } catch (e: unknown) {
      set({ error: String(e) });
    }
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
  // Projects
  // -------------------------------------------------------------------------

  listProjects: async () => {
    set({ projectsLoading: true, error: null });
    try {
      const projects = await invoke<Project[]>("list_projects");
      set({ projects, projectsLoading: false });
    } catch (e: unknown) {
      set({ error: String(e), projectsLoading: false });
    }
  },

  createProject: async (name: string): Promise<Project | null> => {
    set({ projectsLoading: true, error: null });
    try {
      const project = await invoke<Project>("create_project", { name });
      set({ projectsLoading: false });
      await get().listProjects();
      return project;
    } catch (e: unknown) {
      set({ error: String(e), projectsLoading: false });
      return null;
    }
  },

  createProjectFromLinear: async (projectId: string, skipPlan: boolean) => {
    set({ kickoffLoading: true, error: null, kickoffResult: null });
    try {
      const result = await invoke<KickoffResult>("kickoff", {
        projectId,
        skipPlan,
      });
      set({ kickoffLoading: false, kickoffResult: result });
      void get().fetchReviews();
      void get().fetchFrontier();
      void get().fetchPlan();
      void get().listProjects();
    } catch (e: unknown) {
      set({ error: String(e), kickoffLoading: false });
    }
  },

  attachReview: async (pr: string, projectId: string) => {
    try {
      const review = await invoke<Review>("attach_review", {
        pr,
        projectId,
      });
      const replace = (r: Review) => (r.pr === review.pr ? review : r);
      set({
        authoredPrs: get().authoredPrs.map(replace),
        reviewRequests: get().reviewRequests.map(replace),
        frontier: get().frontier.map(replace),
        reviews: get().reviews.map(replace),
      });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  batchStatus: async (projectId?: string): Promise<BatchStatus | null> => {
    try {
      const status = await invoke<BatchStatus>("batch_status", {
        projectId: projectId ?? null,
      });
      return status;
    } catch (e: unknown) {
      set({ error: String(e) });
      return null;
    }
  },

  // -------------------------------------------------------------------------
  // Skills
  // -------------------------------------------------------------------------

  listSkills: async () => {
    set({ skillsLoading: true, error: null });
    try {
      const skills = await invoke<Skill[]>("list_skills");
      set({ skills, skillsLoading: false });
    } catch (e: unknown) {
      set({ error: String(e), skillsLoading: false });
    }
  },

  saveSkill: async (name: string, contents: string) => {
    set({ skillsLoading: true, error: null });
    try {
      await invoke("save_skill", { name, contents });
      set({ skillsLoading: false });
      await get().listSkills();
    } catch (e: unknown) {
      set({ error: String(e), skillsLoading: false });
    }
  },

  deleteSkill: async (name: string) => {
    set({ skillsLoading: true, error: null });
    try {
      await invoke("delete_skill", { name });
      set({ skillsLoading: false });
      await get().listSkills();
    } catch (e: unknown) {
      set({ error: String(e), skillsLoading: false });
    }
  },

  syncSkills: async (): Promise<SyncReport | null> => {
    set({ skillsLoading: true, error: null });
    try {
      const report = await invoke<SyncReport>("sync_skills");
      set({ skillsLoading: false });
      await get().listSkills();
      return report;
    } catch (e: unknown) {
      set({ error: String(e), skillsLoading: false });
      return null;
    }
  },

  // -------------------------------------------------------------------------
  // Agent prompts
  // -------------------------------------------------------------------------

  getAgentPrompt: async (mode: AgentMode): Promise<string | null> => {
    try {
      return await invoke<string | null>("get_agent_prompt", { mode });
    } catch (e: unknown) {
      set({ error: String(e) });
      return null;
    }
  },

  getBuiltinAgentPrompt: async (mode: AgentMode): Promise<string | null> => {
    try {
      return await invoke<string>("get_builtin_agent_prompt", { mode });
    } catch (e: unknown) {
      set({ error: String(e) });
      return null;
    }
  },

  saveAgentPrompt: async (mode: AgentMode, text: string) => {
    try {
      await invoke("save_agent_prompt", { mode, text });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  // -------------------------------------------------------------------------
  // CI (best-effort UI queries; never block the loop)
  // -------------------------------------------------------------------------

  listCiChecks: async (pr: string): Promise<CiCheck[]> => {
    try {
      return await invoke<CiCheck[]>("list_ci_checks", { pr });
    } catch (e: unknown) {
      // Non-fatal (Invariant 1): a CI query never blocks the loop.
      console.error("list_ci_checks failed", e);
      return [];
    }
  },

  ciRunLogsByLink: async (pr: string, link: string): Promise<string> => {
    try {
      return await invoke<string>("ci_run_logs_by_link", { pr, link });
    } catch (e: unknown) {
      // Non-fatal (Invariant 1): a CI query never blocks the loop.
      console.error("ci_run_logs_by_link failed", e);
      return "";
    }
  },

  fixCi: async (pr: string) => {
    try {
      const review = await invoke<Review>("fix_ci", { pr });
      const replace = (r: Review) => (r.pr === review.pr ? review : r);
      set({
        activeReview: review,
        authoredPrs: get().authoredPrs.map(replace),
        reviewRequests: get().reviewRequests.map(replace),
        frontier: get().frontier.map(replace),
        reviews: get().reviews.map(replace),
      });
    } catch (e: unknown) {
      set({ error: String(e) });
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
