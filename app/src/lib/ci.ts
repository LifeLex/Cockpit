/**
 * Shared CI helpers: outcome classification, rollup, and badge coloring.
 *
 * These mirror the Rust `summarize` rules in `cockpit-core`'s github adapter so
 * the frontend badge (DiffView) and the CI tab (CiPanel) agree on pass/fail/
 * pending with no duplicated logic. Neutral/skipped/cancelled count as pass;
 * unknown states count as pending.
 */

import type { CiCheck } from "../bindings/CiCheck";
import type { CiSummary } from "../bindings/CiSummary";

/** Outcome of a single check. */
export type CheckOutcome = "pass" | "fail" | "pending";

/** Overall CI state derived from a summary, for badge coloring. */
export type CiState = "pass" | "fail" | "pending" | "none";

/**
 * Classify a single check outcome. Mirrors the Rust `summarize` rules:
 * neutral/skipped/cancelled count as pass; unknown states count as pending.
 *
 * CROSS-LANGUAGE MIRROR: these arms reproduce `classify_check_signal` in
 * `cockpit-core`'s github adapter one-for-one (incl. the commit-status `error`
 * -> fail). Any signal added or moved on the Rust side must be mirrored here,
 * and vice versa, or the board badge and the server rollup will disagree.
 */
export function checkOutcome(check: CiCheck): CheckOutcome {
  const signal = (check.bucket !== "" ? check.bucket : check.state).toLowerCase();
  switch (signal) {
    case "pass":
    case "skipping":
    case "cancel":
    case "success":
    case "neutral":
    case "skipped":
    case "cancelled":
    case "canceled":
      return "pass";
    case "fail":
    case "failure":
    case "timed_out":
    case "action_required":
    case "startup_failure":
    case "stale":
    case "error":
      return "fail";
    default:
      return "pending";
  }
}

/** Roll up a list of checks into pass/fail/pending counts (client mirror). */
export function summarizeChecks(checks: readonly CiCheck[]): CiSummary {
  let passed = 0;
  let failed = 0;
  let pending = 0;
  for (const c of checks) {
    const outcome = checkOutcome(c);
    if (outcome === "pass") passed += 1;
    else if (outcome === "fail") failed += 1;
    else pending += 1;
  }
  return { passed, failed, pending, total: checks.length };
}

/** Derive the overall CI state for badge coloring from a summary. */
export function ciState(summary: CiSummary): CiState {
  if (summary.total === 0) return "none";
  if (summary.failed > 0) return "fail";
  if (summary.pending > 0) return "pending";
  return "pass";
}

/**
 * Narrow an unknown Tauri event payload to the `ci-updated` tuple
 * `[pr, checks]`. Returns null when the shape does not match. Shared by the
 * DiffView badge and the CI tab so both consume the event identically.
 */
export function parseCiUpdate(
  payload: unknown,
): { readonly pr: string; readonly checks: readonly CiCheck[] } | null {
  if (!Array.isArray(payload) || payload.length !== 2) return null;
  const [pr, checks] = payload;
  if (typeof pr !== "string" || !Array.isArray(checks)) return null;
  const parsed: CiCheck[] = [];
  for (const c of checks) {
    if (
      typeof c === "object" &&
      c !== null &&
      "name" in c &&
      "state" in c &&
      "bucket" in c &&
      "link" in c &&
      "workflow" in c
    ) {
      const check: {
        name: unknown;
        state: unknown;
        bucket: unknown;
        link: unknown;
        workflow: unknown;
      } = c;
      if (
        typeof check.name === "string" &&
        typeof check.state === "string" &&
        typeof check.bucket === "string" &&
        typeof check.link === "string" &&
        typeof check.workflow === "string"
      ) {
        parsed.push({
          name: check.name,
          state: check.state,
          bucket: check.bucket,
          link: check.link,
          workflow: check.workflow,
        });
      }
    }
  }
  return { pr, checks: parsed };
}
