/**
 * Deterministic request→change pairing for interdiff re-review (D1).
 *
 * Maps each request the reviewer dispatched last cycle (a snapshot [`Comment`]
 * anchored to a diff line) to the interdiff region that answers it, so re-review
 * becomes confirm-per-request instead of re-read. Pure: no store access, no
 * side effects — the matching is a function of the [`DispatchSnapshot`] and the
 * parsed interdiff files only.
 *
 * A request matches when the interdiff touches the SAME file and the request's
 * line range overlaps, or falls within {@link PAIRING_TOLERANCE} lines of, any
 * of that file's interdiff hunk spans on the request's side. Hunk spans are
 * derived from {@link FileDiff.lineMap}: the `toReal` arrays list exactly the
 * real file lines the interdiff contains on each side, and maximal runs of
 * consecutive real lines are the hunks. The match target is the nearest such
 * real line, for scroll.
 */

import type { DispatchSnapshot } from "../bindings/DispatchSnapshot";
import type { Comment } from "../bindings/Comment";
import type { DiffSide } from "../bindings/DiffSide";
import type { FileDiff } from "../diff-parser";

/**
 * Real-line tolerance: a request whose range comes within this many lines of an
 * interdiff hunk still counts as addressed (the change that answers it rarely
 * lands on the exact commented line — surrounding edits shift it).
 */
const PAIRING_TOLERANCE = 10;

/** The interdiff region that answers a request, for the checklist + scroll. */
export interface PairingMatch {
  /** Path (relative to repo root) of the interdiff file that answers the request. */
  readonly path: string;
  /** Nearest real file line the interdiff touches, used as the scroll target. */
  readonly realLine: number;
}

/**
 * One dispatched request paired with the interdiff region that answers it, or
 * `null` when no interdiff change matches (a plan anchor, a wrong file, or a
 * range too far from any hunk).
 */
export interface PairingResult {
  /** The dispatched request comment. */
  readonly comment: Comment;
  /** The matching interdiff region, or `null` when nothing addresses it. */
  readonly match: PairingMatch | null;
}

/**
 * The real file lines the interdiff contains on `side`, sorted ascending and
 * deduped. Reads the side's `toReal` map (defensively skipping any non-finite
 * hole a synthesized {@link FileDiff} might carry). Empty when the side is
 * absent (e.g. the new side of a pure deletion).
 */
function presentRealLines(file: FileDiff, side: DiffSide): readonly number[] {
  const toReal = file.lineMap?.[side].toReal;
  if (toReal === undefined) return [];
  const seen = new Set<number>();
  const out: number[] = [];
  for (const line of toReal) {
    if (!Number.isFinite(line) || seen.has(line)) continue;
    seen.add(line);
    out.push(line);
  }
  out.sort((a, b) => a - b);
  return out;
}

/**
 * Maximal runs of consecutive real lines: the interdiff's hunk spans on a side.
 * `realLines` must be sorted ascending (as {@link presentRealLines} returns).
 */
function hunkSpans(
  realLines: readonly number[],
): readonly (readonly [number, number])[] {
  const spans: [number, number][] = [];
  for (const line of realLines) {
    const last = spans[spans.length - 1];
    if (last !== undefined && line <= last[1] + 1) {
      last[1] = Math.max(last[1], line);
    } else {
      spans.push([line, line]);
    }
  }
  return spans;
}

/**
 * Whether the request `range` overlaps the `span` expanded by
 * {@link PAIRING_TOLERANCE} on each side (standard interval intersection).
 */
function rangeNearSpan(
  range: readonly [number, number],
  span: readonly [number, number],
): boolean {
  const [start, end] = range;
  const [spanStart, spanEnd] = span;
  return start <= spanEnd + PAIRING_TOLERANCE && spanStart - PAIRING_TOLERANCE <= end;
}

/**
 * The interdiff real line nearest to the request `range` (0 distance when a line
 * falls inside the range). Ties break to the lower line for determinism.
 * `null` only when there are no present real lines.
 */
function nearestRealLine(
  realLines: readonly number[],
  range: readonly [number, number],
): number | null {
  const [start, end] = range;
  let best: { readonly line: number; readonly dist: number } | null = null;
  for (const line of realLines) {
    const dist = line < start ? start - line : line > end ? line - end : 0;
    if (
      best === null ||
      dist < best.dist ||
      (dist === best.dist && line < best.line)
    ) {
      best = { line, dist };
    }
  }
  return best === null ? null : best.line;
}

/** Pair one request against the interdiff, returning its match or `null`. */
function matchComment(
  comment: Comment,
  interdiffFiles: readonly FileDiff[],
): PairingMatch | null {
  const anchor = comment.anchor;
  // Only diff-line anchors can pair; plan anchors never appear on reviews but we
  // stay total and return null for them.
  if (!("DiffLine" in anchor)) return null;
  const { path, range, side } = anchor.DiffLine;

  const file = interdiffFiles.find((f) => f.path === path);
  if (file === undefined) return null;

  const realLines = presentRealLines(file, side);
  if (realLines.length === 0) return null;

  const near = hunkSpans(realLines).some((span) => rangeNearSpan(range, span));
  if (!near) return null;

  const realLine = nearestRealLine(realLines, range);
  if (realLine === null) return null;
  return { path, realLine };
}

/**
 * Pair every dispatched request against the parsed interdiff, preserving the
 * snapshot's comment order. An empty `interdiffFiles` yields all-`null` matches.
 */
export function pairRequests(
  snapshot: DispatchSnapshot,
  interdiffFiles: readonly FileDiff[],
): readonly PairingResult[] {
  return snapshot.comments.map((comment) => ({
    comment,
    match: matchComment(comment, interdiffFiles),
  }));
}
