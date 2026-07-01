/**
 * CI tab: check pipelines (grouped by workflow) with per-pipeline logs.
 *
 * Loads the PR's checks on mount and stays live via the `ci-updated` Tauri
 * event (the same event DiffView's badge listens to). Each workflow renders as
 * a collapsible pipeline. Failing pipelines auto-expand and auto-fetch their own
 * failed-job logs (derived from a failed check's `link`); passing pipelines stay
 * collapsed and fetch nothing, so no subprocess runs for green CI. Users can
 * inspect CI without leaving cockpit, open a check on GitHub, refresh a
 * pipeline's logs, and dispatch the Fix loop (explicit + confirmed, Invariant
 * 5).
 *
 * All CI queries are best-effort (Invariant 1): a `gh` error yields an empty
 * result, never a blocked loop.
 */

import { useState, useEffect, useCallback, useMemo } from "react";
import { listen } from "@tauri-apps/api/event";
import type { CiCheck } from "../bindings/CiCheck";
import { useAppStore } from "../store";
import { openExternal } from "@/lib/open";
import {
  checkOutcome,
  summarizeChecks,
  ciState,
  parseCiUpdate,
  type CheckOutcome,
} from "@/lib/ci";
import { EmptyState } from "./EmptyState";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import {
  CheckCircle2,
  XCircle,
  Loader2,
  ExternalLink,
  RefreshCw,
  Wrench,
  Workflow,
  ChevronDown,
  ChevronRight,
  FileText,
} from "lucide-react";

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

interface CiPanelProps {
  readonly pr: string;
  /** Whether this tab is currently visible (drives fresh loads on open). */
  readonly active: boolean;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Load state for the checks list. */
type ChecksState =
  | { readonly kind: "loading" }
  | { readonly kind: "loaded"; readonly checks: readonly CiCheck[] };

/** Per-pipeline log load state. */
type LogsState =
  | { readonly kind: "idle" }
  | { readonly kind: "loading" }
  | { readonly kind: "loaded"; readonly text: string };

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** Tailwind classes + icon for a single check outcome. */
function outcomeStyle(outcome: CheckOutcome): {
  readonly className: string;
  readonly icon: React.ReactNode;
} {
  switch (outcome) {
    case "pass":
      return {
        className: "text-success",
        icon: <CheckCircle2 className="h-4 w-4" />,
      };
    case "fail":
      return {
        className: "text-danger",
        icon: <XCircle className="h-4 w-4" />,
      };
    case "pending":
      return {
        className: "text-warning",
        icon: <Loader2 className="h-4 w-4 animate-spin" />,
      };
    default:
      return assertNever(outcome);
  }
}

/** One workflow pipeline: its name, its checks, and whether any check failed. */
interface Pipeline {
  readonly workflow: string;
  readonly checks: readonly CiCheck[];
  readonly hasFailure: boolean;
  /** A failed check's link (used to derive the run id for its logs), if any. */
  readonly failedLink: string | null;
}

/** Group checks by their workflow (each workflow = one pipeline section). */
function groupByWorkflow(checks: readonly CiCheck[]): readonly Pipeline[] {
  const groups = new Map<string, CiCheck[]>();
  for (const c of checks) {
    const key = c.workflow !== "" ? c.workflow : "Other";
    const arr = groups.get(key) ?? [];
    arr.push(c);
    groups.set(key, arr);
  }
  return Array.from(groups.entries())
    .map(([workflow, ws]): Pipeline => {
      const failed = ws.filter((c) => checkOutcome(c) === "fail");
      const failedLink = failed.find((c) => c.link !== "")?.link ?? null;
      return {
        workflow,
        checks: ws,
        hasFailure: failed.length > 0,
        failedLink,
      };
    })
    .sort((a, b) => a.workflow.localeCompare(b.workflow));
}

// ---------------------------------------------------------------------------
// PipelineSection -- one collapsible workflow with inline per-pipeline logs
// ---------------------------------------------------------------------------

function PipelineSection({
  pipeline,
  pr,
}: {
  readonly pipeline: Pipeline;
  readonly pr: string;
}) {
  const ciRunLogsByLink = useAppStore((s) => s.ciRunLogsByLink);

  // Failing pipelines start expanded; passing ones stay collapsed (and never
  // fetch logs) to avoid eager subprocess calls for green CI.
  const [expanded, setExpanded] = useState(pipeline.hasFailure);
  const [logsState, setLogsState] = useState<LogsState>({ kind: "idle" });

  const { failedLink } = pipeline;

  const loadLogs = useCallback(async () => {
    if (failedLink === null) return;
    setLogsState({ kind: "loading" });
    const text = await ciRunLogsByLink(pr, failedLink);
    setLogsState({ kind: "loaded", text });
  }, [pr, failedLink, ciRunLogsByLink]);

  // Auto-fetch this pipeline's logs once, when it has a failure and a link.
  // Passing pipelines (no failedLink) never trigger a fetch.
  useEffect(() => {
    if (!pipeline.hasFailure || failedLink === null) return;
    let cancelled = false;
    setLogsState({ kind: "loading" });
    void ciRunLogsByLink(pr, failedLink).then((text) => {
      if (!cancelled) setLogsState({ kind: "loaded", text });
    });
    return () => {
      cancelled = true;
    };
  }, [pr, pipeline.hasFailure, failedLink, ciRunLogsByLink]);

  const summary = useMemo(
    () => summarizeChecks(pipeline.checks),
    [pipeline.checks],
  );
  const overall = ciState(summary);

  return (
    <section className="overflow-hidden rounded-lg border border-border bg-card">
      <button
        type="button"
        onClick={() => {
          setExpanded((prev) => !prev);
        }}
        aria-expanded={expanded}
        className="flex w-full cursor-pointer items-center gap-2 border-none bg-muted/40 px-3 py-2 text-left hover:bg-muted/60"
      >
        {expanded ? (
          <ChevronDown className="h-3.5 w-3.5 text-muted-foreground" />
        ) : (
          <ChevronRight className="h-3.5 w-3.5 text-muted-foreground" />
        )}
        <Workflow className="h-3.5 w-3.5 text-muted-foreground" />
        <span className="text-xs font-semibold text-foreground">
          {pipeline.workflow}
        </span>
        <span className="text-[10px] text-muted-foreground">
          ({String(pipeline.checks.length)})
        </span>
        <span
          className={cn(
            "ml-auto inline-flex shrink-0 items-center gap-1 text-xs",
            overall === "pass" && "text-success",
            overall === "fail" && "text-danger",
            overall === "pending" && "text-warning",
          )}
        >
          {overall === "pass" && <CheckCircle2 className="h-3.5 w-3.5" />}
          {overall === "fail" && <XCircle className="h-3.5 w-3.5" />}
          {overall === "pending" && (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          )}
        </span>
      </button>

      {expanded && (
        <>
          <ul className="divide-y divide-border">
            {pipeline.checks.map((check) => {
              const style = outcomeStyle(checkOutcome(check));
              return (
                <li
                  key={`${check.workflow}:${check.name}:${check.link}`}
                  className="flex items-center gap-3 px-3 py-2 text-sm"
                >
                  <span className={cn("shrink-0", style.className)}>
                    {style.icon}
                  </span>
                  <span
                    className="flex-1 truncate text-foreground"
                    title={check.name}
                  >
                    {check.name}
                  </span>
                  <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
                    {check.state}
                  </span>
                  {check.link !== "" && (
                    <a
                      href={check.link}
                      onClick={(e) => {
                        e.preventDefault();
                        void openExternal(check.link);
                      }}
                      className="inline-flex shrink-0 items-center gap-1 text-xs text-primary hover:underline"
                      title="Open check on GitHub"
                    >
                      <ExternalLink className="h-3 w-3" />
                    </a>
                  )}
                </li>
              );
            })}
          </ul>

          {/* Per-pipeline failed-run logs (only for failing pipelines) */}
          {pipeline.hasFailure && (
            <div className="border-t border-border">
              <div className="flex items-center gap-2 px-3 py-1.5">
                <FileText className="h-3.5 w-3.5 text-muted-foreground" />
                <span className="text-[10px] font-semibold text-muted-foreground">
                  Failed-run logs
                </span>
                <div className="ml-auto">
                  <Button
                    variant="outline"
                    size="sm"
                    className="h-6 text-[10px]"
                    onClick={() => void loadLogs()}
                    disabled={
                      logsState.kind === "loading" || failedLink === null
                    }
                    title="Refresh this pipeline's failed-run logs"
                  >
                    {logsState.kind === "loading" ? (
                      <Loader2 className="h-3 w-3 animate-spin" />
                    ) : (
                      <RefreshCw className="h-3 w-3" />
                    )}
                    Refresh logs
                  </Button>
                </div>
              </div>

              {logsState.kind === "loading" && (
                <p className="px-3 pb-3 text-[11px] text-muted-foreground">
                  Loading logs...
                </p>
              )}
              {logsState.kind === "loaded" &&
                (logsState.text.trim() === "" ? (
                  <div className="px-3 pb-3 text-[11px] text-muted-foreground">
                    <p>
                      Logs couldn&apos;t be retrieved. The run may still be in
                      progress, or its logs have expired (GitHub keeps Actions
                      logs for a limited time, then returns HTTP 410).
                    </p>
                    {failedLink !== null && (
                      <a
                        href={failedLink}
                        onClick={(e) => {
                          e.preventDefault();
                          void openExternal(failedLink);
                        }}
                        className="mt-1 inline-flex items-center gap-1 text-primary hover:underline"
                        title="Open this run on GitHub"
                      >
                        <ExternalLink className="h-3 w-3" />
                        View run on GitHub
                      </a>
                    )}
                  </div>
                ) : (
                  <pre className="mx-3 mb-3 max-h-[40vh] overflow-auto rounded-md border border-border bg-muted/30 p-3 font-mono text-[11px] leading-relaxed text-foreground">
                    {logsState.text}
                  </pre>
                ))}
            </div>
          )}
        </>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// CiPanel
// ---------------------------------------------------------------------------

export function CiPanel({ pr, active }: CiPanelProps) {
  const listCiChecks = useAppStore((s) => s.listCiChecks);
  const fixCi = useAppStore((s) => s.fixCi);
  const gateState = useAppStore((s) => s.activeReview?.gate_state ?? null);

  const [checksState, setChecksState] = useState<ChecksState>({
    kind: "loading",
  });
  const [refreshing, setRefreshing] = useState(false);
  const [fixing, setFixing] = useState(false);

  // -- Load checks (initial + on every re-open, so the tab is always fresh) --
  const loadChecks = useCallback(async () => {
    const checks = await listCiChecks(pr);
    setChecksState({ kind: "loaded", checks });
  }, [pr, listCiChecks]);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    setRefreshing(true);
    void loadChecks().finally(() => {
      if (!cancelled) setRefreshing(false);
    });
    return () => {
      cancelled = true;
    };
  }, [active, loadChecks]);

  // -- Live updates: the backend pushes the full checks list via `ci-updated` --
  useEffect(() => {
    const unlisten = listen<unknown>("ci-updated", (event) => {
      const update = parseCiUpdate(event.payload);
      if (update === null || update.pr !== pr) return;
      setChecksState({ kind: "loaded", checks: update.checks });
    });
    return () => {
      void unlisten.then((f) => {
        f();
      });
    };
  }, [pr]);

  // -- Fix CI (explicit + confirmed; Invariant 5) --
  const handleFixCi = useCallback(async () => {
    const confirmed = window.confirm(
      "Dispatch an agent to fix the failing CI checks? This transitions the review to Dispatched and runs the fixer agent.",
    );
    if (!confirmed) return;
    setFixing(true);
    try {
      await fixCi(pr);
    } finally {
      setFixing(false);
    }
  }, [pr, fixCi]);

  const checks = checksState.kind === "loaded" ? checksState.checks : [];
  const summary = useMemo(() => summarizeChecks(checks), [checks]);
  const overall = ciState(summary);
  const pipelines = useMemo(() => groupByWorkflow(checks), [checks]);
  const hasFailures = summary.failed > 0;

  // =========================================================================
  // Render
  // =========================================================================

  return (
    <div className="flex h-full flex-col">
      {/* Header ---------------------------------------------------------- */}
      <header className="flex shrink-0 items-center gap-3 border-b border-border bg-card px-4 py-2">
        <span className="text-sm font-semibold">CI</span>

        {checksState.kind === "loaded" && summary.total > 0 && (
          <span
            className={cn(
              "inline-flex shrink-0 items-center gap-1 rounded-md border px-1.5 py-0.5 text-xs",
              overall === "pass" &&
                "border-success/30 bg-success/15 text-success",
              overall === "fail" && "border-danger/30 bg-danger/15 text-danger",
              overall === "pending" &&
                "border-warning/30 bg-warning/15 text-warning",
            )}
            title={`CI: ${String(summary.passed)} passed, ${String(summary.failed)} failed, ${String(summary.pending)} pending`}
          >
            {overall === "pass" && <CheckCircle2 className="h-3 w-3" />}
            {overall === "fail" && <XCircle className="h-3 w-3" />}
            {overall === "pending" && (
              <Loader2 className="h-3 w-3 animate-spin" />
            )}
            {String(summary.passed)}/{String(summary.total)}
          </span>
        )}

        <div className="ml-auto flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => void loadChecks()}
            disabled={refreshing}
            title="Refresh checks"
          >
            <RefreshCw
              className={cn("h-3.5 w-3.5", refreshing && "animate-spin")}
            />
            Refresh
          </Button>

          {/* Fix CI failures — explicit user action; only when CI is failing */}
          {hasFailures && (
            <Button
              variant="outline"
              size="sm"
              className="border-danger/40 text-danger hover:bg-danger/10"
              onClick={() => void handleFixCi()}
              disabled={fixing || gateState === "Dispatched"}
              title="Dispatch an agent to fix the failing CI checks"
            >
              <Wrench className="h-3.5 w-3.5" />
              {fixing ? "Dispatching..." : "Fix CI failures"}
            </Button>
          )}
        </div>
      </header>

      {/* Body ------------------------------------------------------------ */}
      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        {checksState.kind === "loading" ? (
          <div className="flex items-center justify-center py-12 text-sm text-muted-foreground">
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            Loading checks...
          </div>
        ) : checks.length === 0 ? (
          <EmptyState
            icon="🟢"
            title="No CI checks"
            description="No CI checks were found for this PR. It may not be a GitHub PR, GitHub may not be reachable, or no workflows have run yet."
          />
        ) : (
          <div className="space-y-4">
            {pipelines.map((pipeline) => (
              <PipelineSection
                key={pipeline.workflow}
                pipeline={pipeline}
                pr={pr}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
