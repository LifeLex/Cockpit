/**
 * Utilities for parsing unified diff text into structures the Monaco
 * DiffEditor can consume (original + modified content per file).
 *
 * A unified diff contains one or more file hunks. Each hunk has an `@@`
 * header describing the affected line ranges, followed by context (`" "`),
 * added (`"+"`), and removed (`"-"`) lines.
 *
 * The reconstructed `original`/`modified` strings only contain the lines that
 * appear in the diff — hunk gaps are collapsed — so an editor line in those
 * fragments does NOT equal the real file line. Every {@link FileDiff} therefore
 * carries a {@link LineMap} translating between fragment (editor) lines and real
 * file lines on each {@link DiffSide}. Use {@link fragmentToReal} and
 * {@link realToFragment} rather than reading the map directly.
 */

import type { DiffSide } from "@/bindings/DiffSide";

/**
 * Bidirectional line mapping for one side of a file's diff.
 *
 * Fragment lines are the 1-based line numbers within the reconstructed
 * `original`/`modified` text (i.e. the lines Monaco renders). Real lines are
 * the 1-based line numbers in the actual pre-/post-change file.
 */
interface SideLineMap {
  /**
   * Fragment line → real file line. Indexed by `fragmentLine - 1`; a `number`
   * at an index means that editor line exists on this side, `undefined` means
   * it does not.
   */
  readonly toReal: readonly number[];
  /** Real file line → fragment (editor) line for lines present in the diff. */
  readonly toFragment: ReadonlyMap<number, number>;
}

/** Per-file line mapping keyed by {@link DiffSide} (`"Old"` / `"New"`). */
type LineMap = Record<DiffSide, SideLineMap>;

/** A single file's diff, split into the text Monaco expects. */
interface FileDiff {
  /** Path relative to repo root. */
  readonly path: string;
  /** Content before the change (lines prefixed with `-` or ` ` in the diff). */
  readonly original: string;
  /** Content after the change (lines prefixed with `+` or ` ` in the diff). */
  readonly modified: string;
  /**
   * Translation between fragment (editor) lines and real file lines for both
   * sides. Always present on results from {@link parseDiff}; optional so
   * existing callers that synthesize a bare {@link FileDiff} keep compiling.
   * Consume via {@link fragmentToReal} / {@link realToFragment}.
   */
  readonly lineMap?: LineMap;
}

/** Mutable accumulator used while a single file's hunks are parsed. */
interface FileAccumulator {
  path: string;
  originalLines: string[];
  modifiedLines: string[];
  oldToReal: number[];
  newToReal: number[];
  oldToFragment: Map<number, number>;
  newToFragment: Map<number, number>;
  /** Next real old-file line number to assign; set by each hunk header. */
  oldLine: number;
  /** Next real new-file line number to assign; set by each hunk header. */
  newLine: number;
}

/** Matches `@@ -a,b +c,d @@`, tolerating the omitted-count forms `-a`/`+c`. */
const HUNK_HEADER = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/**
 * Parse a raw unified diff string into per-file original/modified pairs.
 *
 * The parser handles:
 * - Multiple files in a single diff
 * - `diff --git a/... b/...` headers
 * - `---`/`+++` file headers
 * - `@@` hunk headers, including the omitted-count forms (`@@ -a +c @@`)
 * - Context, addition, and deletion lines
 * - New files (original is empty) and deleted files (modified is empty)
 * - Non-contiguous hunks (gaps are collapsed in the fragment text but the
 *   {@link LineMap} preserves the real line numbers)
 */
function parseDiff(raw: string): readonly FileDiff[] {
  if (raw.trim() === "") {
    return [];
  }

  const results: FileDiff[] = [];
  const lines = raw.split("\n");

  let acc: FileAccumulator | null = null;

  function newAccumulator(path: string): FileAccumulator {
    return {
      path,
      originalLines: [],
      modifiedLines: [],
      oldToReal: [],
      newToReal: [],
      oldToFragment: new Map(),
      newToFragment: new Map(),
      oldLine: 1,
      newLine: 1,
    };
  }

  function flushFile(): void {
    if (acc !== null) {
      results.push({
        path: acc.path,
        original: acc.originalLines.join("\n"),
        modified: acc.modifiedLines.join("\n"),
        lineMap: {
          Old: { toReal: acc.oldToReal, toFragment: acc.oldToFragment },
          New: { toReal: acc.newToReal, toFragment: acc.newToFragment },
        },
      });
    }
    acc = null;
  }

  for (const line of lines) {
    // New file header: diff --git a/path b/path
    if (line.startsWith("diff --git ")) {
      flushFile();
      // Extract path from "diff --git a/foo b/foo"
      const match = /^diff --git a\/.+ b\/(.+)$/.exec(line);
      if (match?.[1] !== undefined) {
        acc = newAccumulator(match[1]);
      }
      continue;
    }

    // Skip metadata lines that aren't hunk content
    if (
      line.startsWith("index ") ||
      line.startsWith("--- ") ||
      line.startsWith("+++ ") ||
      line.startsWith("new file mode") ||
      line.startsWith("deleted file mode") ||
      line.startsWith("old mode") ||
      line.startsWith("new mode") ||
      line.startsWith("similarity index") ||
      line.startsWith("rename from") ||
      line.startsWith("rename to") ||
      line.startsWith("Binary files")
    ) {
      continue;
    }

    // Hunk header: @@ -a,b +c,d @@ — reset the real line counters for this hunk.
    if (line.startsWith("@@")) {
      const header = HUNK_HEADER.exec(line);
      if (acc !== null && header?.[1] !== undefined && header[3] !== undefined) {
        acc.oldLine = Number.parseInt(header[1], 10);
        acc.newLine = Number.parseInt(header[3], 10);
      }
      continue;
    }

    // No-newline marker
    if (line === "\\ No newline at end of file") {
      continue;
    }

    if (acc === null) {
      continue;
    }

    // Context line (unchanged): advances both sides.
    if (line.startsWith(" ")) {
      acc.originalLines.push(line.slice(1));
      const oldFrag = acc.originalLines.length;
      acc.oldToReal[oldFrag - 1] = acc.oldLine;
      acc.oldToFragment.set(acc.oldLine, oldFrag);
      acc.oldLine += 1;

      acc.modifiedLines.push(line.slice(1));
      const newFrag = acc.modifiedLines.length;
      acc.newToReal[newFrag - 1] = acc.newLine;
      acc.newToFragment.set(acc.newLine, newFrag);
      acc.newLine += 1;
    }
    // Removed line: advances the old side only.
    else if (line.startsWith("-")) {
      acc.originalLines.push(line.slice(1));
      const oldFrag = acc.originalLines.length;
      acc.oldToReal[oldFrag - 1] = acc.oldLine;
      acc.oldToFragment.set(acc.oldLine, oldFrag);
      acc.oldLine += 1;
    }
    // Added line: advances the new side only.
    else if (line.startsWith("+")) {
      acc.modifiedLines.push(line.slice(1));
      const newFrag = acc.modifiedLines.length;
      acc.newToReal[newFrag - 1] = acc.newLine;
      acc.newToFragment.set(acc.newLine, newFrag);
      acc.newLine += 1;
    }
    // Empty line at end of diff (no prefix) -- treat as context
    else if (line === "") {
      // Could be trailing newline in the diff output; skip
    }
  }

  flushFile();
  return results;
}

/**
 * Translate a fragment (editor) line to its real file line on `side`.
 *
 * Returns `undefined` when the fragment line does not exist on that side
 * (e.g. the new side of a deleted file, or a line past the fragment's end).
 */
function fragmentToReal(
  file: FileDiff,
  side: DiffSide,
  fragmentLine: number,
): number | undefined {
  if (fragmentLine < 1) {
    return undefined;
  }
  return file.lineMap?.[side].toReal[fragmentLine - 1];
}

/**
 * Translate a real file line on `side` to its fragment (editor) line.
 *
 * Returns `undefined` when the real line is not present in the diff on that
 * side (i.e. it falls in a collapsed gap between hunks, or the side is absent).
 */
function realToFragment(
  file: FileDiff,
  side: DiffSide,
  realLine: number,
): number | undefined {
  return file.lineMap?.[side].toFragment.get(realLine);
}

/** Added / removed line counts for a raw unified diff. */
interface DiffStats {
  /** Number of added (`+`) content lines. */
  readonly additions: number;
  /** Number of removed (`-`) content lines. */
  readonly deletions: number;
}

/**
 * Count added and removed lines in a raw unified diff.
 *
 * Only content lines are counted: the `+++`/`---` file headers are excluded so
 * a single-file diff does not report a phantom +1/-1.
 */
function diffStats(raw: string): DiffStats {
  let additions = 0;
  let deletions = 0;
  for (const line of raw.split("\n")) {
    if (line.startsWith("+++") || line.startsWith("---")) {
      continue;
    }
    if (line.startsWith("+")) {
      additions += 1;
    } else if (line.startsWith("-")) {
      deletions += 1;
    }
  }
  return { additions, deletions };
}

/**
 * Build an identity {@link LineMap} for full-file content, where the fragment
 * (editor) line equals the real file line on each side.
 *
 * Used by the diff gate's full-file view (B4): when Monaco is fed the complete
 * `original`/`modified` file text there is no hunk collapsing, so fragment and
 * real lines coincide. Returning a real {@link LineMap} lets the comment/zone
 * machinery keep using {@link fragmentToReal} / {@link realToFragment} unchanged.
 */
function identityLineMap(original: string, modified: string): LineMap {
  const build = (text: string): SideLineMap => {
    const count = text === "" ? 0 : text.split("\n").length;
    const toReal: number[] = [];
    const toFragment = new Map<number, number>();
    for (let line = 1; line <= count; line += 1) {
      toReal.push(line);
      toFragment.set(line, line);
    }
    return { toReal, toFragment };
  };
  return { Old: build(original), New: build(modified) };
}

/**
 * Extract the list of changed file paths from a raw unified diff.
 */
function extractFilePaths(raw: string): readonly string[] {
  const files: string[] = [];
  for (const line of raw.split("\n")) {
    if (line.startsWith("diff --git ")) {
      const match = /^diff --git a\/.+ b\/(.+)$/.exec(line);
      if (match?.[1] !== undefined) {
        files.push(match[1]);
      }
    }
  }
  return files;
}

export {
  parseDiff,
  extractFilePaths,
  diffStats,
  fragmentToReal,
  realToFragment,
  identityLineMap,
};
export type { FileDiff, DiffStats, LineMap, SideLineMap };
