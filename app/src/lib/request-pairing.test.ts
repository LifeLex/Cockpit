import { describe, it, expect } from "vitest";
import { pairRequests } from "./request-pairing";
import { parseDiff } from "../diff-parser";
import type { Comment } from "../bindings/Comment";
import type { DiffSide } from "../bindings/DiffSide";
import type { DispatchSnapshot } from "../bindings/DispatchSnapshot";

// A diff-line comment on `path`, `range`, `side`. Built against the real
// bindings so a schema change breaks these tests at compile time.
function makeComment(
  id: string,
  path: string,
  range: [number, number],
  side: DiffSide,
): Comment {
  return {
    id,
    anchor: { DiffLine: { path, range, side } },
    body: `comment ${id}`,
    origin: "Local",
  };
}

function snapshot(comments: Comment[]): DispatchSnapshot {
  return { reviewed_sha: "sha", comments };
}

// An interdiff whose NEW side touches real lines 10..14 on src/a.ts.
const A_NEW_10_14 = [
  "diff --git a/src/a.ts b/src/a.ts",
  "@@ -9,0 +10,5 @@",
  "+ten",
  "+eleven",
  "+twelve",
  "+thirteen",
  "+fourteen",
].join("\n");

// An interdiff whose OLD side deletes real lines 5..8 on src/b.ts.
const B_OLD_5_8 = [
  "diff --git a/src/b.ts b/src/b.ts",
  "@@ -5,4 +4,0 @@",
  "-five",
  "-six",
  "-seven",
  "-eight",
].join("\n");

describe("pairRequests", () => {
  it("matches a request whose range overlaps a hunk exactly", () => {
    const files = parseDiff(A_NEW_10_14);
    const result = pairRequests(
      snapshot([makeComment("c1", "src/a.ts", [11, 12], "New")]),
      files,
    );

    expect(result).toHaveLength(1);
    // Nearest contained line ties to the lower of the two zero-distance lines.
    expect(result[0]?.match).toEqual({ path: "src/a.ts", realLine: 11 });
  });

  it("matches a request within ±10 lines of a hunk (boundary)", () => {
    const files = parseDiff(A_NEW_10_14);
    // 24 is exactly 10 lines past the span end (14) — still a match.
    const result = pairRequests(
      snapshot([makeComment("c1", "src/a.ts", [24, 24], "New")]),
      files,
    );

    expect(result[0]?.match).toEqual({ path: "src/a.ts", realLine: 14 });
  });

  it("does not match a request outside the ±10 window", () => {
    const files = parseDiff(A_NEW_10_14);
    // 25 is 11 lines past the span end (14) — out of range.
    const result = pairRequests(
      snapshot([makeComment("c1", "src/a.ts", [25, 25], "New")]),
      files,
    );

    expect(result[0]?.match).toBeNull();
  });

  it("does not match a request on a file the interdiff never touches", () => {
    const files = parseDiff(A_NEW_10_14);
    const result = pairRequests(
      snapshot([makeComment("c1", "src/other.ts", [11, 12], "New")]),
      files,
    );

    expect(result[0]?.match).toBeNull();
  });

  it("matches a deleted-side (Old) request against an Old-side span", () => {
    const files = parseDiff(B_OLD_5_8);
    const result = pairRequests(
      snapshot([makeComment("c1", "src/b.ts", [6, 6], "Old")]),
      files,
    );

    expect(result[0]?.match).toEqual({ path: "src/b.ts", realLine: 6 });
  });

  it("is side-aware: a New-side request finds nothing on a pure deletion", () => {
    const files = parseDiff(B_OLD_5_8);
    const result = pairRequests(
      snapshot([makeComment("c1", "src/b.ts", [6, 6], "New")]),
      files,
    );

    expect(result[0]?.match).toBeNull();
  });

  it("pairs each request against its own file across a multi-file interdiff", () => {
    const files = parseDiff(`${A_NEW_10_14}\n${B_OLD_5_8}`);
    const result = pairRequests(
      snapshot([
        makeComment("c1", "src/a.ts", [10, 10], "New"),
        makeComment("c2", "src/b.ts", [7, 7], "Old"),
      ]),
      files,
    );

    expect(result[0]?.match).toEqual({ path: "src/a.ts", realLine: 10 });
    expect(result[1]?.match).toEqual({ path: "src/b.ts", realLine: 7 });
  });

  it("returns all-null matches for an empty interdiff", () => {
    const result = pairRequests(
      snapshot([
        makeComment("c1", "src/a.ts", [11, 12], "New"),
        makeComment("c2", "src/b.ts", [6, 6], "Old"),
      ]),
      [],
    );

    expect(result.map((r) => r.match)).toEqual([null, null]);
  });

  it("is total for a non-diff (plan) anchor", () => {
    const files = parseDiff(A_NEW_10_14);
    const planComment: Comment = {
      id: "p1",
      anchor: { PlanStep: 2 },
      body: "plan note",
      origin: "Local",
    };
    const result = pairRequests(snapshot([planComment]), files);

    expect(result).toHaveLength(1);
    expect(result[0]?.match).toBeNull();
    expect(result[0]?.comment.id).toBe("p1");
  });
});
