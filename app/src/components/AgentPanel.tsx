/**
 * Agent activity timeline — the product's signature surface for watching an
 * agent rework a PR in real time.
 *
 * Listens for Tauri `"agent-event"` events, maintains a local list, and renders
 * each event as a row on a vertical timeline rail: an SVG icon tile (no emoji)
 * colored by event tone, a bold title, a faint mono detail line, and a
 * right-aligned tabular-nums elapsed timestamp. A header LED pulses while the
 * agent runs and stops on a terminal Complete / Error event.
 *
 * Presentation (icon + tone + copy per event variant) lives in
 * `@/lib/agent-event`; this component owns streaming, layout, and lifecycle.
 */

import { useState, useEffect, useRef, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import type { Event } from "../bindings/Event";
import {
  presentEvent,
  toneTileClass,
  AgentMark,
  type EventPresentation,
} from "@/lib/agent-event";
import { X, Trash2 } from "lucide-react";
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

interface AgentPanelProps {
  /** Whether the panel is visible. */
  readonly visible: boolean;
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
function EndBanner({ phase }: { readonly phase: "complete" | "failed" }) {
  const complete = phase === "complete";
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
      <span className="font-medium">
        {complete ? "Reworked — ready to re-review" : "Failed"}
      </span>
    </div>
  );
}

export function AgentPanel({ visible, onClose }: AgentPanelProps) {
  const [entries, setEntries] = useState<readonly TimelineEntry[]>([]);
  const [startTime, setStartTime] = useState<number | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const seqRef = useRef(0);

  const phase = runPhase(entries);
  const isRunning = phase === "running";

  // Count tool uses and total thinking tokens for the header telemetry.
  let toolCount = 0;
  let thinkingTokens = 0;
  for (const e of entries) {
    if (e.event.kind === "ToolUse") toolCount += 1;
    if (e.event.kind === "Thinking") {
      thinkingTokens = Math.max(thinkingTokens, e.event.estimated_tokens);
    }
  }

  // Elapsed timer while running.
  useEffect(() => {
    if (!isRunning || startTime === null) return;
    const interval = setInterval(() => {
      setElapsed(Date.now() - startTime);
    }, 200);
    return () => {
      clearInterval(interval);
    };
  }, [isRunning, startTime]);

  // Listen for agent events from Tauri.
  useEffect(() => {
    const unlisten = listen<AgentEventEnvelope>("agent-event", (tauriEvent) => {
      // Unwrap the object-keyed envelope. Scoping/filtering by object_id is a
      // later wave — for now the panel behaves exactly as before.
      const event = tauriEvent.payload.event;
      const now = Date.now();
      setStartTime((prev) => prev ?? now);

      // Thinking events collapse in place: keep the latest token count on a
      // single row instead of spamming the rail.
      if (event.kind === "Thinking") {
        setEntries((prev) => {
          const lastIdx = prev.length - 1;
          const last = lastIdx >= 0 ? prev[lastIdx] : undefined;
          if (last !== undefined && last.event.kind === "Thinking") {
            const updated = [...prev];
            updated[lastIdx] = { ...last, event, timestamp: now };
            return updated;
          }
          seqRef.current += 1;
          return [...prev, { seq: seqRef.current, event, timestamp: now }];
        });
        return;
      }

      seqRef.current += 1;
      setEntries((prev) => [
        ...prev,
        { seq: seqRef.current, event, timestamp: now },
      ]);
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
    setEntries([]);
    seqRef.current = 0;
    setStartTime(null);
    setElapsed(0);
  }, []);

  if (!visible) return null;

  const modeLabel = phase === "failed" ? "failed" : isRunning ? "running" : "idle";
  // Once any event has arrived, startTime is set; captured here so the render
  // path never needs a non-null assertion.
  const anchor = startTime;

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
              <EndBanner phase={phase} />
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
