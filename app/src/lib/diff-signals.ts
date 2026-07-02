/**
 * Card-subset TypeScript mirror of the Rust deterministic diff signals.
 *
 * These pure functions operate on a raw unified diff string and reproduce a
 * *subset* of `crates/cockpit-core/src/diff_signals.rs` — the parts a PR card
 * needs to render at a glance (size class, add/del totals, sensitive-path and
 * test-touch flags). The full signal set (weakening flags, per-flag jump
 * targets, the evidence bundle) is computed server-side and delivered via
 * [`EvidenceSummary`]; this mirror exists only so the board can classify a diff
 * without a round-trip.
 *
 * The classification heuristics here are COPIED from the documented Rust rules
 * (including the `autho`-stem exclusion for the auth flag) so the two sides
 * agree. The fixtures in `diff-signals.test.ts` are the same ones used by the
 * Rust unit tests, guarding against drift between the two implementations.
 */

import type { RiskFlag } from "../bindings/RiskFlag";
import type { SizeClass } from "../bindings/SizeClass";
import { diffStats, extractFilePaths } from "../diff-parser";

/**
 * Bucket a diff by its total changed lines (additions + deletions):
 * `S` < 50, `M` < 200, `L` < 600, else `Xl`. Mirrors `classify_size`.
 */
export function sizeClass(additions: number, deletions: number): SizeClass {
  const total = additions + deletions;
  if (total < 50) return "S";
  if (total < 200) return "M";
  if (total < 600) return "L";
  return "Xl";
}

/** Added/removed line totals plus the file count for a raw unified diff. */
export interface DiffTotals {
  /** Number of added (`+`) content lines. */
  readonly additions: number;
  /** Number of removed (`-`) content lines. */
  readonly deletions: number;
  /** Number of files the diff touches (one per `diff --git` header). */
  readonly filesChanged: number;
}

/**
 * Add/remove/file totals for a raw unified diff.
 *
 * Reuses {@link diffStats} for the line counts (which already excludes the
 * `+++`/`---` file headers) and {@link extractFilePaths} for the file count.
 */
export function diffTotals(raw: string): DiffTotals {
  const { additions, deletions } = diffStats(raw);
  return {
    additions,
    deletions,
    filesChanged: extractFilePaths(raw).length,
  };
}

/** The final path component (basename), or the whole path if it has no `/`. */
function fileName(path: string): string {
  const parts = path.split("/");
  return parts[parts.length - 1] ?? path;
}

/** Lockfile basenames, matched case-sensitively. Mirrors `LOCKFILES`. */
const LOCKFILES: readonly string[] = [
  "Cargo.lock",
  "package-lock.json",
  "pnpm-lock.yaml",
  "yarn.lock",
  "uv.lock",
  "poetry.lock",
];

/**
 * Whether a path names auth/secret material. Mirrors `has_auth_indicator`.
 *
 * `credential`, `secret`, and `token` match as case-insensitive substrings.
 * `auth` is matched unless immediately followed by `o`, which filters the
 * `autho…` stem (`author`, `authored`) while keeping `auth.rs`, `auth_token`,
 * etc. As documented on the Rust side, `authorize`/`authorization` share that
 * stem and so do not flag.
 */
function hasAuthIndicator(path: string): boolean {
  const lower = path.toLowerCase();
  if (
    lower.includes("credential") ||
    lower.includes("secret") ||
    lower.includes("token")
  ) {
    return true;
  }
  let idx = lower.indexOf("auth");
  while (idx !== -1) {
    // A missing char (auth at end of string) is `undefined !== "o"` -> flags,
    // matching the Rust `get(idx + 4) != Some(b'o')`.
    if (lower[idx + 4] !== "o") return true;
    idx = lower.indexOf("auth", idx + 1);
  }
  return false;
}

/**
 * The single risk flag for a path, in priority order. Mirrors `classify_risk`:
 * `Migration`/`Lockfile` outrank `CiConfig`, then `Auth`, then the catch-all
 * `GithubDir`, then `Dependency`. Returns `null` for an unremarkable path.
 */
function classifyRisk(path: string): RiskFlag | null {
  const lower = path.toLowerCase();
  const name = fileName(path);

  if (lower.includes("migration") || lower.endsWith(".sql")) {
    return "Migration";
  }
  if (LOCKFILES.includes(name)) {
    return "Lockfile";
  }
  if (
    lower.includes(".github/workflows/") ||
    name === "ci.yml" ||
    lower.includes(".gitlab-ci") ||
    name === "Jenkinsfile"
  ) {
    return "CiConfig";
  }
  if (hasAuthIndicator(path)) {
    return "Auth";
  }
  if (lower.includes(".github/")) {
    return "GithubDir";
  }
  if (name === "Cargo.toml" || name === "package.json") {
    return "Dependency";
  }
  return null;
}

/** A path is a test file when it contains `test` or a `.spec.` segment. */
function isTestFile(path: string): boolean {
  const lower = path.toLowerCase();
  return lower.includes("test") || lower.includes(".spec.");
}

/**
 * The risk flags a diff carries, at most one per touched file (in the diff's
 * file order). Mirrors the per-file `classify_risk` pass; duplicates across
 * files are preserved, matching the Rust `Vec<RiskPath>`.
 */
export function sensitiveFlags(raw: string): RiskFlag[] {
  const flags: RiskFlag[] = [];
  for (const path of extractFilePaths(raw)) {
    const flag = classifyRisk(path);
    if (flag !== null) flags.push(flag);
  }
  return flags;
}

/** Whether the diff touches any file flagged as risky. */
export function hasSensitivePath(raw: string): boolean {
  return sensitiveFlags(raw).length > 0;
}

/** Whether the diff touches any test file. */
export function touchesTests(raw: string): boolean {
  return extractFilePaths(raw).some(isTestFile);
}
