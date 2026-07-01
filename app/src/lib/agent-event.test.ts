import { describe, it, expect } from "vitest";
import type { Event } from "../bindings/Event";
import {
  presentEvent,
  toneTextClass,
  toneTileClass,
  type EventTone,
} from "./agent-event";

/** One representative payload per event variant. */
const samples: readonly Event[] = [
  { kind: "Init", model: "claude", session_id: "s", tools: ["Read", "Bash"] },
  { kind: "Thinking", estimated_tokens: 120, delta: 12 },
  { kind: "ToolUse", id: "t1", name: "Read", input_summary: "src/lib.rs" },
  { kind: "ToolUse", id: "t2", name: "Bash", input_summary: "cargo test" },
  { kind: "ToolResult", tool_use_id: "t1", success: true, summary: "ok" },
  { kind: "ToolResult", tool_use_id: "t2", success: false, summary: "boom" },
  { kind: "Text", content: "hello" },
  { kind: "SubagentSpawn", id: "a1", prompt: "do the thing" },
  { kind: "SubagentResult", tool_use_id: "a1", result: "done" },
  { kind: "RateLimit", status: "retrying in 30s" },
  {
    kind: "Complete",
    duration_ms: 1000,
    cost_usd: 0.02,
    num_turns: 3,
    output_tokens: 400,
    result_text: "reworked",
  },
  { kind: "Error", message: "agent died" },
];

describe("presentEvent", () => {
  it("returns a non-empty title and an icon component for every variant", () => {
    for (const event of samples) {
      const p = presentEvent(event);
      expect(p.title).not.toBe("");
      // lucide icons are forwardRef objects; assert one was returned.
      expect(p.icon).toBeDefined();
    }
  });

  it("maps tool use to the brand tone and tool results to success/danger", () => {
    expect(presentEvent(samples[2]!).tone).toBe("brand");
    expect(presentEvent(samples[4]!).tone).toBe("success");
    expect(presentEvent(samples[5]!).tone).toBe("danger");
  });

  it("maps thinking to the reworked/violet tone and errors to danger", () => {
    expect(presentEvent(samples[1]!).tone).toBe("thinking");
    expect(presentEvent(samples[11]!).tone).toBe("danger");
  });

  it("uses distinct tool icons per tool name", () => {
    const read = presentEvent(samples[2]!).icon;
    const bash = presentEvent(samples[3]!).icon;
    expect(read).not.toBe(bash);
  });
});

describe("tone class maps", () => {
  const tones: readonly EventTone[] = [
    "brand",
    "thinking",
    "progress",
    "success",
    "danger",
    "warning",
    "neutral",
  ];

  it("returns a class string for every tone", () => {
    for (const tone of tones) {
      expect(toneTextClass(tone)).not.toBe("");
      expect(toneTileClass(tone)).not.toBe("");
    }
  });

  it("routes thinking to the reworked color and brand to teal", () => {
    expect(toneTextClass("thinking")).toContain("state-reworked");
    expect(toneTextClass("brand")).toContain("brand");
  });
});
