/**
 * Presentation mapping for agent stream events.
 *
 * Turns each {@link Event} variant into a designed timeline row: an SVG icon,
 * a semantic tone (color), a title, and an optional faint detail line. The
 * mapping is exhaustive over the event union via {@link assertNever}, so a new
 * Rust variant surfaces as a compile error here rather than a silent gap.
 *
 * No emoji: icons are lucide React components rendered by the timeline.
 */

import type { ComponentType } from "react";
import type { Event } from "../bindings/Event";
import {
  Sparkles,
  Brain,
  FileText,
  Pencil,
  FilePlus,
  Terminal,
  Wrench,
  CheckCircle2,
  XCircle,
  Boxes,
  AlertTriangle,
  MessageSquare,
  Flag,
} from "lucide-react";

/** Minimal shape a lucide icon satisfies; avoids importing lucide's props type. */
export type IconComponent = ComponentType<{ readonly className?: string }>;

/**
 * Semantic tone for a timeline row, mapped to the Glass Cockpit status set.
 *
 * - `brand` — tool activity (HUD teal)
 * - `thinking` — reasoning (reworked violet)
 * - `progress` — git/push style progress (in-review blue)
 * - `success` — completion / ok result
 * - `danger` — failure / error
 * - `warning` — rate limit and other soft alerts
 * - `neutral` — informational (init, plain text)
 */
export type EventTone =
  | "brand"
  | "thinking"
  | "progress"
  | "success"
  | "danger"
  | "warning"
  | "neutral";

/** A fully resolved presentation for one timeline row. */
export interface EventPresentation {
  readonly icon: IconComponent;
  readonly tone: EventTone;
  /** Bold leading label (display/sans). */
  readonly title: string;
  /** Optional mono/faint detail line; empty string means "no detail". */
  readonly detail: string;
}

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** Pick an icon for a tool-use row by tool name. */
function toolIconFor(name: string): IconComponent {
  switch (name) {
    case "Read":
      return FileText;
    case "Edit":
      return Pencil;
    case "Write":
      return FilePlus;
    case "Bash":
      return Terminal;
    case "Agent":
    case "Skill":
    case "Task":
      return Boxes;
    default:
      return Wrench;
  }
}

/**
 * Resolve the icon / tone / title / detail for a single agent event.
 *
 * Pure and exhaustive over the {@link Event} union — the sole source of truth
 * for how the timeline renders each variant.
 */
export function presentEvent(event: Event): EventPresentation {
  switch (event.kind) {
    case "Init":
      return {
        icon: Flag,
        tone: "neutral",
        title: "Session started",
        detail: `${event.model} · ${String(event.tools.length)} tools`,
      };
    case "Thinking":
      return {
        icon: Brain,
        tone: "thinking",
        title: "Thinking",
        detail: `${String(event.estimated_tokens)} tokens`,
      };
    case "ToolUse":
      return {
        icon: toolIconFor(event.name),
        tone: "brand",
        title: event.name,
        detail: event.input_summary,
      };
    case "ToolResult":
      return {
        icon: event.success ? CheckCircle2 : XCircle,
        tone: event.success ? "success" : "danger",
        title: event.success ? "Result" : "Tool failed",
        detail: event.summary,
      };
    case "Text":
      return {
        icon: MessageSquare,
        tone: "neutral",
        title: "Message",
        detail: event.content,
      };
    case "SubagentSpawn":
      return {
        icon: Boxes,
        tone: "progress",
        title: "Subagent spawned",
        detail: event.prompt,
      };
    case "SubagentResult":
      return {
        icon: Boxes,
        tone: "success",
        title: "Subagent done",
        detail: event.result,
      };
    case "RateLimit":
      return {
        icon: AlertTriangle,
        tone: "warning",
        title: "Rate limited",
        detail: event.status,
      };
    case "Complete":
      return {
        icon: CheckCircle2,
        tone: "success",
        title: "Complete",
        detail: event.result_text,
      };
    case "Error":
      return {
        icon: XCircle,
        tone: "danger",
        title: "Failed",
        detail: event.message,
      };
    default:
      return assertNever(event);
  }
}

/** Sparkles is exported for the header LED / brand mark of the timeline. */
export const AgentMark: IconComponent = Sparkles;

/**
 * Tailwind text-color class for a tone. Kept as a lookup so the timeline and
 * any tests agree on the exact class strings.
 */
export function toneTextClass(tone: EventTone): string {
  switch (tone) {
    case "brand":
      return "text-brand";
    case "thinking":
      return "text-state-reworked";
    case "progress":
      return "text-state-in-review";
    case "success":
      return "text-success";
    case "danger":
      return "text-danger";
    case "warning":
      return "text-warning";
    case "neutral":
      return "text-muted-foreground";
    default:
      return assertNever(tone);
  }
}

/**
 * Tailwind classes for the icon tile (background tint + ring) for a tone.
 * The border-color uses the current text color so the tile ring matches.
 */
export function toneTileClass(tone: EventTone): string {
  switch (tone) {
    case "brand":
      return "bg-brand/10 text-brand ring-1 ring-brand/25";
    case "thinking":
      return "bg-state-reworked/10 text-state-reworked ring-1 ring-state-reworked/25";
    case "progress":
      return "bg-state-in-review/10 text-state-in-review ring-1 ring-state-in-review/25";
    case "success":
      return "bg-success/10 text-success ring-1 ring-success/25";
    case "danger":
      return "bg-danger/10 text-danger ring-1 ring-danger/25";
    case "warning":
      return "bg-warning/10 text-warning ring-1 ring-warning/25";
    case "neutral":
      return "bg-muted text-muted-foreground ring-1 ring-border";
    default:
      return assertNever(tone);
  }
}
