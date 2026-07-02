import { describe, it, expect } from "vitest";
import {
  sizeClass,
  diffTotals,
  sensitiveFlags,
  hasSensitivePath,
  touchesTests,
} from "./diff-signals";

// The fixtures below are COPIED from `crates/cockpit-core/src/diff_signals.rs`
// (its `diff_adding`, `touch`, and the risk/test-delta unit tests). Keeping the
// exact same inputs on both sides guards against drift between the Rust engine
// and this card-subset TS mirror: if a heuristic changes, both suites move
// together.

/** A diff that adds `n` lines to a plain (non-test) file. Mirrors `diff_adding`. */
function diffAdding(n: number): string {
  let s = "diff --git a/data.txt b/data.txt\n";
  s += "--- a/data.txt\n";
  s += "+++ b/data.txt\n";
  s += `@@ -0,0 +1,${String(n)} @@\n`;
  for (let i = 0; i < n; i++) {
    s += `+row ${String(i)}\n`;
  }
  return s;
}

/** A minimal diff that touches (one context + one added line) `path`. Mirrors `touch`. */
function touch(path: string): string {
  return `diff --git a/${path} b/${path}\n--- a/${path}\n+++ b/${path}\n@@ -1,1 +1,2 @@\n unchanged\n+changed\n`;
}

describe("sizeClass — boundaries mirror classify_size", () => {
  it("S/M boundary at 50 changed lines", () => {
    expect(sizeClass(49, 0)).toBe("S");
    expect(sizeClass(50, 0)).toBe("M");
  });

  it("M/L boundary at 200 changed lines", () => {
    expect(sizeClass(199, 0)).toBe("M");
    expect(sizeClass(200, 0)).toBe("L");
  });

  it("L/Xl boundary at 600 changed lines", () => {
    expect(sizeClass(599, 0)).toBe("L");
    expect(sizeClass(600, 0)).toBe("Xl");
  });

  it("counts additions + deletions together", () => {
    expect(sizeClass(30, 30)).toBe("M");
  });
});

describe("diffTotals", () => {
  it("counts additions from a pure-add diff", () => {
    const totals = diffTotals(diffAdding(50));
    expect(totals.additions).toBe(50);
    expect(totals.deletions).toBe(0);
    expect(totals.filesChanged).toBe(1);
  });

  it("counts a modify (1 add / 1 del) without a phantom header line", () => {
    // Same shape as the Rust `clean_refactor_has_no_weakening` fixture.
    const raw = [
      "diff --git a/src/util.rs b/src/util.rs",
      "--- a/src/util.rs",
      "+++ b/src/util.rs",
      "@@ -1,3 +1,3 @@",
      " fn add(a: i32, b: i32) -> i32 {",
      "-    a + b",
      "+    a.wrapping_add(b)",
      " }",
      "",
    ].join("\n");
    const totals = diffTotals(raw);
    expect(totals.additions).toBe(1);
    expect(totals.deletions).toBe(1);
    expect(totals.filesChanged).toBe(1);
  });

  it("counts files across a multi-file diff", () => {
    expect(diffTotals(touch("a.txt") + touch("b.txt")).filesChanged).toBe(2);
  });
});

describe("sensitiveFlags — mirrors classify_risk priority order", () => {
  it("classifies each path with the expected single flag", () => {
    const cases: readonly [string, RiskFlagValue][] = [
      ["db/migrations/001_init.sql", "Migration"],
      ["Cargo.lock", "Lockfile"],
      [".github/workflows/ci.yml", "CiConfig"],
      ["src/auth.rs", "Auth"],
      [".github/dependabot.yml", "GithubDir"],
      ["Cargo.toml", "Dependency"],
    ];
    for (const [path, expected] of cases) {
      const flags = sensitiveFlags(touch(path));
      expect(flags).toEqual([expected]);
    }
  });

  it("does not flag author.rs as Auth (the `autho` stem is excluded)", () => {
    expect(sensitiveFlags(touch("src/author.rs"))).toEqual([]);
    expect(hasSensitivePath(touch("src/author.rs"))).toBe(false);
  });

  it("hasSensitivePath is true when any file is risky", () => {
    expect(hasSensitivePath(touch("src/auth_token.ts"))).toBe(true);
    expect(hasSensitivePath(touch("src/main.rs"))).toBe(false);
  });
});

describe("touchesTests", () => {
  it("is true for a `test`-containing or `.spec.` path", () => {
    expect(touchesTests(touch("tests/alpha.rs"))).toBe(true);
    expect(touchesTests(touch("src/widget.test.ts"))).toBe(true);
    expect(touchesTests(touch("src/foo.spec.ts"))).toBe(true);
  });

  it("is false for a plain source path", () => {
    expect(touchesTests(touch("src/main.rs"))).toBe(false);
  });
});

/** Local alias so the fixture table stays terse and typed. */
type RiskFlagValue = ReturnType<typeof sensitiveFlags>[number];
