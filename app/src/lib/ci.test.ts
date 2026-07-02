import { describe, it, expect } from "vitest";
import type { CiCheck } from "../bindings/CiCheck";
import {
  checkOutcome,
  summarizeChecks,
  ciState,
  parseCiUpdate,
} from "./ci";

/** Build a CiCheck with sane defaults; override the fields under test. */
function check(overrides: Partial<CiCheck>): CiCheck {
  return {
    name: "build",
    state: "SUCCESS",
    bucket: "pass",
    link: "https://github.com/o/r/runs/1",
    workflow: "CI",
    ...overrides,
  };
}

describe("checkOutcome", () => {
  it("classifies bucket 'pass' as pass", () => {
    expect(checkOutcome(check({ bucket: "pass" }))).toBe("pass");
  });

  it("classifies bucket 'fail' as fail", () => {
    expect(checkOutcome(check({ bucket: "fail" }))).toBe("fail");
  });

  it("classifies bucket 'pending' as pending", () => {
    expect(checkOutcome(check({ bucket: "pending" }))).toBe("pending");
  });

  it("treats neutral/skipped/cancelled as pass", () => {
    for (const signal of ["neutral", "skipped", "cancelled", "canceled"]) {
      expect(checkOutcome(check({ bucket: signal }))).toBe("pass");
    }
  });

  it("falls back to raw state when bucket is empty", () => {
    // Empty bucket means "use `state`": SUCCESS -> pass, FAILURE -> fail.
    expect(checkOutcome(check({ bucket: "", state: "SUCCESS" }))).toBe("pass");
    expect(checkOutcome(check({ bucket: "", state: "FAILURE" }))).toBe("fail");
    expect(checkOutcome(check({ bucket: "", state: "timed_out" }))).toBe(
      "fail",
    );
  });

  it("classifies commit-status 'error' as fail (mirrors Rust classify_check_signal)", () => {
    // The legacy commit-status `ERROR` state is a failure — it must not fall
    // through to the pending default. Mirrors the Rust `error` -> fail arm.
    expect(checkOutcome(check({ bucket: "error" }))).toBe("fail");
    expect(checkOutcome(check({ bucket: "", state: "ERROR" }))).toBe("fail");
  });

  it("is case-insensitive on the signal", () => {
    expect(checkOutcome(check({ bucket: "PASS" }))).toBe("pass");
    expect(checkOutcome(check({ bucket: "", state: "Failure" }))).toBe("fail");
  });

  it("maps unknown signals to pending", () => {
    expect(checkOutcome(check({ bucket: "in_progress" }))).toBe("pending");
    expect(checkOutcome(check({ bucket: "", state: "queued" }))).toBe(
      "pending",
    );
  });
});

describe("summarizeChecks", () => {
  it("returns all-zero for empty input", () => {
    expect(summarizeChecks([])).toEqual({
      passed: 0,
      failed: 0,
      pending: 0,
      total: 0,
    });
  });

  it("rolls up a mixed set counting skipped as passed", () => {
    const summary = summarizeChecks([
      check({ bucket: "pass" }),
      check({ bucket: "skipped" }), // counts as passed
      check({ bucket: "fail" }),
      check({ bucket: "pending" }),
      check({ bucket: "queued" }), // unknown -> pending
    ]);
    expect(summary).toEqual({ passed: 2, failed: 1, pending: 2, total: 5 });
  });
});

describe("ciState", () => {
  it("is 'none' when there are no checks", () => {
    expect(ciState({ passed: 0, failed: 0, pending: 0, total: 0 })).toBe(
      "none",
    );
  });

  it("prioritizes fail over pending", () => {
    expect(ciState({ passed: 1, failed: 1, pending: 3, total: 5 })).toBe(
      "fail",
    );
  });

  it("is 'pending' when only pending remains", () => {
    expect(ciState({ passed: 2, failed: 0, pending: 1, total: 3 })).toBe(
      "pending",
    );
  });

  it("is 'pass' when everything passed", () => {
    expect(ciState({ passed: 3, failed: 0, pending: 0, total: 3 })).toBe(
      "pass",
    );
  });
});

describe("parseCiUpdate", () => {
  it("parses a well-formed [pr, checks] tuple", () => {
    const c = check({ name: "lint" });
    const result = parseCiUpdate(["https://gh/pr/1", [c]]);
    expect(result).not.toBeNull();
    // Real guard, not a cast: only proceed once we know it parsed.
    if (result === null) throw new Error("expected a parsed update");
    expect(result.pr).toBe("https://gh/pr/1");
    expect(result.checks).toHaveLength(1);
    const [only] = result.checks;
    expect(only?.name).toBe("lint");
  });

  it("rejects a payload that is not a 2-tuple", () => {
    expect(parseCiUpdate(null)).toBeNull();
    expect(parseCiUpdate("nope")).toBeNull();
    expect(parseCiUpdate([])).toBeNull();
    expect(parseCiUpdate(["only-one"])).toBeNull();
    expect(parseCiUpdate(["pr", [], "extra"])).toBeNull();
  });

  it("rejects when pr is not a string or checks is not an array", () => {
    expect(parseCiUpdate([1, []])).toBeNull();
    expect(parseCiUpdate(["pr", "not-an-array"])).toBeNull();
  });

  it("drops malformed checks but keeps well-formed ones", () => {
    const good = check({ name: "good" });
    const result = parseCiUpdate([
      "pr",
      [
        good,
        { name: "missing-fields" }, // missing state/bucket/link/workflow
        { name: 1, state: "S", bucket: "b", link: "l", workflow: "w" }, // wrong type
        null,
        "string-check",
      ],
    ]);
    if (result === null) throw new Error("expected a parsed update");
    expect(result.checks).toHaveLength(1);
    const [only] = result.checks;
    expect(only?.name).toBe("good");
  });
});
