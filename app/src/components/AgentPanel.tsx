/**
 * Agent activity timeline — renders streaming JSONL events from a
 * Claude Code agent run as a real-time activity feed.
 *
 * Listens for Tauri `"agent-event"` events, maintains a list in local
 * state, and renders each event type with appropriate styling.
 * Subagent spawns (Agent/Skill tools) are visualised inline.
 */

import { useState, useEffect, useRef, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import type { Event } from "../bindings/Event";

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** Internal event entry with a local sequence number for keying. */
interface TimelineEntry {
  readonly seq: number;
  readonly event: Event;
  readonly timestamp: number;
}

interface AgentPanelProps {
  /** Whether the panel is visible. */
  readonly visible: boolean;
  /** Callback to hide the panel. */
  readonly onClose: () => void;
}

/** Format milliseconds as a human-readable duration. */
function formatDuration(ms: number): string {
  if (ms < 1000) return `${String(ms)}ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = seconds % 60;
  return `${String(minutes)}m ${remainingSeconds.toFixed(0)}s`;
}

/** Format cost in USD. */
function formatCost(usd: number): string {
  if (usd < 0.01) return `$${usd.toFixed(4)}`;
  return `$${usd.toFixed(2)}`;
}

/** Determine the icon/prefix for a tool name. */
function toolIcon(name: string): string {
  switch (name) {
    case "Read":
      return "\u{1F4C4}"; // page facing up
    case "Edit":
      return "\u{270F}\u{FE0F}"; // pencil
    case "Write":
      return "\u{1F4DD}"; // memo
    case "Bash":
      return "\u{1F4BB}"; // laptop
    case "Agent":
    case "Skill":
    case "Task":
      return "\u{25C8}"; // diamond
    default:
      return "\u{2699}\u{FE0F}"; // gear
  }
}

/** Render a single timeline event. */
function renderEvent(entry: TimelineEntry): React.ReactNode {
  const { event } = entry;

  switch (event.kind) {
    case "Init":
      return (
        <div className="flex items-center gap-2 text-muted-foreground">
          <span className="text-primary">{"\u{2721}"}</span>
          <span>
            Started &middot; {event.model} &middot; {String(event.tools.length)} tools
          </span>
        </div>
      );

    case "Thinking":
      return (
        <div className="flex items-center gap-2 text-muted-foreground">
          <span>{"\u{1F4AD}"}</span>
          <span>thinking &middot; {String(event.estimated_tokens)} tokens</span>
        </div>
      );

    case "ToolUse":
      return (
        <div className="flex items-center gap-2">
          <span>{toolIcon(event.name)}</span>
          <span className="font-medium text-foreground">{event.name}</span>
          <span className="text-muted-foreground truncate">{event.input_summary}</span>
        </div>
      );

    case "ToolResult":
      return (
        <div className="flex items-center gap-2">
          <span>{event.success ? "\u{2713}" : "\u{2717}"}</span>
          <span
            className={
              event.success
                ? "text-success text-xs"
                : "text-danger text-xs"
            }
          >
            {event.success ? "ok" : "error"}
          </span>
          {event.summary !== "" && (
            <span className="text-muted-foreground text-xs truncate max-w-[400px]">
              {event.summary}
            </span>
          )}
        </div>
      );

    case "Text":
      return (
        <div className="text-foreground text-sm whitespace-pre-wrap max-h-[80px] overflow-y-auto">
          {event.content}
        </div>
      );

    case "SubagentSpawn":
      return (
        <div className="flex items-center gap-2">
          <span className="text-primary">{"\u{25C8}"}</span>
          <span className="font-medium text-foreground">Agent</span>
          <span className="text-muted-foreground truncate">{event.prompt}</span>
          <span className="inline-block w-3 h-3 border-2 border-primary border-t-transparent rounded-full animate-spin" />
        </div>
      );

    case "SubagentResult":
      return (
        <div className="flex items-center gap-2">
          <span className="text-success">{"\u{25C8}"}</span>
          <span className="text-success text-xs">done</span>
          <span className="text-muted-foreground text-xs truncate max-w-[400px]">
            {event.result}
          </span>
        </div>
      );

    case "RateLimit":
      return (
        <div className="flex items-center gap-2 text-warning">
          <span>{"\u{26A0}\u{FE0F}"}</span>
          <span className="text-xs">Rate limited: {event.status}</span>
        </div>
      );

    case "Complete":
      return (
        <div className="flex items-center gap-2 text-success font-medium">
          <span>{"\u{2713}"}</span>
          <span>
            Complete &middot; {formatDuration(event.duration_ms)} &middot;{" "}
            {formatCost(event.cost_usd)} &middot; {String(event.output_tokens)} tokens
          </span>
        </div>
      );

    case "Error":
      return (
        <div className="flex items-center gap-2 text-danger">
          <span>{"\u{2717}"}</span>
          <span>{event.message}</span>
        </div>
      );

    default:
      return assertNever(event);
  }
}

export function AgentPanel({ visible, onClose }: AgentPanelProps) {
  const [entries, setEntries] = useState<readonly TimelineEntry[]>([]);
  const [startTime] = useState(() => Date.now());
  const [elapsed, setElapsed] = useState(0);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const seqRef = useRef(0);

  // Track whether the agent is still running: at least one event received
  // and no terminal event yet.
  const isRunning = entries.length > 0 && !entries.some(
    (e) => e.event.kind === "Complete" || e.event.kind === "Error",
  );

  // Elapsed time counter while running.
  useEffect(() => {
    if (!isRunning) return;
    const interval = setInterval(() => {
      setElapsed(Date.now() - startTime);
    }, 100);
    return () => { clearInterval(interval); };
  }, [isRunning, startTime]);

  // Listen for agent events from Tauri.
  useEffect(() => {
    const unlisten = listen<Event>("agent-event", (tauriEvent) => {
      const event = tauriEvent.payload;

      // For Thinking events, update in place instead of appending.
      if (event.kind === "Thinking") {
        setEntries((prev) => {
          const lastIdx = prev.length - 1;
          const last = lastIdx >= 0 ? prev[lastIdx] : undefined;
          if (last !== undefined && last.event.kind === "Thinking") {
            // Replace the last thinking entry.
            const updated = [...prev];
            updated[lastIdx] = {
              ...last,
              event,
              timestamp: Date.now(),
            };
            return updated;
          }
          // New thinking entry.
          seqRef.current += 1;
          return [
            ...prev,
            { seq: seqRef.current, event, timestamp: Date.now() },
          ];
        });
        return;
      }

      seqRef.current += 1;
      const entry: TimelineEntry = {
        seq: seqRef.current,
        event,
        timestamp: Date.now(),
      };
      setEntries((prev) => [...prev, entry]);
    });

    return () => {
      void unlisten.then((f) => { f(); });
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
  }, []);

  if (!visible) return null;

  return (
    <div className="border-t border-border bg-card flex flex-col max-h-[300px] shrink-0">
      {/* Header */}
      <div className="px-4 py-2 border-b border-border flex items-center gap-3 shrink-0">
        <span className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">
          Agent Activity
        </span>
        {isRunning && (
          <span className="text-xs text-warning">
            {formatDuration(elapsed)}
          </span>
        )}
        {isRunning && (
          <span className="inline-block w-2 h-2 bg-warning rounded-full animate-pulse" />
        )}
        <div className="ml-auto flex items-center gap-2">
          <button
            onClick={handleClear}
            className="text-xs text-muted-foreground hover:text-foreground bg-transparent border-none cursor-pointer"
          >
            Clear
          </button>
          <button
            onClick={onClose}
            className="text-xs text-muted-foreground hover:text-foreground bg-transparent border-none cursor-pointer"
          >
            Close
          </button>
        </div>
      </div>

      {/* Timeline */}
      <div
        ref={scrollRef}
        className="flex-1 overflow-y-auto px-4 py-2 space-y-1 text-[13px] font-mono"
      >
        {entries.length === 0 && (
          <span className="text-muted-foreground text-xs">
            Waiting for agent events...
          </span>
        )}
        {entries.map((entry) => (
          <div key={entry.seq} className="py-0.5">
            {renderEvent(entry)}
          </div>
        ))}
      </div>
    </div>
  );
}
