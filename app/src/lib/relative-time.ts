/**
 * Compact, human-readable relative-time formatting for card telemetry.
 *
 * The domain carries wall-clock instants as `{ secs_since_epoch }` tuples
 * (Rust `SystemTime` on the wire). These helpers turn such an instant into a
 * short elapsed string like `3m` / `2h` / `4d` suitable for a dense readout.
 */

/** A serialized `SystemTime` as it crosses the IPC boundary. */
export interface SystemTimeLike {
  readonly secs_since_epoch: number;
}

/**
 * Format the elapsed time since `since` as a compact string (`now`, `45s`,
 * `12m`, `3h`, `5d`). `now` is injected for deterministic tests and defaults to
 * the wall clock. Future timestamps clamp to `now`.
 */
export function elapsedSince(
  since: SystemTimeLike,
  now: number = Date.now(),
): string {
  const seconds = Math.max(0, Math.floor(now / 1000 - since.secs_since_epoch));
  if (seconds < 5) return "now";
  if (seconds < 60) return `${String(seconds)}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${String(minutes)}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${String(hours)}h`;
  const days = Math.floor(hours / 24);
  return `${String(days)}d`;
}
