import { describe, it, expect } from "vitest";
import { cardSignal } from "./card-signal";
import { makeReview } from "../test/fixtures";
import type { CiSummary } from "../bindings/CiSummary";

/** A diff adding `n` lines to `path`. */
function addLines(path: string, n: number): string {
  let s = `diff --git a/${path} b/${path}\n--- a/${path}\n+++ b/${path}\n@@ -0,0 +1,${String(n)} @@\n`;
  for (let i = 0; i < n; i++) {
    s += `+row ${String(i)}\n`;
  }
  return s;
}

const CI_FAIL: CiSummary = { passed: 1, total: 2, failed: 1, pending: 0 };
const LARGE = addLines("data.txt", 500);

describe("cardSignal — risk note layering", () => {
  it("keeps the gate reason as the headline and adds the risk note", () => {
    const signal = cardSignal(
      makeReview({ gate_state: "Pending", ci_summary: CI_FAIL }),
    );
    expect(signal.reason).toBe("Needs your review");
    expect(signal.note).toBe("CI failing");
  });

  it("notes the sharpest reason for a reworked review", () => {
    const signal = cardSignal(
      makeReview({ gate_state: "Reworked", diff: { raw: LARGE } }),
    );
    expect(signal.reason).toBe("Agent reworked — re-review");
    expect(signal.note).toBe("Large diff");
  });

  it("omits the note for a clean actionable review", () => {
    const signal = cardSignal(makeReview({ gate_state: "InReview" }));
    expect(signal.note).toBeUndefined();
  });

  it("lets the stale reason take precedence over any risk note", () => {
    const signal = cardSignal(
      makeReview({ stale: true, gate_state: "InReview", ci_summary: CI_FAIL }),
    );
    expect(signal.reason).toBe("Restack needed");
    expect(signal.note).toBeUndefined();
  });

  it("does not note risk on non-actionable states", () => {
    const dispatched = cardSignal(
      makeReview({ gate_state: "Dispatched", ci_summary: CI_FAIL }),
    );
    const approved = cardSignal(
      makeReview({ gate_state: "Approved", ci_summary: CI_FAIL }),
    );
    expect(dispatched.note).toBeUndefined();
    expect(approved.note).toBeUndefined();
  });
});
