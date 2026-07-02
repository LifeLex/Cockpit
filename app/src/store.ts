import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { Review } from "./bindings/Review";
import type { DiffData } from "./bindings/DiffData";
import type { MirrorResult } from "./bindings/MirrorResult";
import type { SubmitReviewResult } from "./bindings/SubmitReviewResult";
import type { ReviewEvent } from "./bindings/ReviewEvent";
import type { ProjectPlan } from "./bindings/ProjectPlan";
import type { Config } from "./bindings/Config";
import type { KickoffResult } from "./bindings/KickoffResult";
import type { Project } from "./bindings/Project";
import type { ProjectId } from "./bindings/ProjectId";
import type { BatchStatus } from "./bindings/BatchStatus";
import type { Skill } from "./bindings/Skill";
import type { SyncReport } from "./bindings/SyncReport";
import type { AgentMode } from "./bindings/AgentMode";
import type { CiCheck } from "./bindings/CiCheck";
import type { DiffSide } from "./bindings/DiffSide";
import type { EvidenceSummary } from "./bindings/EvidenceSummary";
import type { FilePair } from "./bindings/FilePair";
import type { TrajectorySummary } from "./bindings/TrajectorySummary";

/**
 * Live progress of a whole-stack restack (D3), mirroring the backend's
 * `restack-progress` event payload (hand-typed; no ts-rs binding). `current` /
 * `total` are 1-based over the dependency-ordered descendants; `status` is
 * `"restacking"` before a child, its outcome afterwards, and `"done"` once the
 * sequence completes cleanly.
 */
export interface RestackProgress {
  /** PR ref of the stack root the run was requested for. */
  readonly root_pr: string;
  /** 1-based index of the child currently being restacked (0 on a load error). */
  readonly current: number;
  /** Total number of descendants in the restack order. */
  readonly total: number;
  /** PR ref of the child this event is about (empty on the final `"done"`). */
  readonly current_pr: string;
  /** Phase of the run. */
  readonly status: "restacking" | "clean" | "conflict" | "done" | "error";
  /**
   * Human-readable reason for a halt (currently only the TOCTOU agent-guard
   * `"error"`). Optional — omitted from the wire when the backend has none.
   */
  readonly detail?: string;
}

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
  | { readonly kind: "plan"; readonly project: ProjectId }
  | { readonly kind: "projects" }
  | { readonly kind: "new-project" }
  | { readonly kind: "skills" }
  | { readonly kind: "agents" }
  | { readonly kind: "settings" };

interface AppStore {
  readonly reviews: readonly Review[];
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
  openReview: (pr: string) => Promise<void>;

  /** Navigate to the diff view for a specific PR. */
  navigateToDiff: (pr: string) => Promise<void>;

  /** Navigate to a project's plan gate. */
  navigateToPlan: (projectId: ProjectId) => void;

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

  /**
   * Add an anchored comment to the active review. `side` selects which side of
   * the diff the line refers to (D12).
   */
  addComment: (
    file: string,
    lineStart: number,
    lineEnd: number,
    body: string,
    side: DiffSide,
  ) => Promise<void>;

  /** Request changes on the active review (InReview -> Dispatched). */
  requestChanges: () => Promise<void>;

  /** Mirror local comments for the active review to GitHub (explicit user action). */
  mirrorComments: () => Promise<MirrorResult | null>;

  /** Refresh the active review to pick up state changes. */
  refreshActiveReview: () => Promise<void>;

  /** Fetch the plan for a specific project into the store. */
  fetchPlan: (projectId: ProjectId) => Promise<void>;

  /** Add a comment to a project's plan (anchored to a step or file). */
  addPlanComment: (
    projectId: ProjectId,
    anchor: string,
    body: string,
  ) => Promise<void>;

  /** Request changes on a project's plan (spawns the plan agent). */
  planRequestChanges: (projectId: ProjectId) => Promise<void>;

  /** Approve a project's plan (explicit user action; fans out the batch). */
  planApprove: (projectId: ProjectId) => Promise<void>;

  /** Open a project's plan for review (`Pending | Reworked` -> `InReview`). */
  planOpen: (projectId: ProjectId) => Promise<void>;

  /** Generate the plan document via the plan agent for a project. */
  generatePlan: (projectId: ProjectId) => Promise<void>;

  /** Approve a single review by PR ref (explicit user action; `InReview` -> `Approved`). */
  approveReview: (pr: string) => Promise<void>;

  /**
   * Merge an approved review's PR (explicit, confirmed user action; Invariant 5).
   *
   * Squash-merges on GitHub and deletes the branch, advancing the local gate to
   * `Merged`. Failure is surfaced via the store `error` and never blocks the loop.
   */
  mergeReview: (pr: string) => Promise<void>;

  /**
   * Submit a real GitHub PR review (approve / request changes / comment) carrying
   * the review's inline Local comments (explicit, confirmed user action;
   * Invariant 5 / §9). Returns the [`SubmitReviewResult`] so the caller can show
   * the submitted count; on partial success (non-empty `skipped`) the store
   * `error` is set listing the skipped comments' reasons. Returns `null` on
   * failure (also surfaced via `error`).
   */
  submitGithubReview: (
    pr: string,
    event: ReviewEvent,
    body: string,
  ) => Promise<SubmitReviewResult | null>;

  /**
   * Fetch the interdiff (changes since the last review dispatch) for a PR.
   *
   * Returns `null` on failure, setting the store `error`; the caller then falls
   * back to the full diff (D10). Requires a dispatch snapshot server-side.
   */
  fetchInterdiff: (pr: string) => Promise<DiffData | null>;

  /**
   * Restack a stale review onto its parent's new head (explicit user action).
   *
   * A clean rebase clears the stale flag; on conflict the backend spawns the
   * conflict-resolver agent and returns the review with an active agent run.
   * Failure is non-fatal: it sets the store `error` and never blocks the loop.
   */
  restackPr: (pr: string) => Promise<void>;

  /**
   * Kill the agent running on a review (explicit user action; D11/D12).
   *
   * Applies the returned reconciled [`Review`] (comments preserved, agent
   * cleared, back to `InReview`). Failure is non-fatal: it sets the store
   * `error` and never blocks the loop.
   */
  killAgent: (pr: string) => Promise<void>;

  /**
   * Fetch the persisted trajectory summary for a PR's last agent run (D2).
   *
   * Best-effort: returns `null` and logs on failure rather than setting the
   * blocking `error` — the summary is a glance aid ("what did the agent try?"),
   * never a gate (Invariant 1). `null` also when no run has been recorded.
   */
  fetchTrajectorySummary: (pr: string) => Promise<TrajectorySummary | null>;

  /**
   * Restack a whole stack rooted at `rootPr` in dependency order (explicit user
   * action; D3). The command returns immediately and drives the sequence on a
   * background task, emitting `restack-progress` events. Failure is non-fatal:
   * it sets the store `error` and never blocks the loop.
   */
  restackStack: (rootPr: string) => Promise<void>;

  /**
   * Latest whole-stack restack progress keyed by the stack root's PR ref (D3).
   *
   * Fed from the `restack-progress` Tauri listener (owned by `App`) via
   * {@link applyRestackProgress}; read by `StackContainer` to show a live
   * progress line / conflict note in place of the restack button.
   */
  readonly restackProgress: Readonly<Record<string, RestackProgress>>;

  /** Merge a `restack-progress` event into {@link restackProgress} by root (D3). */
  applyRestackProgress: (progress: RestackProgress) => void;

  /**
   * Drop the recorded progress for a stack root (D3).
   *
   * Called when the user launches a fresh restack for `rootPr` so a terminal
   * `conflict`/`error` entry from a previous run does not linger under the new
   * sequence's live progress.
   */
  clearRestackProgress: (rootPr: string) => void;

  /**
   * Ensure a review has a checked-out worktree on disk and return its path
   * (D12). Materializes a dedicated branch worktree for an imported same-repo
   * PR; a no-op for cockpit-managed worktrees. Returns `null` on failure
   * (also surfaced via `error`) so callers can fall back to the review's
   * recorded worktree.
   */
  ensureReviewWorktree: (pr: string) => Promise<string | null>;

  // -------------------------------------------------------------------------
  // Diff-gate evidence (Phase B; best-effort UI reads, never block the loop)
  // -------------------------------------------------------------------------

  /**
   * Fetch the review-time evidence bundle (diff signals, CI rollup, agent
   * commands) for a PR (B1). Best-effort: returns `null` and logs on failure
   * rather than setting the blocking `error` — evidence is a glance aid and
   * never blocks the loop (Invariant 1).
   */
  fetchEvidence: (pr: string) => Promise<EvidenceSummary | null>;

  /**
   * Run the advisory read-only pre-pass reviewer for a PR (B2). Applies the
   * returned [`Review`] (which now carries a running Review agent) across every
   * list. Non-fatal: a spawn failure sets `error` and never blocks the loop.
   */
  preReview: (pr: string) => Promise<void>;

  /**
   * Per `pr:path:head_sha` cache of resolved full-file pairs (B4). Populated by
   * {@link fetchFilePair}; keyed on the head SHA so a force-push naturally
   * misses and refetches. Not reactive — read only through {@link fetchFilePair}.
   */
  readonly filePairCache: Map<string, FilePair>;

  /**
   * Fetch the full text of a file on both sides of a review's diff for the
   * full-file view (B4), memoized in {@link filePairCache}. Best-effort:
   * returns `null` and logs on failure (never blocks the loop, Invariant 1).
   */
  fetchFilePair: (pr: string, path: string) => Promise<FilePair | null>;

  // -------------------------------------------------------------------------
  // Config
  // -------------------------------------------------------------------------

  /** The persisted application config, or null if not yet fetched. */
  readonly config: Config | null;

  /** Whether a config fetch is in progress. */
  readonly configLoading: boolean;

  /** Error from the last config operation, if any. */
  readonly configError: string | null;

  /** Active Monaco editor theme ID, loaded from config. Defaults to "glass-cockpit". */
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

  /** Fetch per-project batch progress. */
  batchStatus: (projectId: ProjectId) => Promise<BatchStatus | null>;

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
  editorTheme: "glass-cockpit",
  projects: [],
  projectsLoading: false,
  kickoffLoading: false,
  kickoffResult: null,
  skills: [],
  skillsLoading: false,
  authoredPrs: [],
  reviewRequests: [],
  prFetchLoading: false,
  filePairCache: new Map<string, FilePair>(),
  restackProgress: {},

  fetchReviews: async () => {
    set({ loading: true, error: null });
    try {
      const reviews = await invoke<Review[]>("list_reviews");
      set({ reviews, loading: false });
    } catch (e: unknown) {
      set({ error: String(e), loading: false });
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
        reviews: get().reviews.map(replace),
      });
    } catch (e: unknown) {
      set({ error: String(e), loading: false });
    }
  },

  navigateToPlan: (projectId: ProjectId) => {
    set({ view: { kind: "plan", project: projectId } });
    void get().fetchPlan(projectId);
  },

  navigateToPrs: () => {
    set({
      view: { kind: "prs" },
      activeReview: null,
      activeDiff: null,
    });
    void get().fetchReviews();
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
    side: DiffSide,
  ) => {
    const { view } = get();
    if (view.kind !== "diff") return;

    const review = await invoke<Review>("add_comment", {
      pr: view.pr,
      file,
      lineStart,
      lineEnd,
      body,
      side,
    });
    const replace = (r: Review) => (r.pr === review.pr ? review : r);
    set({
      activeReview: review,
      authoredPrs: get().authoredPrs.map(replace),
      reviewRequests: get().reviewRequests.map(replace),
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
        reviews: get().reviews.map(replace),
      });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  fetchPlan: async (projectId: ProjectId) => {
    try {
      const plan = await invoke<ProjectPlan | null>("get_plan", {
        projectId,
      });
      set({ plan });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  addPlanComment: async (projectId: ProjectId, anchor: string, body: string) => {
    try {
      const plan = await invoke<ProjectPlan>("add_plan_comment", {
        projectId,
        anchor,
        body,
      });
      set({ plan });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  planRequestChanges: async (projectId: ProjectId) => {
    try {
      const plan = await invoke<ProjectPlan>("plan_request_changes", {
        projectId,
      });
      set({ plan });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  planApprove: async (projectId: ProjectId) => {
    try {
      const plan = await invoke<ProjectPlan>("plan_approve", { projectId });
      set({ plan });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  planOpen: async (projectId: ProjectId) => {
    try {
      const plan = await invoke<ProjectPlan>("plan_open", { projectId });
      set({ plan });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  generatePlan: async (projectId: ProjectId) => {
    set({ loading: true, error: null });
    try {
      const plan = await invoke<ProjectPlan>("generate_plan", { projectId });
      set({ plan, loading: false });
    } catch (e: unknown) {
      set({ error: String(e), loading: false });
    }
  },

  approveReview: async (pr: string) => {
    try {
      const review = await invoke<Review>("approve_review", { pr });
      const replace = (r: Review) => (r.pr === review.pr ? review : r);
      set({
        activeReview:
          get().activeReview?.pr === review.pr ? review : get().activeReview,
        authoredPrs: get().authoredPrs.map(replace),
        reviewRequests: get().reviewRequests.map(replace),
        reviews: get().reviews.map(replace),
      });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  mergeReview: async (pr: string) => {
    try {
      const review = await invoke<Review>("merge_review", { pr });
      const replace = (r: Review) => (r.pr === review.pr ? review : r);
      set({
        activeReview:
          get().activeReview?.pr === review.pr ? review : get().activeReview,
        authoredPrs: get().authoredPrs.map(replace),
        reviewRequests: get().reviewRequests.map(replace),
        reviews: get().reviews.map(replace),
      });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  submitGithubReview: async (
    pr: string,
    event: ReviewEvent,
    body: string,
  ): Promise<SubmitReviewResult | null> => {
    try {
      const result = await invoke<SubmitReviewResult>("submit_github_review", {
        pr,
        event,
        // The command takes `Option<String>`; an empty body maps to `None`.
        body: body.trim() === "" ? null : body,
      });
      // The backend may clear submitted Local comments and/or advance the local
      // gate (Approve on a review-requested PR), so refresh the active review to
      // reflect that. Best-effort: a refresh failure is non-fatal.
      await get().refreshActiveReview();
      if (result.skipped.length > 0) {
        const reasons = result.skipped
          .map(([, reason]) => reason)
          .join("; ");
        set({
          error: `${String(result.skipped.length)} comment${
            result.skipped.length === 1 ? "" : "s"
          } skipped: ${reasons}`,
        });
      }
      return result;
    } catch (e: unknown) {
      set({ error: String(e) });
      return null;
    }
  },

  fetchInterdiff: async (pr: string): Promise<DiffData | null> => {
    try {
      return await invoke<DiffData>("get_interdiff", { pr });
    } catch (e: unknown) {
      set({ error: String(e) });
      return null;
    }
  },

  restackPr: async (pr: string) => {
    try {
      const review = await invoke<Review>("restack_pr", { pr });
      const replace = (r: Review) => (r.pr === review.pr ? review : r);
      set({
        activeReview:
          get().activeReview?.pr === review.pr ? review : get().activeReview,
        authoredPrs: get().authoredPrs.map(replace),
        reviewRequests: get().reviewRequests.map(replace),
        reviews: get().reviews.map(replace),
      });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  killAgent: async (pr: string) => {
    try {
      const review = await invoke<Review>("kill_agent", { pr });
      const replace = (r: Review) => (r.pr === review.pr ? review : r);
      set({
        activeReview:
          get().activeReview?.pr === review.pr ? review : get().activeReview,
        authoredPrs: get().authoredPrs.map(replace),
        reviewRequests: get().reviewRequests.map(replace),
        reviews: get().reviews.map(replace),
      });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  ensureReviewWorktree: async (pr: string): Promise<string | null> => {
    try {
      return await invoke<string>("ensure_review_worktree", { pr });
    } catch (e: unknown) {
      set({ error: String(e) });
      return null;
    }
  },

  fetchTrajectorySummary: async (
    pr: string,
  ): Promise<TrajectorySummary | null> => {
    try {
      return await invoke<TrajectorySummary | null>(
        "get_trajectory_summary",
        { pr },
      );
    } catch (e: unknown) {
      // Non-fatal (Invariant 1): the trajectory summary is a glance aid.
      console.error("get_trajectory_summary failed", e);
      return null;
    }
  },

  restackStack: async (rootPr: string) => {
    try {
      await invoke("restack_stack", { rootPr });
    } catch (e: unknown) {
      // Non-fatal (Invariant 1): a failed launch never blocks the loop.
      set({ error: String(e) });
    }
  },

  applyRestackProgress: (progress: RestackProgress) => {
    set({
      restackProgress: {
        ...get().restackProgress,
        [progress.root_pr]: progress,
      },
    });
  },

  clearRestackProgress: (rootPr: string) => {
    const next = { ...get().restackProgress };
    delete next[rootPr];
    set({ restackProgress: next });
  },

  // -------------------------------------------------------------------------
  // Diff-gate evidence (Phase B)
  // -------------------------------------------------------------------------

  fetchEvidence: async (pr: string): Promise<EvidenceSummary | null> => {
    try {
      return await invoke<EvidenceSummary>("get_evidence", { pr });
    } catch (e: unknown) {
      // Non-fatal (Invariant 1): evidence is a glance aid, never a gate.
      console.error("get_evidence failed", e);
      return null;
    }
  },

  preReview: async (pr: string) => {
    try {
      const review = await invoke<Review>("pre_review", { pr });
      const replace = (r: Review) => (r.pr === review.pr ? review : r);
      const active = get().activeReview;
      set({
        activeReview: active?.pr === review.pr ? review : active,
        authoredPrs: get().authoredPrs.map(replace),
        reviewRequests: get().reviewRequests.map(replace),
        reviews: get().reviews.map(replace),
      });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  fetchFilePair: async (
    pr: string,
    path: string,
  ): Promise<FilePair | null> => {
    // Key the cache on the review's head SHA so a force-push (new head) misses
    // and refetches instead of showing stale content.
    const known =
      get().activeReview?.pr === pr
        ? get().activeReview
        : ([
            ...get().reviews,
            ...get().authoredPrs,
            ...get().reviewRequests,
          ].find((r) => r.pr === pr) ?? null);
    const head = known?.head_sha ?? "";
    const key = `${pr}:${path}:${head}`;

    const cache = get().filePairCache;
    const cached = cache.get(key);
    if (cached !== undefined) return cached;

    try {
      const pair = await invoke<FilePair>("get_file_pair", { pr, path });
      cache.set(key, pair);
      return pair;
    } catch (e: unknown) {
      // Non-fatal (Invariant 1): the full-file view is optional; fall back to
      // the diff fragments rather than blocking.
      console.error("get_file_pair failed", e);
      return null;
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
        editorTheme: config.editor_theme ?? "glass-cockpit",
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
        editorTheme: config.editor_theme ?? "glass-cockpit",
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
        reviews: get().reviews.map(replace),
      });
    } catch (e: unknown) {
      set({ error: String(e) });
    }
  },

  batchStatus: async (projectId: ProjectId): Promise<BatchStatus | null> => {
    try {
      const status = await invoke<BatchStatus>("batch_status", {
        projectId,
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
      const active = get().activeReview;
      set({
        activeReview: active?.pr === review.pr ? review : active,
        authoredPrs: get().authoredPrs.map(replace),
        reviewRequests: get().reviewRequests.map(replace),
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
