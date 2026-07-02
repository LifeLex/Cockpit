/**
 * Attention-first ordering and triage signals for the PRs board (C1/C2).
 *
 * The board leads with what needs the reviewer. `attentionRank` produces a
 * numeric key where reviews awaiting a human decision rise and settled or
 * blocked-by-ancestor work sinks. The rank is *layered*: the gate state is the
 * primary bucket, and small documented within-bucket adjustments (CI status,
 * diff size, path sensitivity) reorder reviews *inside* a bucket without ever
 * crossing a bucket boundary — a Reworked review always outranks an Approved one
 * regardless of CI or size. `attentionReasons` exposes the human-readable WHY
 * behind those adjustments for the cards, and `isFastLane` picks out the
 * "small + green + low-risk" reviews for the fast-lane shelf.
 */

import type { Review } from "../bindings/Review";
import type { GateState } from "../bindings/GateState";
import type { SizeClass } from "../bindings/SizeClass";
import type { RiskFlag } from "../bindings/RiskFlag";
import type { CiState } from "./ci";
import { ciState } from "./ci";
import { diffTotals, sizeClass, sensitiveFlags } from "./diff-signals";

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/**
 * Base rank for a gate state. Lower sorts first (more attention). Reworked and
 * InReview are the reviewer's active queue; Pending is next; Dispatched is
 * agent-owned and waiting; Approved is done and sinks.
 */
function gateStateRank(state: GateState): number {
  switch (state) {
    case "Reworked":
      return 0;
    case "InReview":
      return 1;
    case "Pending":
      return 2;
    case "Dispatched":
      return 3;
    case "Approved":
      return 4;
    // Merged is fully settled — the lowest urgency, below Approved.
    case "Merged":
      return 5;
    default:
      return assertNever(state);
  }
}

/**
 * Within-bucket attention adjustments (C1).
 *
 * These are *secondary* terms added on top of the integer gate-state bucket
 * rank. Their combined span is deliberately kept below 1.0 — the spacing
 * between two adjacent gate-state buckets — so they can reorder reviews *within*
 * a bucket but can NEVER move one across a bucket boundary. The most a review
 * can rise is `CI_PASS + SIZE_SMALL = -0.20`; the most it can sink is
 * `CI_FAIL + SIZE_XL + SENSITIVE = +0.50`; the total span (0.70) is < 1.0, so
 * the needs-human ordering is invariant.
 *
 * Sign convention: negative = rises (more attention), positive = sinks. The
 * roadmap's routing rule (C1/C2): small + green surfaces for the fast lane;
 * red / XL / risky sinks toward deep review. A CI-failing PR still *needs*
 * attention (for a fix-CI decision), but it is a deep-review decision, not a
 * 2-minute one, so it sinks within its bucket rather than leading it.
 */
const CI_PASS = -0.1; // all checks green — a decision you can make in 2 minutes.
const CI_PENDING = 0.05; // not yet decidable — mildly deprioritized.
const CI_FAIL = 0.15; // needs a deeper fix-CI decision — sinks.
const SIZE_SMALL = -0.1; // an S diff reviews fast — rises.
const SIZE_LARGE = 0.1; // an L diff needs a careful read — sinks.
const SIZE_XL = 0.2; // an XL diff sinks hardest.
const SENSITIVE = 0.15; // auth/migrations/config/etc. warrant scrutiny — sinks.

/** The rolled-up CI state for a review, or `"none"` when no checks are loaded. */
function reviewCiState(review: Review): CiState {
  const ci = review.ci_summary;
  return ci === undefined ? "none" : ciState(ci);
}

/** Within-bucket adjustment from CI status. */
function ciAdjustment(state: CiState): number {
  switch (state) {
    case "pass":
      return CI_PASS;
    case "pending":
      return CI_PENDING;
    case "fail":
      return CI_FAIL;
    case "none":
      return 0;
    default:
      return assertNever(state);
  }
}

/** Within-bucket adjustment from diff size class. */
function sizeAdjustment(size: SizeClass): number {
  switch (size) {
    case "S":
      return SIZE_SMALL;
    case "M":
      return 0;
    case "L":
      return SIZE_LARGE;
    case "Xl":
      return SIZE_XL;
    default:
      return assertNever(size);
  }
}

/**
 * The memoized triage signals derived for a single review: its rank plus the
 * diff-scan results the cards and the fast lane read. Bundled so a review's
 * (potentially large) diff is parsed at most once, no matter how many of
 * `attentionRank` / `attentionReasons` / `isFastLane` / the stack-tree sort keys
 * touch it within a render pass.
 */
interface ReviewSignals {
  /** Full attention rank (lower sorts first). */
  readonly rank: number;
  /** Rolled-up CI state (`"none"` when no checks are loaded). */
  readonly ci: CiState;
  /** Diff size class, from the add/del totals. */
  readonly size: SizeClass;
  /** Total changed lines (additions + deletions), for the fast-lane cutoff. */
  readonly totalLines: number;
  /** Sensitive-path risk flags, at most one per touched file, in diff order. */
  readonly sensitiveFlags: readonly RiskFlag[];
  /** Whether the diff touches any sensitive path. */
  readonly hasSensitive: boolean;
}

/**
 * Per-review signal cache, keyed on object identity.
 *
 * A `WeakMap<Review, …>` is safe against staleness precisely because it is keyed
 * on identity: every fetch/refresh replaces a review with a *fresh* object, so a
 * mutated review can never hit a stale entry (the old object is simply gone and
 * gets garbage-collected with its cache slot). The cache lives only to spare
 * repeated diff scans of the *same* object within a single render pass.
 */
const signalsCache = new WeakMap<Review, ReviewSignals>();

/** Compute (or return the cached) triage signals for a review. */
function signalsFor(review: Review): ReviewSignals {
  const cached = signalsCache.get(review);
  if (cached !== undefined) {
    return cached;
  }

  const { additions, deletions } = diffTotals(review.diff.raw);
  const size = sizeClass(additions, deletions);
  const flags = sensitiveFlags(review.diff.raw);
  const hasSensitive = flags.length > 0;
  const ci = reviewCiState(review);

  const stalePenalty = review.stale ? 100 : 0;
  const within =
    ciAdjustment(ci) + sizeAdjustment(size) + (hasSensitive ? SENSITIVE : 0);
  const rank = stalePenalty + gateStateRank(review.gate_state) + within;

  const signals: ReviewSignals = {
    rank,
    ci,
    size,
    totalLines: additions + deletions,
    sensitiveFlags: flags,
    hasSensitive,
  };
  signalsCache.set(review, signals);
  return signals;
}

/**
 * Full attention rank for a review. Lower sorts first. A stale review is
 * deprioritized past every non-stale one (it is blocked on an ancestor's
 * rework) via a large `+100` penalty, while the gate-state bucket and the small
 * within-bucket signal adjustments order the rest.
 */
export function attentionRank(review: Review): number {
  return signalsFor(review).rank;
}

/** Human-readable phrase for a sensitive-path risk flag. */
function riskReason(flag: RiskFlag): string {
  switch (flag) {
    case "Migration":
      return "Touches migrations";
    case "Lockfile":
      return "Touches lockfile";
    case "CiConfig":
      return "Touches CI config";
    case "Auth":
      return "Touches auth";
    case "GithubDir":
      return "Touches .github";
    case "Dependency":
      return "Touches dependencies";
    default:
      return assertNever(flag);
  }
}

/**
 * The human-readable reasons a review carries extra risk, in priority order:
 * CI failing first, then a large-diff note, then one entry per distinct
 * sensitive path touched. These mirror the within-bucket rank adjustments and
 * are what the cards surface as the "why". Returns `[]` for a clean, small,
 * green review (its gate reason stands alone).
 */
export function attentionReasons(review: Review): string[] {
  const { ci, size, sensitiveFlags: flags } = signalsFor(review);
  const reasons: string[] = [];

  if (ci === "fail") {
    reasons.push("CI failing");
  }

  if (size === "Xl") {
    reasons.push("Very large diff");
  } else if (size === "L") {
    reasons.push("Large diff");
  }

  // One reason per distinct flag, in first-seen order (dedupe repeats).
  const seen = new Set<RiskFlag>();
  for (const flag of flags) {
    if (!seen.has(flag)) {
      seen.add(flag);
      reasons.push(riskReason(flag));
    }
  }

  return reasons;
}

/** Whether a review is awaiting a human decision (not blocked, not settled). */
function isActionable(review: Review): boolean {
  if (review.stale) {
    return false;
  }
  return (
    review.gate_state === "Pending" ||
    review.gate_state === "InReview" ||
    review.gate_state === "Reworked"
  );
}

/**
 * The upper bound (exclusive) on changed lines for the fast lane. `< 200` covers
 * the whole `S` bucket (< 50) *and* the whole `M` bucket (`M` is [50, 200)),
 * matching the roadmap's "small" definition; `L`/`Xl` (>= 200) never qualify.
 */
const FAST_LANE_MAX_LINES = 200;

/**
 * Whether a review belongs in the fast-lane shelf (C2): an actionable,
 * "small + green + low-risk" review whose decision should take ~2 minutes.
 *
 * A review qualifies when it is actionable (Pending/InReview/Reworked and not
 * stale), it is a stack ROOT (no parents), its diff is small (<
 * {@link FAST_LANE_MAX_LINES} changed lines), CI is present and fully green, and
 * it touches no sensitive paths. The test-weakening signal is deliberately *not*
 * consulted: it is computed server-side and is not part of the card-subset TS
 * mirror, so the fast lane is defined on size + CI + paths only (a weakening flag
 * still surfaces later at the diff gate).
 *
 * The stack-root requirement keeps stacks intact: surfacing a child in the fast
 * lane ahead of its unreviewed parent violates frontier-root-first ordering and
 * leaves a visual hole in the stack below it, so a parented review stays in its
 * stack group even when it is small + green + low-risk.
 *
 * The fast lane compresses the *decision*, never the *authority*: it is not
 * auto-approve and grants no new terminal action (see roadmap C2 / CLAUDE.md
 * §9).
 */
export function isFastLane(review: Review): boolean {
  if (!isActionable(review)) {
    return false;
  }
  // Only a stack root (no parents) may enter the fast lane — see the note above.
  if (review.parents.length > 0) {
    return false;
  }
  const { totalLines, ci, hasSensitive } = signalsFor(review);
  if (totalLines >= FAST_LANE_MAX_LINES) {
    return false;
  }
  // CI must be loaded and fully green; absent CI ("none") does not qualify.
  if (ci !== "pass") {
    return false;
  }
  if (hasSensitive) {
    return false;
  }
  return true;
}

/**
 * Return a new array of reviews sorted attention-first. Stable within equal
 * ranks (uses `id` as the tiebreaker) so ordering is deterministic. Ranks are
 * computed once per review (not per comparison) so a large diff is parsed only
 * once. Does not mutate the input.
 */
export function sortByAttention(reviews: readonly Review[]): readonly Review[] {
  const ranked = reviews.map((review) => ({
    review,
    rank: attentionRank(review),
  }));
  ranked.sort((a, b) => {
    const byRank = a.rank - b.rank;
    if (byRank !== 0) return byRank;
    return a.review.id < b.review.id ? -1 : a.review.id > b.review.id ? 1 : 0;
  });
  return ranked.map((r) => r.review);
}
