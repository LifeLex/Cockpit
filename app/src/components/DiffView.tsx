/**
 * Diff gate UI: Monaco-based diff editor with inline comments.
 *
 * Renders the unified diff for a review using Monaco's DiffEditor,
 * with a file selector, comment list, comment input, gate state badge,
 * and request-changes button.
 */

import { useState, useMemo, useCallback } from "react";
import { DiffEditor } from "@monaco-editor/react";
import type { Review } from "../bindings/Review";
import type { DiffData } from "../bindings/DiffData";
import type { GateState } from "../bindings/GateState";
import type { Comment } from "../bindings/Comment";
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

function gateStateColor(state: GateState): string {
  switch (state) {
    case "Pending":
      return "#888";
    case "InReview":
      return "#2196F3";
    case "Dispatched":
      return "#FF9800";
    case "Reworked":
      return "#9C27B0";
    case "Approved":
      return "#4CAF50";
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

/** Filter comments that belong to a specific file. */
function fileComments(
  comments: readonly Comment[],
  filePath: string,
): readonly Comment[] {
  return comments.filter((c) => anchorPath(c.anchor) === filePath);
}

export function DiffView({
  review,
  diff,
  onBack,
  onAddComment,
  onRequestChanges,
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

  const currentFileDiff = useMemo(
    () => getFileDiff(fileDiffs, selectedFile),
    [fileDiffs, selectedFile],
  );

  const commentsForFile = useMemo(
    () => fileComments(review.comments, selectedFile),
    [review.comments, selectedFile],
  );

  const canRequestChanges =
    review.gate_state === "InReview" && review.comments.length > 0;

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
    <div style={{ display: "flex", flexDirection: "column", height: "100vh" }}>
      {/* Header */}
      <header
        style={{
          padding: "12px 24px",
          borderBottom: "1px solid #333",
          display: "flex",
          alignItems: "center",
          gap: 16,
          flexShrink: 0,
        }}
      >
        <button
          onClick={onBack}
          style={{ cursor: "pointer", padding: "4px 12px" }}
        >
          Back
        </button>

        <strong>PR {review.pr}</strong>
        <span style={{ color: "#888" }}>{review.branch}</span>
        <span style={{ color: "#888" }}>Issue: {review.issue}</span>

        <span
          style={{
            padding: "2px 8px",
            borderRadius: 4,
            backgroundColor: gateStateColor(review.gate_state),
            color: "white",
            fontSize: 12,
          }}
        >
          {gateStateLabel(review.gate_state)}
        </span>

        {review.stale && (
          <span style={{ color: "#FF5722", fontSize: 12 }}>(stale)</span>
        )}

        {review.agent != null && (
          <span style={{ color: "#FF9800", fontSize: 12 }}>
            Agent running (PID: {review.agent.pid})
          </span>
        )}

        <div style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
          {canRequestChanges && (
            <button
              onClick={() => void handleRequestChanges()}
              disabled={submitting}
              style={{
                cursor: submitting ? "wait" : "pointer",
                padding: "4px 12px",
                backgroundColor: "#FF5722",
                color: "white",
                border: "none",
                borderRadius: 4,
              }}
            >
              Request Changes ({review.comments.length})
            </button>
          )}
        </div>
      </header>

      {/* File selector */}
      <nav
        style={{
          padding: "8px 24px",
          borderBottom: "1px solid #333",
          display: "flex",
          gap: 4,
          overflowX: "auto",
          flexShrink: 0,
        }}
      >
        {filePaths.map((path) => (
          <button
            key={path}
            onClick={() => {
              setSelectedFile(path);
            }}
            style={{
              cursor: "pointer",
              padding: "4px 8px",
              fontSize: 12,
              backgroundColor:
                path === selectedFile ? "#2196F3" : "transparent",
              color: path === selectedFile ? "white" : "#ccc",
              border: "1px solid #555",
              borderRadius: 4,
              whiteSpace: "nowrap",
            }}
          >
            {path}
          </button>
        ))}
        {filePaths.length === 0 && (
          <span style={{ color: "#888", fontSize: 12 }}>
            No files in diff
          </span>
        )}
      </nav>

      {/* Monaco Diff Editor */}
      <div style={{ flex: 1, minHeight: 0 }}>
        {selectedFile !== "" ? (
          <DiffEditor
            original={currentFileDiff.original}
            modified={currentFileDiff.modified}
            language={detectLanguage(selectedFile)}
            theme="vs-dark"
            options={{
              readOnly: true,
              renderSideBySide: true,
              minimap: { enabled: false },
              scrollBeyondLastLine: false,
              fontSize: 13,
            }}
          />
        ) : (
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              height: "100%",
              color: "#888",
            }}
          >
            No diff available
          </div>
        )}
      </div>

      {/* Comments panel */}
      <div
        style={{
          borderTop: "1px solid #333",
          padding: "12px 24px",
          maxHeight: 300,
          overflowY: "auto",
          flexShrink: 0,
        }}
      >
        <h3 style={{ margin: "0 0 8px 0", fontSize: 14 }}>
          Comments
          {commentsForFile.length > 0 && (
            <span style={{ color: "#888", fontWeight: "normal" }}>
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
              style={{
                padding: "8px 12px",
                marginBottom: 8,
                backgroundColor: "#1e1e1e",
                borderLeft: "3px solid #2196F3",
                borderRadius: 4,
                fontSize: 13,
              }}
            >
              <div style={{ color: "#888", fontSize: 11, marginBottom: 4 }}>
                {range !== null
                  ? `Lines ${String(range[0])}-${String(range[1])}`
                  : "File-level"}
                {" | "}
                {comment.origin}
              </div>
              <div style={{ whiteSpace: "pre-wrap" }}>{comment.body}</div>
            </div>
          );
        })}

        {/* All comments (other files) summary */}
        {review.comments.length > commentsForFile.length && (
          <div
            style={{
              color: "#888",
              fontSize: 12,
              marginBottom: 8,
            }}
          >
            + {review.comments.length - commentsForFile.length} comment
            {review.comments.length - commentsForFile.length !== 1
              ? "s"
              : ""}{" "}
            on other files
          </div>
        )}

        {/* Add comment form */}
        {review.gate_state === "InReview" && (
          <div
            style={{
              display: "flex",
              gap: 8,
              alignItems: "flex-end",
              flexWrap: "wrap",
              marginTop: 8,
              paddingTop: 8,
              borderTop: "1px solid #333",
            }}
          >
            <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
              <label style={{ fontSize: 11, color: "#888" }}>File</label>
              <input
                type="text"
                value={commentFile}
                onChange={(e) => {
                  setCommentFile(e.target.value);
                }}
                placeholder={selectedFile}
                style={{
                  width: 200,
                  padding: "4px 8px",
                  fontSize: 12,
                  backgroundColor: "#1e1e1e",
                  color: "#ccc",
                  border: "1px solid #555",
                  borderRadius: 4,
                }}
              />
            </div>

            <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
              <label style={{ fontSize: 11, color: "#888" }}>Lines</label>
              <div style={{ display: "flex", gap: 4 }}>
                <input
                  type="number"
                  min={1}
                  value={commentLineStart}
                  onChange={(e) => {
                    setCommentLineStart(Number(e.target.value));
                  }}
                  style={{
                    width: 60,
                    padding: "4px 8px",
                    fontSize: 12,
                    backgroundColor: "#1e1e1e",
                    color: "#ccc",
                    border: "1px solid #555",
                    borderRadius: 4,
                  }}
                />
                <span style={{ color: "#888", alignSelf: "center" }}>-</span>
                <input
                  type="number"
                  min={1}
                  value={commentLineEnd}
                  onChange={(e) => {
                    setCommentLineEnd(Number(e.target.value));
                  }}
                  style={{
                    width: 60,
                    padding: "4px 8px",
                    fontSize: 12,
                    backgroundColor: "#1e1e1e",
                    color: "#ccc",
                    border: "1px solid #555",
                    borderRadius: 4,
                  }}
                />
              </div>
            </div>

            <div
              style={{
                display: "flex",
                flexDirection: "column",
                gap: 4,
                flex: 1,
              }}
            >
              <label style={{ fontSize: 11, color: "#888" }}>Comment</label>
              <div style={{ display: "flex", gap: 8 }}>
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
                  style={{
                    flex: 1,
                    padding: "4px 8px",
                    fontSize: 12,
                    backgroundColor: "#1e1e1e",
                    color: "#ccc",
                    border: "1px solid #555",
                    borderRadius: 4,
                  }}
                />
                <button
                  onClick={() => void handleAddComment()}
                  disabled={submitting || commentBody.trim() === ""}
                  style={{
                    cursor:
                      submitting || commentBody.trim() === ""
                        ? "not-allowed"
                        : "pointer",
                    padding: "4px 12px",
                    fontSize: 12,
                    backgroundColor: "#4CAF50",
                    color: "white",
                    border: "none",
                    borderRadius: 4,
                    opacity:
                      submitting || commentBody.trim() === "" ? 0.5 : 1,
                  }}
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
