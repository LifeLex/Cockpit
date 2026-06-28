/**
 * Utilities for parsing unified diff text into structures the Monaco
 * DiffEditor can consume (original + modified content per file).
 *
 * A unified diff contains one or more file hunks. Each hunk has an `@@`
 * header describing the affected line ranges, followed by context (`" "`),
 * added (`"+"`), and removed (`"-"`) lines.
 */

/** A single file's diff, split into the text Monaco expects. */
interface FileDiff {
  /** Path relative to repo root. */
  readonly path: string;
  /** Content before the change (lines prefixed with `-` or ` ` in the diff). */
  readonly original: string;
  /** Content after the change (lines prefixed with `+` or ` ` in the diff). */
  readonly modified: string;
}

/**
 * Parse a raw unified diff string into per-file original/modified pairs.
 *
 * The parser handles:
 * - Multiple files in a single diff
 * - `diff --git a/... b/...` headers
 * - `---`/`+++` file headers
 * - `@@` hunk headers
 * - Context, addition, and deletion lines
 * - New files (original is empty) and deleted files (modified is empty)
 */
function parseDiff(raw: string): readonly FileDiff[] {
  if (raw.trim() === "") {
    return [];
  }

  const results: FileDiff[] = [];
  const lines = raw.split("\n");

  let currentPath: string | null = null;
  let originalLines: string[] = [];
  let modifiedLines: string[] = [];

  function flushFile(): void {
    if (currentPath !== null) {
      results.push({
        path: currentPath,
        original: originalLines.join("\n"),
        modified: modifiedLines.join("\n"),
      });
    }
    currentPath = null;
    originalLines = [];
    modifiedLines = [];
  }

  for (const line of lines) {
    // New file header: diff --git a/path b/path
    if (line.startsWith("diff --git ")) {
      flushFile();
      // Extract path from "diff --git a/foo b/foo"
      const match = /^diff --git a\/.+ b\/(.+)$/.exec(line);
      if (match?.[1] !== undefined) {
        currentPath = match[1];
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

    // Hunk header: @@ -a,b +c,d @@
    if (line.startsWith("@@")) {
      continue;
    }

    // No-newline marker
    if (line === "\\ No newline at end of file") {
      continue;
    }

    if (currentPath === null) {
      continue;
    }

    // Context line (unchanged)
    if (line.startsWith(" ")) {
      originalLines.push(line.slice(1));
      modifiedLines.push(line.slice(1));
    }
    // Removed line
    else if (line.startsWith("-")) {
      originalLines.push(line.slice(1));
    }
    // Added line
    else if (line.startsWith("+")) {
      modifiedLines.push(line.slice(1));
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

export { parseDiff, extractFilePaths };
export type { FileDiff };
