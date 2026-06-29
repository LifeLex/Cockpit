/**
 * Diff gate UI: Monaco-based diff editor with inline comments.
 *
 * Renders the unified diff for a review using Monaco's DiffEditor,
 * with a file selector, comment list, comment input, gate state badge,
 * and request-changes button.
 */

import { useState, useMemo, useCallback, useEffect, useRef } from "react";
import { DiffEditor } from "@monaco-editor/react";
import type { Review } from "../bindings/Review";
import type { DiffData } from "../bindings/DiffData";
import type { GateState } from "../bindings/GateState";
import type { Comment } from "../bindings/Comment";
import type { CommentOrigin } from "../bindings/CommentOrigin";
import type { MirrorResult } from "../bindings/MirrorResult";
import type { Anchor } from "../bindings/Anchor";
import { parseDiff, extractFilePaths } from "../diff-parser";
import type { FileDiff } from "../diff-parser";

interface DiffViewProps {
  readonly review: Review;
  readonly diff: DiffData;
  readonly onBack: () => void;
  readonly onAddComment: (
    file: string,
    lineStart: number,
    lineEnd: number,
    body: string,
  ) => Promise<void>;
  readonly onRequestChanges: () => Promise<void>;
  readonly onMirrorComments: () => Promise<MirrorResult | null>;
}

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

function gateStateLabel(state: GateState): string {
  switch (state) {
    case "Pending":
      return "Pending";
    case "InReview":
      return "In Review";
    case "Dispatched":
      return "Dispatched";
    case "Reworked":
      return "Reworked";
    case "Approved":
      return "Approved";
    default:
      return assertNever(state);
  }
}

function gateStateBgClass(state: GateState): string {
  switch (state) {
    case "Pending":
      return "bg-state-pending";
    case "InReview":
      return "bg-state-in-review";
    case "Dispatched":
      return "bg-state-dispatched";
    case "Reworked":
      return "bg-state-reworked";
    case "Approved":
      return "bg-state-approved";
    default:
      return assertNever(state);
  }
}

/** Type guard: is this a DiffLine anchor? */
function isDiffLineAnchor(
  anchor: Anchor,
): anchor is { readonly DiffLine: { path: string; range: [number, number] } } {
  return "DiffLine" in anchor;
}

/** Extract the path from an Anchor, if it is a DiffLine anchor. */
function anchorPath(anchor: Anchor): string | null {
  if (isDiffLineAnchor(anchor)) {
    return anchor.DiffLine.path;
  }
  return null;
}

/** Extract line range from a DiffLine anchor. */
function anchorRange(
  anchor: Anchor,
): readonly [number, number] | null {
  if (isDiffLineAnchor(anchor)) {
    return anchor.DiffLine.range;
  }
  return null;
}

/** Get a FileDiff by path, falling back to empty strings for files not in the parsed diff. */
function getFileDiff(
  fileDiffs: readonly FileDiff[],
  path: string,
): FileDiff {
  const found = fileDiffs.find((fd) => fd.path === path);
  if (found !== undefined) {
    return found;
  }
  return { path, original: "", modified: "" };
}

/** File status derived from the diff content. */
type FileStatus = "added" | "modified" | "deleted";

/** Determine file status by checking original/modified content. */
function fileStatus(fileDiffs: readonly FileDiff[], path: string): FileStatus {
  const fd = fileDiffs.find((d) => d.path === path);
  if (fd === undefined) return "modified";
  if (fd.original.trim() === "") return "added";
  if (fd.modified.trim() === "") return "deleted";
  return "modified";
}

/** Count added and deleted lines for a file from the raw diff. */
function lineCounts(
  fileDiffs: readonly FileDiff[],
  path: string,
): { readonly additions: number; readonly deletions: number } {
  const fd = fileDiffs.find((d) => d.path === path);
  if (fd === undefined) return { additions: 0, deletions: 0 };
  const origLines = fd.original === "" ? 0 : fd.original.split("\n").length;
  const modLines = fd.modified === "" ? 0 : fd.modified.split("\n").length;
  // For a newly added file, all lines are additions
  if (fd.original.trim() === "") return { additions: modLines, deletions: 0 };
  // For a deleted file, all lines are deletions
  if (fd.modified.trim() === "") return { additions: 0, deletions: origLines };
  // For modified files, approximate: additions = new lines not in original count
  const additions = Math.max(0, modLines - origLines);
  const deletions = Math.max(0, origLines - modLines);
  // If same length but content differs, show at least 1 for each
  if (additions === 0 && deletions === 0 && fd.original !== fd.modified) {
    return { additions: 1, deletions: 1 };
  }
  return { additions, deletions };
}

/** Status indicator for the file tree. */
function statusIndicator(status: FileStatus): { readonly label: string; readonly className: string } {
  switch (status) {
    case "added":
      return { label: "+", className: "text-success font-bold" };
    case "modified":
      return { label: "M", className: "text-warning font-bold" };
    case "deleted":
      return { label: "-", className: "text-danger font-bold" };
    default:
      return assertNever(status);
  }
}

/** Filter comments that belong to a specific file. */
function fileComments(
  comments: readonly Comment[],
  filePath: string,
): readonly Comment[] {
  return comments.filter((c) => anchorPath(c.anchor) === filePath);
}

/** Check whether a comment origin is Local. */
function isLocalOrigin(origin: CommentOrigin): boolean {
  return origin === "Local";
}

export function DiffView({
  review,
  diff,
  onBack,
  onAddComment,
  onRequestChanges,
  onMirrorComments,
}: DiffViewProps) {
  const fileDiffs = useMemo(() => parseDiff(diff.raw), [diff.raw]);
  const filePaths = useMemo(() => extractFilePaths(diff.raw), [diff.raw]);

  const [selectedFile, setSelectedFile] = useState<string>(
    filePaths[0] ?? "",
  );

  const [commentFile, setCommentFile] = useState("");
  const [commentLineStart, setCommentLineStart] = useState(1);
  const [commentLineEnd, setCommentLineEnd] = useState(1);
  const [commentBody, setCommentBody] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [diffMode, setDiffMode] = useState<"split" | "unified">("split");

  const activeFileRef = useRef<HTMLButtonElement | null>(null);

  // Keyboard shortcut: `m` toggles the file tree sidebar
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent): void {
      // Ignore when typing in an input or textarea
      // Justified: e.target is EventTarget which doesn't expose tagName;
      // in a DOM KeyboardEvent it is always an Element or null.
      const tag = (e.target as HTMLElement | null)?.tagName ?? "";
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      if (e.key === "m") {
        setSidebarOpen((prev) => !prev);
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, []);

  // Scroll the active file into view when it changes
  useEffect(() => {
    activeFileRef.current?.scrollIntoView({ block: "nearest" });
  }, [selectedFile]);

  const currentFileDiff = useMemo(
    () => getFileDiff(fileDiffs, selectedFile),
    [fileDiffs, selectedFile],
  );

  const commentsForFile = useMemo(
    () => fileComments(review.comments, selectedFile),
    [review.comments, selectedFile],
  );

  const [mirrorResult, setMirrorResult] = useState<MirrorResult | null>(null);
  const [mirroring, setMirroring] = useState(false);

  const hasLocalComments = useMemo(
    () => review.comments.some((c) => isLocalOrigin(c.origin)),
    [review.comments],
  );

  const canRequestChanges =
    review.gate_state === "InReview" && review.comments.length > 0;

  const handleMirrorComments = useCallback(async () => {
    setMirroring(true);
    setMirrorResult(null);
    try {
      const result = await onMirrorComments();
      setMirrorResult(result);
    } finally {
      setMirroring(false);
    }
  }, [onMirrorComments]);

  const handleAddComment = useCallback(async () => {
    const file = commentFile.trim() !== "" ? commentFile : selectedFile;
    if (file === "" || commentBody.trim() === "") return;

    setSubmitting(true);
    try {
      await onAddComment(file, commentLineStart, commentLineEnd, commentBody);
      setCommentBody("");
      setCommentFile("");
      setCommentLineStart(1);
      setCommentLineEnd(1);
    } finally {
      setSubmitting(false);
    }
  }, [
    commentFile,
    selectedFile,
    commentBody,
    commentLineStart,
    commentLineEnd,
    onAddComment,
  ]);

  const handleRequestChanges = useCallback(async () => {
    setSubmitting(true);
    try {
      await onRequestChanges();
    } finally {
      setSubmitting(false);
    }
  }, [onRequestChanges]);

  return (
    <div className="flex flex-col h-screen">
      {/* Header */}
      <header className="px-6 py-3 border-b border-border flex items-center gap-4 shrink-0 bg-card">
        <button
          onClick={onBack}
          className="px-3 py-1 rounded bg-muted hover:bg-accent text-muted-foreground cursor-pointer"
        >
          Back
        </button>

        <strong>PR {review.pr}</strong>
        <span className="text-muted-foreground">{review.branch}</span>
        <span className="text-muted-foreground">Issue: {review.issue}</span>

        <span
          className={`px-2 py-0.5 rounded text-xs font-bold text-white ${gateStateBgClass(review.gate_state)}`}
        >
          {gateStateLabel(review.gate_state)}
        </span>

        {review.stale && (
          <span className="text-danger text-xs">(stale)</span>
        )}

        {review.agent != null && (
          <span className="text-warning text-xs">
            Agent running (PID: {review.agent.pid})
          </span>
        )}

        {/* Unified / Split toggle */}
        <div className="flex rounded overflow-hidden border border-border">
          <button
            onClick={() => { setDiffMode("split"); }}
            className={
              diffMode === "split"
                ? "px-3 py-1 text-xs bg-accent text-white border-none cursor-pointer"
                : "px-3 py-1 text-xs bg-muted text-muted-foreground border-none cursor-pointer hover:bg-accent"
            }
          >
            Split
          </button>
          <button
            onClick={() => { setDiffMode("unified"); }}
            className={
              diffMode === "unified"
                ? "px-3 py-1 text-xs bg-accent text-white border-none cursor-pointer"
                : "px-3 py-1 text-xs bg-muted text-muted-foreground border-none cursor-pointer hover:bg-accent"
            }
          >
            Unified
          </button>
        </div>

        <div className="ml-auto flex gap-2">
          {hasLocalComments && (
            <button
              onClick={() => void handleMirrorComments()}
              disabled={mirroring}
              className="px-3 py-1 rounded bg-muted hover:bg-accent text-white border-none cursor-pointer disabled:cursor-wait"
            >
              {mirroring ? "Mirroring..." : "Mirror to GitHub"}
            </button>
          )}
          {canRequestChanges && (
            <button
              onClick={() => void handleRequestChanges()}
              disabled={submitting}
              className="px-3 py-1 rounded bg-danger text-white border-none cursor-pointer disabled:cursor-wait"
            >
              Request Changes ({review.comments.length})
            </button>
          )}
        </div>
      </header>

      {/* Mirror result banner */}
      {mirrorResult !== null && (
        <div
          className={`px-6 py-2 border-b border-border text-[13px] ${
            mirrorResult.failed.length === 0
              ? "bg-success/20 text-success"
              : "bg-danger/20 text-danger"
          }`}
        >
          Mirrored: {mirrorResult.posted} posted
          {mirrorResult.failed.length > 0 &&
            `, ${String(mirrorResult.failed.length)} failed`}
          <button
            onClick={() => {
              setMirrorResult(null);
            }}
            className="ml-3 cursor-pointer bg-transparent border border-white/50 text-white rounded px-2 py-0.5 text-[11px]"
          >
            Dismiss
          </button>
        </div>
      )}

      {/* File tree sidebar + Monaco Diff Editor */}
      <div className="flex flex-1 min-h-0">
        {/* File tree sidebar */}
        {sidebarOpen && (
          <aside className="w-60 shrink-0 bg-card border-r border-border flex flex-col">
            <div className="px-3 py-2 border-b border-border flex items-center justify-between">
              <span className="text-xs font-semibold text-muted-foreground">
                Files ({String(filePaths.length)})
              </span>
              <button
                onClick={() => { setSidebarOpen(false); }}
                className="text-muted-foreground hover:text-muted-foreground text-xs cursor-pointer bg-transparent border-none"
                title="Hide file tree (m)"
              >
                &laquo;
              </button>
            </div>
            <nav className="flex-1 overflow-y-auto py-1">
              {filePaths.map((path) => {
                const status = fileStatus(fileDiffs, path);
                const indicator = statusIndicator(status);
                const counts = lineCounts(fileDiffs, path);
                const isActive = path === selectedFile;
                return (
                  <button
                    key={path}
                    ref={isActive ? activeFileRef : null}
                    onClick={() => { setSelectedFile(path); }}
                    className={
                      isActive
                        ? "w-full text-left px-3 py-1.5 flex items-center gap-2 text-xs bg-muted border-l-2 border-l-primary cursor-pointer border-y-0 border-r-0"
                        : "w-full text-left px-3 py-1.5 flex items-center gap-2 text-xs bg-transparent border-l-2 border-l-transparent cursor-pointer hover:bg-muted border-y-0 border-r-0"
                    }
                  >
                    <span className={`${indicator.className} w-4 text-center shrink-0`}>
                      {indicator.label}
                    </span>
                    <span className="text-foreground truncate flex-1" title={path}>
                      {path}
                    </span>
                    {(counts.additions > 0 || counts.deletions > 0) && (
                      <span className="shrink-0 text-[10px] text-muted-foreground">
                        {counts.additions > 0 && (
                          <span className="text-success">+{String(counts.additions)}</span>
                        )}
                        {counts.additions > 0 && counts.deletions > 0 && " "}
                        {counts.deletions > 0 && (
                          <span className="text-danger">-{String(counts.deletions)}</span>
                        )}
                      </span>
                    )}
                  </button>
                );
              })}
              {filePaths.length === 0 && (
                <span className="px-3 py-2 text-muted-foreground text-xs block">
                  No files in diff
                </span>
              )}
            </nav>
          </aside>
        )}

        {/* Collapsed sidebar toggle */}
        {!sidebarOpen && (
          <button
            onClick={() => { setSidebarOpen(true); }}
            className="w-6 shrink-0 bg-card border-r border-border flex items-center justify-center cursor-pointer hover:bg-muted border-y-0 border-l-0 text-muted-foreground hover:text-muted-foreground"
            title="Show file tree (m)"
          >
            &raquo;
          </button>
        )}

        {/* Monaco Diff Editor */}
        <div className="flex-1 min-h-0">
          {selectedFile !== "" ? (
            <DiffEditor
              original={currentFileDiff.original}
              modified={currentFileDiff.modified}
              language={detectLanguage(selectedFile)}
              theme="vs-dark"
              options={{
                readOnly: true,
                renderSideBySide: diffMode === "split",
                minimap: { enabled: false },
                scrollBeyondLastLine: false,
                fontSize: 13,
              }}
            />
          ) : (
            <div className="flex items-center justify-center h-full text-muted-foreground">
              No diff available
            </div>
          )}
        </div>
      </div>

      {/* Comments panel */}
      <div className="border-t border-border px-6 py-3 max-h-[300px] overflow-y-auto shrink-0">
        <h3 className="m-0 mb-2 text-sm">
          Comments
          {commentsForFile.length > 0 && (
            <span className="text-muted-foreground font-normal">
              {" "}
              ({commentsForFile.length} on {selectedFile})
            </span>
          )}
        </h3>

        {/* Existing comments for this file */}
        {commentsForFile.map((comment) => {
          const range = anchorRange(comment.anchor);
          return (
            <div
              key={comment.id}
              className="p-3 mb-2 bg-card border-l-[3px] border-l-primary rounded text-sm"
            >
              <div className="text-muted-foreground text-[11px] mb-1">
                {range !== null
                  ? `Lines ${String(range[0])}-${String(range[1])}`
                  : "File-level"}
                {" | "}
                {comment.origin}
              </div>
              <div className="whitespace-pre-wrap">{comment.body}</div>
            </div>
          );
        })}

        {/* All comments (other files) summary */}
        {review.comments.length > commentsForFile.length && (
          <div className="text-muted-foreground text-xs mb-2">
            + {review.comments.length - commentsForFile.length} comment
            {review.comments.length - commentsForFile.length !== 1
              ? "s"
              : ""}{" "}
            on other files
          </div>
        )}

        {/* Add comment form */}
        {review.gate_state === "InReview" && (
          <div className="flex gap-2 items-end flex-wrap mt-2 pt-2 border-t border-border">
            <div className="flex flex-col gap-1">
              <label className="text-[11px] text-muted-foreground">File</label>
              <input
                type="text"
                value={commentFile}
                onChange={(e) => {
                  setCommentFile(e.target.value);
                }}
                placeholder={selectedFile}
                className="w-[200px] bg-muted text-muted-foreground border border-border rounded px-2 py-1 text-xs focus:border-accent focus:outline-none"
              />
            </div>

            <div className="flex flex-col gap-1">
              <label className="text-[11px] text-muted-foreground">Lines</label>
              <div className="flex gap-1">
                <input
                  type="number"
                  min={1}
                  value={commentLineStart}
                  onChange={(e) => {
                    setCommentLineStart(Number(e.target.value));
                  }}
                  className="w-[60px] bg-muted text-muted-foreground border border-border rounded px-2 py-1 text-xs focus:border-accent focus:outline-none"
                />
                <span className="text-muted-foreground self-center">-</span>
                <input
                  type="number"
                  min={1}
                  value={commentLineEnd}
                  onChange={(e) => {
                    setCommentLineEnd(Number(e.target.value));
                  }}
                  className="w-[60px] bg-muted text-muted-foreground border border-border rounded px-2 py-1 text-xs focus:border-accent focus:outline-none"
                />
              </div>
            </div>

            <div className="flex flex-col gap-1 flex-1">
              <label className="text-[11px] text-muted-foreground">Comment</label>
              <div className="flex gap-2">
                <input
                  type="text"
                  value={commentBody}
                  onChange={(e) => {
                    setCommentBody(e.target.value);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && commentBody.trim() !== "") {
                      void handleAddComment();
                    }
                  }}
                  placeholder="Add a review comment..."
                  className="flex-1 bg-muted text-muted-foreground border border-border rounded px-2 py-1 text-xs focus:border-accent focus:outline-none"
                />
                <button
                  onClick={() => void handleAddComment()}
                  disabled={submitting || commentBody.trim() === ""}
                  className="px-3 py-1 text-xs bg-success text-white border-none rounded cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  Add
                </button>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

/** Best-effort language detection from file extension for Monaco syntax highlighting. */
function detectLanguage(filePath: string): string {
  const ext = filePath.split(".").pop()?.toLowerCase();
  if (ext === undefined) return "plaintext";

  const languageMap = {
    rs: "rust",
    ts: "typescript",
    tsx: "typescript",
    js: "javascript",
    jsx: "javascript",
    json: "json",
    toml: "toml",
    yaml: "yaml",
    yml: "yaml",
    md: "markdown",
    css: "css",
    html: "html",
    py: "python",
    sh: "shell",
    bash: "shell",
    sql: "sql",
    xml: "xml",
    svg: "xml",
  } as const satisfies Record<string, string>;

  // Type-safe lookup: check if the extension is a known key
  if (ext in languageMap) {
    // Justified: ext is validated by the `in` check above, and languageMap
    // is an immutable const object. The indexed access is safe.
    return languageMap[ext as keyof typeof languageMap];
  }
  return "plaintext";
}
