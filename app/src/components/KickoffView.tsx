/**
 * Kickoff flow view — starts a new project from a Linear project ID.
 *
 * Lets the user enter a project ID (pre-filled from config), toggle the
 * plan gate, and launch the kickoff. Displays loading state and results.
 */

import { useState, useEffect, useCallback } from "react";
import { useAppStore } from "../store";

export function KickoffView() {
  const config = useAppStore((s) => s.config);
  const error = useAppStore((s) => s.error);
  const kickoffLoading = useAppStore((s) => s.kickoffLoading);
  const kickoffResult = useAppStore((s) => s.kickoffResult);
  const runKickoff = useAppStore((s) => s.runKickoff);
  const fetchConfig = useAppStore((s) => s.fetchConfig);

  const [projectId, setProjectId] = useState("");
  const [skipPlan, setSkipPlan] = useState(false);

  // Pre-fill project ID from config if available.
  useEffect(() => {
    void fetchConfig();
  }, [fetchConfig]);

  useEffect(() => {
    if (config !== null && projectId === "") {
      setProjectId(config.linear_project_id ?? "");
    }
  }, [config, projectId]);

  const handleKickoff = useCallback(() => {
    if (projectId.trim() === "") return;
    void runKickoff(projectId.trim(), skipPlan);
  }, [projectId, skipPlan, runKickoff]);

  return (
    <div className="mx-auto max-w-2xl px-6 py-8">
      <h1 className="mb-2 text-xl font-semibold text-text-primary">
        Kickoff
      </h1>
      <p className="mb-6 text-sm text-text-secondary">
        Fetch issues from Linear, compute the frontier, and create reviews.
      </p>

      {/* Error banner */}
      {error !== null && (
        <div className="mb-6 rounded-lg border border-danger bg-danger/10 px-4 py-3 text-sm text-danger">
          {error}
        </div>
      )}

      <div className="rounded-lg border border-border bg-surface-1 p-6">
        <form
          onSubmit={(e) => {
            e.preventDefault();
            handleKickoff();
          }}
          className="flex flex-col gap-5"
        >
          {/* Project ID */}
          <label className="flex flex-col gap-1.5">
            <span className="text-sm font-medium text-text-secondary">
              Linear Project ID
            </span>
            <input
              type="text"
              value={projectId}
              onChange={(e) => {
                setProjectId(e.target.value);
              }}
              placeholder="PRJ-123"
              disabled={kickoffLoading}
              className="rounded-md border border-border bg-surface-2 px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-50"
            />
          </label>

          {/* Skip Plan Gate checkbox */}
          <label className="flex items-center gap-3">
            <input
              type="checkbox"
              checked={skipPlan}
              onChange={(e) => {
                setSkipPlan(e.target.checked);
              }}
              disabled={kickoffLoading}
              className="h-4 w-4 rounded border-border bg-surface-2 accent-accent"
            />
            <span className="text-sm text-text-secondary">
              Skip Plan Gate
            </span>
          </label>

          {/* Kick Off button */}
          <div className="flex items-center gap-3 pt-2">
            <button
              type="submit"
              disabled={kickoffLoading || projectId.trim() === ""}
              className="rounded-md bg-accent px-5 py-2 text-sm font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
            >
              {kickoffLoading ? "Running..." : "Kick Off"}
            </button>
            {kickoffLoading && (
              <Spinner />
            )}
          </div>
        </form>
      </div>

      {/* Result display */}
      {kickoffResult !== null && (
        <div className="mt-6 rounded-lg border border-border bg-surface-1 p-6">
          <h2 className="mb-4 text-base font-semibold text-text-primary">
            Kickoff Complete
          </h2>
          <dl className="grid grid-cols-2 gap-x-6 gap-y-3 text-sm">
            <dt className="text-text-secondary">Issues fetched</dt>
            <dd className="font-medium text-text-primary">
              {kickoffResult.issue_count}
            </dd>

            <dt className="text-text-secondary">Frontier issues</dt>
            <dd className="font-medium text-text-primary">
              {kickoffResult.frontier.length}
            </dd>

            <dt className="text-text-secondary">Reviews created</dt>
            <dd className="font-medium text-text-primary">
              {kickoffResult.reviews.length}
            </dd>

            <dt className="text-text-secondary">Plan</dt>
            <dd className="font-medium text-text-primary">
              {kickoffResult.plan !== null ? "Created" : "Skipped"}
            </dd>
          </dl>

          {kickoffResult.reviews.length > 0 && (
            <div className="mt-4 border-t border-border pt-4">
              <h3 className="mb-2 text-sm font-medium text-text-secondary">
                Created Reviews
              </h3>
              <ul className="flex flex-col gap-1">
                {kickoffResult.reviews.map((review) => (
                  <li
                    key={review.id}
                    className="rounded-md bg-surface-2 px-3 py-2 text-sm text-text-primary"
                  >
                    <span className="font-medium">{review.pr}</span>
                    <span className="ml-2 text-text-muted">
                      {review.branch}
                    </span>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Spinner (CSS-only via Tailwind animation)
// ---------------------------------------------------------------------------

function Spinner() {
  return (
    <svg
      className="h-5 w-5 animate-spin text-accent"
      viewBox="0 0 24 24"
      fill="none"
    >
      <circle
        className="opacity-25"
        cx="12"
        cy="12"
        r="10"
        stroke="currentColor"
        strokeWidth="4"
      />
      <path
        className="opacity-75"
        fill="currentColor"
        d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"
      />
    </svg>
  );
}
