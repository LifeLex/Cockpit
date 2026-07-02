import { describe, it, expect } from "vitest";
import type { FileDiff } from "./diff-parser";
import {
  parseDiff,
  fragmentToReal,
  realToFragment,
  extractFilePaths,
  diffStats,
  identityLineMap,
} from "./diff-parser";

/** Fetch the single parsed file, failing loudly if the diff produced none. */
function only(files: readonly FileDiff[]): FileDiff {
  const file = files[0];
  if (file === undefined) {
    throw new Error("expected at least one parsed file");
  }
  return file;
}

describe("parseDiff line mapping — single hunk deep in a file", () => {
  // Hunk starts at old line 100 / new line 100.
  const raw = [
    "diff --git a/src/deep.ts b/src/deep.ts",
    "index 111..222 100644",
    "--- a/src/deep.ts",
    "+++ b/src/deep.ts",
    "@@ -100,4 +100,5 @@",
    " ctx-a",
    " ctx-b",
    "-old-line",
    "+new-line-1",
    "+new-line-2",
    " ctx-c",
  ].join("\n");
  const file = only(parseDiff(raw));

  it("maps new-side fragment line k to real line c+k-1", () => {
    // Fragment new lines: 1 ctx-a(100), 2 ctx-b(101), 3 new-line-1(102),
    // 4 new-line-2(103), 5 ctx-c(104).
    expect(fragmentToReal(file, "New", 1)).toBe(100);
    expect(fragmentToReal(file, "New", 3)).toBe(102);
    expect(fragmentToReal(file, "New", 5)).toBe(104);
  });

  it("maps old-side fragment lines to real old lines", () => {
    // Fragment old lines: 1 ctx-a(100), 2 ctx-b(101), 3 old-line(102),
    // 4 ctx-c(103).
    expect(fragmentToReal(file, "Old", 1)).toBe(100);
    expect(fragmentToReal(file, "Old", 3)).toBe(102);
    expect(fragmentToReal(file, "Old", 4)).toBe(103);
  });

  it("returns undefined past the fragment end and below line 1", () => {
    expect(fragmentToReal(file, "New", 6)).toBeUndefined();
    expect(fragmentToReal(file, "New", 0)).toBeUndefined();
    expect(fragmentToReal(file, "Old", 5)).toBeUndefined();
  });
});

describe("identityLineMap — full-file view (B4)", () => {
  it("maps every fragment line to the same real line on both sides", () => {
    const file: FileDiff = {
      path: "src/full.ts",
      original: "a\nb\nc",
      modified: "a\nB\nc\nd",
      lineMap: identityLineMap("a\nb\nc", "a\nB\nc\nd"),
    };
    expect(fragmentToReal(file, "Old", 1)).toBe(1);
    expect(fragmentToReal(file, "Old", 3)).toBe(3);
    expect(fragmentToReal(file, "New", 4)).toBe(4);
    expect(realToFragment(file, "Old", 2)).toBe(2);
    expect(realToFragment(file, "New", 4)).toBe(4);
  });

  it("returns undefined past each side's line count", () => {
    const file: FileDiff = {
      path: "src/full.ts",
      original: "a\nb\nc",
      modified: "a\nB\nc\nd",
      lineMap: identityLineMap("a\nb\nc", "a\nB\nc\nd"),
    };
    expect(fragmentToReal(file, "Old", 4)).toBeUndefined();
    expect(fragmentToReal(file, "New", 5)).toBeUndefined();
  });

  it("treats an empty side (added/deleted file) as zero lines", () => {
    const map = identityLineMap("", "x\ny");
    expect(map.Old.toReal).toEqual([]);
    expect(map.New.toReal).toEqual([1, 2]);
  });
});

describe("parseDiff line mapping — multiple non-contiguous hunks", () => {
  // Two hunks separated by a large gap; this is the case the fragment
  // reconstruction would otherwise mis-number.
  const raw = [
    "diff --git a/src/multi.ts b/src/multi.ts",
    "--- a/src/multi.ts",
    "+++ b/src/multi.ts",
    "@@ -1,3 +1,3 @@",
    " a",
    "-b",
    "+B",
    " c",
    "@@ -50,3 +50,4 @@",
    " x",
    "+Y",
    " y",
    " z",
  ].join("\n");
  const file = only(parseDiff(raw));

  it("keeps first-hunk lines near the top of the file", () => {
    expect(fragmentToReal(file, "New", 1)).toBe(1); // a
    expect(fragmentToReal(file, "New", 2)).toBe(2); // B
    expect(fragmentToReal(file, "New", 3)).toBe(3); // c
  });

  it("maps lines after the second header to real lines ~50, not ~4", () => {
    // Fragment new lines continue: 4 x(50), 5 Y(51), 6 y(52), 7 z(53).
    expect(fragmentToReal(file, "New", 4)).toBe(50);
    expect(fragmentToReal(file, "New", 5)).toBe(51);
    expect(fragmentToReal(file, "New", 7)).toBe(53);
  });

  it("maps the old side across the gap too", () => {
    // Old fragment: 1 a(1), 2 b(2), 3 c(3), 4 x(50), 5 y(51), 6 z(52).
    expect(fragmentToReal(file, "Old", 4)).toBe(50);
    expect(fragmentToReal(file, "Old", 6)).toBe(52);
  });

  it("real lines inside the collapsed gap map to no fragment line", () => {
    // Real new line 10 sits in the gap between hunk 1 (ends at 3) and hunk 2
    // (starts at 50), so it appears in no fragment.
    expect(realToFragment(file, "New", 10)).toBeUndefined();
    expect(realToFragment(file, "Old", 25)).toBeUndefined();
  });
});

describe("parseDiff line mapping — added file", () => {
  const raw = [
    "diff --git a/src/added.ts b/src/added.ts",
    "new file mode 100644",
    "index 000..333 100644",
    "--- /dev/null",
    "+++ b/src/added.ts",
    "@@ -0,0 +1,3 @@",
    "+one",
    "+two",
    "+three",
  ].join("\n");
  const file = only(parseDiff(raw));

  it("maps the new side 1:1", () => {
    expect(fragmentToReal(file, "New", 1)).toBe(1);
    expect(fragmentToReal(file, "New", 3)).toBe(3);
    expect(realToFragment(file, "New", 2)).toBe(2);
  });

  it("has no old side", () => {
    expect(fragmentToReal(file, "Old", 1)).toBeUndefined();
    expect(realToFragment(file, "Old", 1)).toBeUndefined();
    expect(file.original).toBe("");
  });
});

describe("parseDiff line mapping — deleted file", () => {
  const raw = [
    "diff --git a/src/gone.ts b/src/gone.ts",
    "deleted file mode 100644",
    "index 444..000 100644",
    "--- a/src/gone.ts",
    "+++ /dev/null",
    "@@ -1,3 +0,0 @@",
    "-alpha",
    "-beta",
    "-gamma",
  ].join("\n");
  const file = only(parseDiff(raw));

  it("maps the old side 1:1", () => {
    expect(fragmentToReal(file, "Old", 1)).toBe(1);
    expect(fragmentToReal(file, "Old", 3)).toBe(3);
    expect(realToFragment(file, "Old", 2)).toBe(2);
  });

  it("has no new side", () => {
    expect(fragmentToReal(file, "New", 1)).toBeUndefined();
    expect(realToFragment(file, "New", 1)).toBeUndefined();
    expect(file.modified).toBe("");
  });
});

describe("parseDiff line mapping — round trip", () => {
  const raw = [
    "diff --git a/src/round.ts b/src/round.ts",
    "--- a/src/round.ts",
    "+++ b/src/round.ts",
    "@@ -10,4 +10,4 @@",
    " keep-1",
    "-drop",
    "+add",
    " keep-2",
    " keep-3",
  ].join("\n");
  const file = only(parseDiff(raw));

  it("realToFragment(fragmentToReal(x)) === x for present lines", () => {
    for (const side of ["Old", "New"] as const) {
      for (let frag = 1; frag <= 4; frag += 1) {
        const real = fragmentToReal(file, side, frag);
        expect(real).toBeDefined();
        if (real !== undefined) {
          expect(realToFragment(file, side, real)).toBe(frag);
        }
      }
    }
  });

  it("fragmentToReal(realToFragment(y)) === y for present real lines", () => {
    // New side real lines present: 10 keep-1, 11 add, 12 keep-2, 13 keep-3.
    for (const real of [10, 11, 12, 13]) {
      const frag = realToFragment(file, "New", real);
      expect(frag).toBeDefined();
      if (frag !== undefined) {
        expect(fragmentToReal(file, "New", frag)).toBe(real);
      }
    }
  });

  it("returns undefined for real lines not in any hunk", () => {
    expect(realToFragment(file, "New", 1)).toBeUndefined();
    expect(realToFragment(file, "New", 999)).toBeUndefined();
    expect(realToFragment(file, "Old", 999)).toBeUndefined();
  });
});

describe("parseDiff line mapping — omitted-count header forms", () => {
  it("parses @@ -a +c @@ (both counts omitted, implicit 1)", () => {
    const raw = [
      "diff --git a/src/one.ts b/src/one.ts",
      "--- a/src/one.ts",
      "+++ b/src/one.ts",
      "@@ -7 +7 @@",
      "-was",
      "+now",
    ].join("\n");
    const file = only(parseDiff(raw));
    expect(fragmentToReal(file, "Old", 1)).toBe(7);
    expect(fragmentToReal(file, "New", 1)).toBe(7);
  });

  it("parses @@ -a,0 +c,n @@ (zero old count, pure insertion)", () => {
    const raw = [
      "diff --git a/src/two.ts b/src/two.ts",
      "--- a/src/two.ts",
      "+++ b/src/two.ts",
      "@@ -20,0 +21,2 @@",
      "+inserted-1",
      "+inserted-2",
    ].join("\n");
    const file = only(parseDiff(raw));
    expect(fragmentToReal(file, "New", 1)).toBe(21);
    expect(fragmentToReal(file, "New", 2)).toBe(22);
    expect(fragmentToReal(file, "Old", 1)).toBeUndefined();
  });

  it("parses @@ -a,n +c,0 @@ (zero new count, pure deletion)", () => {
    const raw = [
      "diff --git a/src/three.ts b/src/three.ts",
      "--- a/src/three.ts",
      "+++ b/src/three.ts",
      "@@ -30,2 +29,0 @@",
      "-removed-1",
      "-removed-2",
    ].join("\n");
    const file = only(parseDiff(raw));
    expect(fragmentToReal(file, "Old", 1)).toBe(30);
    expect(fragmentToReal(file, "Old", 2)).toBe(31);
    expect(fragmentToReal(file, "New", 1)).toBeUndefined();
  });
});

describe("parseDiff — existing API stays backward-compatible", () => {
  const raw = [
    "diff --git a/src/compat.ts b/src/compat.ts",
    "--- a/src/compat.ts",
    "+++ b/src/compat.ts",
    "@@ -1,2 +1,2 @@",
    " unchanged",
    "-before",
    "+after",
  ].join("\n");

  it("still reconstructs original/modified fragments", () => {
    const file = only(parseDiff(raw));
    expect(file.path).toBe("src/compat.ts");
    expect(file.original).toBe("unchanged\nbefore");
    expect(file.modified).toBe("unchanged\nafter");
  });

  it("extractFilePaths and diffStats are unaffected", () => {
    expect(extractFilePaths(raw)).toEqual(["src/compat.ts"]);
    expect(diffStats(raw)).toEqual({ additions: 1, deletions: 1 });
  });

  it("returns [] for empty input", () => {
    expect(parseDiff("")).toEqual([]);
    expect(parseDiff("   \n  ")).toEqual([]);
  });
});
