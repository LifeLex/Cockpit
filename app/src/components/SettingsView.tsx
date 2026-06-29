/**
 * Settings pane for configuring Cockpit credentials, paths, and ports.
 *
 * Fetches the persisted Config on mount and renders an editable form.
 * The "Browse" button for the repo path uses Tauri's dialog plugin to
 * open a native directory picker.
 */

import { useState, useEffect, useCallback } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "../store";
import type { Config } from "../bindings/Config";

// ---------------------------------------------------------------------------
// Feedback union: discriminated on kind so the banner can style itself.
// ---------------------------------------------------------------------------

type Feedback =
  | { readonly kind: "success"; readonly message: string }
  | { readonly kind: "error"; readonly message: string };

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function SettingsView() {
  const config = useAppStore((s) => s.config);
  const configLoading = useAppStore((s) => s.configLoading);
  const configError = useAppStore((s) => s.configError);
  const fetchConfig = useAppStore((s) => s.fetchConfig);
  const saveConfig = useAppStore((s) => s.saveConfig);

  // Local form state — seeded from the store config once it arrives.
  const [linearApiKey, setLinearApiKey] = useState("");
  const [linearProjectId, setLinearProjectId] = useState("");
  const [repoPath, setRepoPath] = useState("");
  const [agentCommand, setAgentCommand] = useState("claude");
  const [hookPort, setHookPort] = useState(19876);
  const [showApiKey, setShowApiKey] = useState(false);
  const [feedback, setFeedback] = useState<Feedback | null>(null);

  // Fetch config on mount.
  useEffect(() => {
    void fetchConfig();
  }, [fetchConfig]);

  // Seed form fields when config arrives from the backend.
  useEffect(() => {
    if (config !== null) {
      setLinearApiKey(config.linear_api_key ?? "");
      setLinearProjectId(config.linear_project_id ?? "");
      setRepoPath(config.repo_path ?? "");
      setAgentCommand(config.agent_command);
      setHookPort(config.hook_port);
    }
  }, [config]);

  const handleBrowse = useCallback(async () => {
    const selected = await open({ directory: true });
    if (selected !== null) {
      setRepoPath(selected);
    }
  }, []);

  const handleSave = useCallback(async () => {
    setFeedback(null);
    const next: Config = {
      linear_api_key: linearApiKey || null,
      linear_project_id: linearProjectId || null,
      repo_path: repoPath || null,
      agent_command: agentCommand,
      hook_port: hookPort,
    };
    try {
      await saveConfig(next);
      setFeedback({ kind: "success", message: "Settings saved." });
    } catch (e: unknown) {
      setFeedback({ kind: "error", message: String(e) });
    }
  }, [linearApiKey, linearProjectId, repoPath, agentCommand, hookPort, saveConfig]);

  // Surface backend config-fetch errors as feedback.
  useEffect(() => {
    if (configError !== null) {
      setFeedback({ kind: "error", message: configError });
    }
  }, [configError]);

  return (
    <div className="mx-auto max-w-2xl px-6 py-8">
      <h1 className="mb-6 text-xl font-semibold text-text-primary">
        Settings
      </h1>

      {/* Feedback banner */}
      {feedback !== null && (
        <div
          className={[
            "mb-6 rounded-lg border px-4 py-3 text-sm",
            feedback.kind === "success"
              ? "border-success bg-success/10 text-success"
              : "border-danger bg-danger/10 text-danger",
          ].join(" ")}
        >
          {feedback.message}
        </div>
      )}

      <div className="rounded-lg border border-border bg-surface-1 p-6">
        {configLoading && config === null ? (
          <p className="text-sm text-text-muted">Loading configuration...</p>
        ) : (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void handleSave();
            }}
            className="flex flex-col gap-5"
          >
            {/* Linear API Key */}
            <label className="flex flex-col gap-1.5">
              <span className="text-sm font-medium text-text-secondary">
                Linear API Key
              </span>
              <div className="flex gap-2">
                <input
                  type={showApiKey ? "text" : "password"}
                  value={linearApiKey}
                  onChange={(e) => {
                    setLinearApiKey(e.target.value);
                  }}
                  placeholder="lin_api_..."
                  className="flex-1 rounded-md border border-border bg-surface-2 px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent"
                />
                <button
                  type="button"
                  onClick={() => {
                    setShowApiKey((prev) => !prev);
                  }}
                  className="rounded-md border border-border bg-surface-2 px-3 py-2 text-sm text-text-secondary hover:bg-surface-3 hover:text-text-primary"
                >
                  {showApiKey ? "Hide" : "Show"}
                </button>
              </div>
            </label>

            {/* Linear Project ID */}
            <label className="flex flex-col gap-1.5">
              <span className="text-sm font-medium text-text-secondary">
                Linear Project ID
              </span>
              <input
                type="text"
                value={linearProjectId}
                onChange={(e) => {
                  setLinearProjectId(e.target.value);
                }}
                placeholder="PRJ-123"
                className="rounded-md border border-border bg-surface-2 px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent"
              />
            </label>

            {/* Repository Path */}
            <label className="flex flex-col gap-1.5">
              <span className="text-sm font-medium text-text-secondary">
                Repository Path
              </span>
              <div className="flex gap-2">
                <input
                  type="text"
                  value={repoPath}
                  onChange={(e) => {
                    setRepoPath(e.target.value);
                  }}
                  placeholder="/path/to/repo"
                  className="flex-1 rounded-md border border-border bg-surface-2 px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent"
                />
                <button
                  type="button"
                  onClick={() => {
                    void handleBrowse();
                  }}
                  className="rounded-md border border-border bg-surface-2 px-3 py-2 text-sm text-text-secondary hover:bg-surface-3 hover:text-text-primary"
                >
                  Browse
                </button>
              </div>
            </label>

            {/* Agent Command */}
            <label className="flex flex-col gap-1.5">
              <span className="text-sm font-medium text-text-secondary">
                Agent Command
              </span>
              <input
                type="text"
                value={agentCommand}
                onChange={(e) => {
                  setAgentCommand(e.target.value);
                }}
                placeholder="claude"
                className="rounded-md border border-border bg-surface-2 px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent"
              />
            </label>

            {/* Hook Port */}
            <label className="flex flex-col gap-1.5">
              <span className="text-sm font-medium text-text-secondary">
                Hook Port
              </span>
              <input
                type="number"
                value={hookPort}
                onChange={(e) => {
                  const parsed = parseInt(e.target.value, 10);
                  if (!Number.isNaN(parsed)) {
                    setHookPort(parsed);
                  }
                }}
                placeholder="19876"
                className="rounded-md border border-border bg-surface-2 px-3 py-2 text-sm text-text-primary placeholder:text-text-muted focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent"
              />
            </label>

            {/* Save button */}
            <div className="flex justify-end pt-2">
              <button
                type="submit"
                disabled={configLoading}
                className="rounded-md bg-accent px-5 py-2 text-sm font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
              >
                {configLoading ? "Saving..." : "Save"}
              </button>
            </div>
          </form>
        )}
      </div>
    </div>
  );
}
