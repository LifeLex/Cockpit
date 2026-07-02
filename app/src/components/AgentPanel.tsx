/**
 * Agent activity timeline — the product's signature surface for watching an
 * agent rework a PR in real time.
 *
 * Listens for Tauri `"agent-event"` envelopes, keeps a per-object buffer so
 * switching between reviews never interleaves or wipes timelines, and renders
 * the current object's events as rows on a vertical timeline rail: an SVG icon
 * tile (no emoji) colored by event tone, a bold title, a faint mono detail
 * line, and a right-aligned tabular-nums elapsed timestamp. A header LED pulses
 * while the agent runs and stops on a terminal Complete / Error event.
 *
 * The panel also owns two explicit agent controls for the current object: a
 * Stop button (visible while an agent is attached) that kills the run, and an
 * Open-log affordance that opens the run's log file in the configured editor.
 *
 * Presentation (icon + tone + copy per event variant) lives in
 * `@/lib/agent-event`; this component owns streaming, layout, and lifecycle.
 */

import { useState, useEffect, useRef, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import type { Event } from "../bindings/Event";
import { useAppStore } from "../store";
import {
  presentEvent,
  toneTileClass,
  AgentMark,
  type EventPresentation,
} from "@/lib/agent-event";
import { X, Trash2, Square, FileText } from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * Envelope wrapping an agent {@link Event} with the UI key of the object it
 * belongs to, as emitted on the backend `"agent-event"` channel. Hand-typed to
 * mirror the Rust `AgentEventEnvelope` (no ts-rs binding).
 */
interface AgentEventEnvelope {
  /** UI key of the object this event belongs to (PR ref or project id). */
  readonly object_id: string;
  /** The parsed agent stream event. */
  readonly event: Event;
}

/** Internal event entry with a local sequence number for keying. */
interface TimelineEntry {
  readonly seq: number;
  readonly event: Event;
  /** Wall-clock ms when received (for the elapsed-since-start stamp). */
  readonly timestamp: number;
}

/** Per-object timeline snapshot: its entries plus whether it was user-stopped. */
interface ObjectTimeline {
  readonly entries: readonly TimelineEntry[];
  /** True once the user stopped this object's run (terminal, danger banner). */
  readonly stopped: boolean;
}

/**
 * Module-level per-object event buffers, keyed by `object_id`.
 *
 * The panel stays mounted per workspace and its `objectId` prop changes as the
 * user navigates between reviews; buffering here (rather than in component
 * state) means each object keeps its full timeline across those switches and
 * across panel remounts. Buffers are append-only for the app's lifetime, which
 * is acceptable for the number of agent runs in a session.
 */
const timelines = new Map<string, ObjectTimeline>();

/** Monotonic sequence source for stable React keys across all objects. */
let globalSeq = 0;

/** Read (or lazily create) the timeline buffer for an object. */
function timelineFor(objectId: string): ObjectTimeline {
  const existing = timelines.get(objectId);
  if (existing !== undefined) return existing;
  const created: ObjectTimeline = { entries: [], stopped: false };
  timelines.set(objectId, created);
  return created;
}

/**
 * Append an incoming stream event to an object's buffer, returning a fresh
 * snapshot. Consecutive `Thinking` events collapse onto a single row (latest
 * token count wins) instead of spamming the rail. A real stream event means the
 * agent is active again, so any prior user-stop marker is cleared.
 */
function appendEvent(
  objectId: string,
  event: Event,
  now: number,
): ObjectTimeline {
  const cur = timelineFor(objectId);
  let entries: TimelineEntry[];

  if (event.kind === "Thinking") {
    const lastIdx = cur.entries.length - 1;
    const last = lastIdx >= 0 ? cur.entries[lastIdx] : undefined;
    if (last !== undefined && last.event.kind === "Thinking") {
      entries = [...cur.entries];
      entries[lastIdx] = { ...last, event, timestamp: now };
    } else {
      globalSeq += 1;
      entries = [...cur.entries, { seq: globalSeq, event, timestamp: now }];
    }
  } else {
    globalSeq += 1;
    entries = [...cur.entries, { seq: globalSeq, event, timestamp: now }];
  }

  const next: ObjectTimeline = { entries, stopped: false };
  timelines.set(objectId, next);
  return next;
}

/**
 * Push a synthetic terminal "stopped" entry after a user Stop, returning a
 * fresh snapshot. The entry is a plain `Error` event so it reuses the existing
 * error-row presentation; the `stopped` flag drives the terminal banner copy.
 */
function appendStopped(objectId: string, now: number): ObjectTimeline {
  const cur = timelineFor(objectId);
  globalSeq += 1;
  const event: Event = { kind: "Error", message: "Agent stopped by you" };
  const next: ObjectTimeline = {
    entries: [...cur.entries, { seq: globalSeq, event, timestamp: now }],
    stopped: true,
  };
  timelines.set(objectId, next);
  return next;
}

/** Clear an object's buffer (Clear button). */
function clearTimeline(objectId: string): void {
  timelines.set(objectId, { entries: [], stopped: false });
}

interface AgentPanelProps {
  /** Whether the panel is visible. */
  readonly visible: boolean;
  /** UI key of the object whose timeline this panel shows (PR ref / project id). */
  readonly objectId: string;
  /** Callback to hide the panel / return to the diff. */
  readonly onClose: () => void;
}

/** Terminal lifecycle of the run, derived from the received events. */
type RunPhase = "idle" | "running" | "complete" | "failed";

/** Format milliseconds as a compact, monospace-friendly duration. */
function formatElapsed(ms: number): string {
  if (ms < 1000) return `${String(ms)}ms`;
  const totalSeconds = Math.floor(ms / 1000);
  if (totalSeconds < 60) return `${String(totalSeconds)}s`;
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${String(minutes)}m ${String(seconds).padStart(2, "0")}s`;
}

/** Derive the run phase from the current entry list. */
function runPhase(entries: readonly TimelineEntry[]): RunPhase {
  if (entries.length === 0) return "idle";
  for (let i = entries.length - 1; i >= 0; i -= 1) {
    const kind = entries[i]?.event.kind;
    if (kind === "Complete") return "complete";
    if (kind === "Error") return "failed";
  }
  return "running";
}

/** The pulsing/steady header LED, honoring reduced-motion. */
function HeaderLed({ phase }: { readonly phase: RunPhase }) {
  const color =
    phase === "running"
      ? "bg-state-dispatched"
      : phase === "complete"
        ? "bg-success"
        : phase === "failed"
          ? "bg-danger"
          : "bg-muted-foreground";
  return (
    <span className="relative flex h-2.5 w-2.5 shrink-0" aria-hidden="true">
      {phase === "running" && (
        <span
          className={cn(
            "absolute inline-flex h-full w-full rounded-full opacity-60 animate-ping motion-reduce:hidden",
            color,
          )}
        />
      )}
      <span
        className={cn("relative inline-flex h-2.5 w-2.5 rounded-full", color)}
      />
    </span>
  );
}

/** A single timeline row: rail node + icon tile + title/detail + timestamp. */
function TimelineRow({
  presentation,
  elapsedLabel,
  isLast,
  running,
}: {
  readonly presentation: EventPresentation;
  readonly elapsedLabel: string;
  readonly isLast: boolean;
  readonly running: boolean;
}) {
  const { icon: Icon, tone, title, detail } = presentation;
  return (
    <li className="relative flex gap-3 pb-4 last:pb-0">
      {/* Rail: vertical connector behind the tile, hidden on the last row. */}
      {!isLast && (
        <span
          aria-hidden="true"
          className="absolute left-[13px] top-7 bottom-0 w-px bg-border"
        />
      )}

      {/* Icon tile */}
      <span
        className={cn(
          "relative z-10 flex h-[26px] w-[26px] shrink-0 items-center justify-center rounded-md",
          toneTileClass(tone),
        )}
      >
        <Icon className="h-3.5 w-3.5" />
      </span>

      {/* Body */}
      <div className="flex min-w-0 flex-1 items-baseline gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="font-display text-[13px] font-semibold text-foreground">
              {title}
            </span>
            {isLast && running && (
              <span
                aria-hidden="true"
                className="inline-block h-2.5 w-2.5 rounded-full border-2 border-current border-t-transparent text-muted-foreground animate-spin motion-reduce:hidden"
              />
            )}
          </div>
          {detail !== "" && (
            <p className="mt-0.5 truncate font-mono text-xs text-muted-foreground">
              {detail}
            </p>
          )}
        </div>
        <span className="shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground/70">
          {elapsedLabel}
        </span>
      </div>
    </li>
  );
}

/** Calm, emoji-free empty state shown before any event arrives. */
function EmptyState() {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
      <AgentMark className="h-6 w-6 text-muted-foreground/50" />
      <p className="font-display text-sm text-muted-foreground">
        Waiting for the agent…
      </p>
      <p className="max-w-[32ch] text-xs text-muted-foreground/70">
        Activity will stream here as the agent reworks this PR.
      </p>
    </div>
  );
}

/** Terminal banner shown after the run finishes. */
function EndBanner({
  phase,
  stopped,
}: {
  readonly phase: "complete" | "failed";
  readonly stopped: boolean;
}) {
  const complete = phase === "complete" && !stopped;
  const label = complete
    ? "Reworked — ready to re-review"
    : stopped
      ? "Stopped by you"
      : "Failed";
  return (
    <div
      className={cn(
        "mt-1 flex items-center gap-2 rounded-md border px-3 py-2 text-xs",
        complete
          ? "border-success/30 bg-success/10 text-success"
          : "border-danger/30 bg-danger/10 text-danger",
      )}
    >
      <span
        className={cn(
          "inline-block h-2 w-2 rounded-full",
          complete ? "bg-success" : "bg-danger",
        )}
      />
      <span className="font-medium">{label}</span>
    </div>
  );
}

export function AgentPanel({ visible, objectId, onClose }: AgentPanelProps) {
  const initial = timelineFor(objectId);
  const [entries, setEntries] = useState<readonly TimelineEntry[]>(
    initial.entries,
  );
  const [stopped, setStopped] = useState<boolean>(initial.stopped);
  const [elapsed, setElapsed] = useState(0);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  // The review whose agent this panel controls, when it is the current object.
  // AgentPanel is mounted per workspace, so `activeReview.pr === objectId`.
  const activeReview = useAppStore((s) => s.activeReview);
  const killAgent = useAppStore((s) => s.killAgent);
  const reviewForObject = activeReview?.pr === objectId ? activeReview : null;
  const agentAttached = reviewForObject?.agent != null;
  const logPath = reviewForObject?.agent?.log_path ?? null;

  const phase = runPhase(entries);
  const isRunning = phase === "running";
  // The first event's timestamp is the run anchor for elapsed stamps; null
  // until any event has arrived.
  const anchor = entries[0]?.timestamp ?? null;

  // Count tool uses and total thinking tokens for the header telemetry.
  let toolCount = 0;
  let thinkingTokens = 0;
  for (const e of entries) {
    if (e.event.kind === "ToolUse") toolCount += 1;
    if (e.event.kind === "Thinking") {
      thinkingTokens = Math.max(thinkingTokens, e.event.estimated_tokens);
    }
  }

  // Keep the current object id reachable from the (mount-once) event listener.
  const objectIdRef = useRef(objectId);
  useEffect(() => {
    objectIdRef.current = objectId;
  }, [objectId]);

  // Load the buffered timeline whenever the shown object changes.
  useEffect(() => {
    const t = timelineFor(objectId);
    setEntries(t.entries);
    setStopped(t.stopped);
    setElapsed(0);
  }, [objectId]);

  // Elapsed timer while running.
  useEffect(() => {
    if (!isRunning || anchor === null) return;
    const interval = setInterval(() => {
      setElapsed(Date.now() - anchor);
    }, 200);
    return () => {
      clearInterval(interval);
    };
  }, [isRunning, anchor]);

  // Listen for agent events from Tauri. Registered once; every event is
  // buffered under its own object_id and only the shown object updates state.
  useEffect(() => {
    const unlisten = listen<AgentEventEnvelope>("agent-event", (tauriEvent) => {
      const { object_id, event } = tauriEvent.payload;
      const next = appendEvent(object_id, event, Date.now());
      if (object_id === objectIdRef.current) {
        setEntries(next.entries);
        setStopped(next.stopped);
      }
    });

    return () => {
      void unlisten.then((f) => {
        f();
      });
    };
  }, []);

  // Auto-scroll to the latest event.
  useEffect(() => {
    scrollRef.current?.scrollTo({
      top: scrollRef.current.scrollHeight,
      behavior: "smooth",
    });
  }, [entries]);

  const handleClear = useCallback(() => {
    clearTimeline(objectId);
    setEntries([]);
    setStopped(false);
    setElapsed(0);
  }, [objectId]);

  const handleStop = useCallback(async () => {
    const confirmed = window.confirm(
      `Stop the agent working on ${objectId}?`,
    );
    if (!confirmed) return;
    await killAgent(objectId);
    // Only mark the timeline stopped when the kill actually settled the review
    // (its agent handle was cleared). A failed kill leaves the agent running
    // and surfaces via the store error, so the timeline is left untouched.
    const settled = useAppStore.getState().activeReview;
    if (settled?.pr === objectId && settled.agent != null) return;
    const next = appendStopped(objectId, Date.now());
    setEntries(next.entries);
    setStopped(next.stopped);
  }, [objectId, killAgent]);

  const handleOpenLog = useCallback(() => {
    if (logPath === null) return;
    // Open the local log file in the configured editor. `open_in_editor`
    // resolves an absolute path as-is and uses existing capabilities (no
    // opener-plugin URL scheme needed for a local file).
    void invoke("open_in_editor", {
      filePath: logPath,
      repoSlug: null,
      branch: null,
    });
  }, [logPath]);

  if (!visible) return null;

  const modeLabel = stopped
    ? "stopped"
    : phase === "failed"
      ? "failed"
      : isRunning
        ? "running"
        : "idle";

  return (
    <div className="flex h-full min-h-0 flex-col bg-card">
      {/* Header */}
      <div className="flex shrink-0 items-center gap-3 border-b border-border px-4 py-2.5">
        <HeaderLed phase={phase} />
        <AgentMark className="h-4 w-4 text-brand" />
        <span className="font-display text-sm font-semibold text-foreground">
          Agent
        </span>
        <span className="font-mono text-xs text-muted-foreground">
          · {modeLabel}
        </span>

        {anchor !== null && (
          <span className="font-mono text-xs tabular-nums text-muted-foreground">
            {formatElapsed(isRunning ? elapsed : lastElapsed(entries, anchor))}
          </span>
        )}

        <div className="ml-auto flex items-center gap-3">
          {(toolCount > 0 || thinkingTokens > 0) && (
            <span className="flex items-center gap-2 font-mono text-[11px] tabular-nums text-muted-foreground/80">
              {toolCount > 0 && <span>{String(toolCount)} tools</span>}
              {thinkingTokens > 0 && (
                <span>{String(thinkingTokens)} tok</span>
              )}
            </span>
          )}
          {logPath !== null && (
            <button
              type="button"
              onClick={handleOpenLog}
              title="Open the agent log in your editor"
              className="flex cursor-pointer items-center gap-1 border-none bg-transparent text-xs text-muted-foreground hover:text-foreground"
            >
              <FileText className="h-3.5 w-3.5" />
              Open log
            </button>
          )}
          {agentAttached && (
            <button
              type="button"
              onClick={() => void handleStop()}
              title="Stop the running agent"
              className="flex cursor-pointer items-center gap-1 border-none bg-transparent text-xs text-danger hover:text-danger/80"
            >
              <Square className="h-3.5 w-3.5" />
              Stop
            </button>
          )}
          <button
            type="button"
            onClick={handleClear}
            title="Clear activity"
            className="flex cursor-pointer items-center gap-1 border-none bg-transparent text-xs text-muted-foreground hover:text-foreground"
          >
            <Trash2 className="h-3.5 w-3.5" />
            Clear
          </button>
          <button
            type="button"
            onClick={onClose}
            title="Close"
            className="flex cursor-pointer items-center border-none bg-transparent text-muted-foreground hover:text-foreground"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
      </div>

      {/* Timeline */}
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto px-4 py-4">
        {entries.length === 0 || anchor === null ? (
          <EmptyState />
        ) : (
          <>
            <ol className="list-none">
              {entries.map((entry, idx) => (
                <TimelineRow
                  key={entry.seq}
                  presentation={presentEvent(entry.event)}
                  elapsedLabel={formatElapsed(
                    Math.max(0, entry.timestamp - anchor),
                  )}
                  isLast={idx === entries.length - 1}
                  running={isRunning}
                />
              ))}
            </ol>
            {(phase === "complete" || phase === "failed") && (
              <EndBanner phase={phase} stopped={stopped} />
            )}
          </>
        )}
      </div>
    </div>
  );
}

/** Elapsed from start to the last received event (for the frozen final stamp). */
function lastElapsed(
  entries: readonly TimelineEntry[],
  startTime: number,
): number {
  const last = entries[entries.length - 1];
  if (last === undefined) return 0;
  return Math.max(0, last.timestamp - startTime);
}
