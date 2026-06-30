/**
 * Diff gate UI: Monaco-based diff editor with GitHub-style inline comments.
 *
 * Click a line number in the modified editor gutter to open an inline
 * comment form. Existing comments render as view zones at their anchored
 * lines, pushing content down exactly like GitHub PR reviews.
 */

import {
  useState,
  useMemo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
} from "react";
import { createPortal } from "react-dom";
import {
  DiffEditor,
  type DiffOnMount,
  type Monaco,
  type MonacoDiffEditor,
} from "@monaco-editor/react";
import type { editor as MonacoEditorNs } from "monaco-editor";
import type { Review } from "../bindings/Review";
import type { DiffData } from "../bindings/DiffData";
import type { GateState } from "../bindings/GateState";
import type { Comment } from "../bindings/Comment";
import type { CommentOrigin } from "../bindings/CommentOrigin";
import type { MirrorResult } from "../bindings/MirrorResult";
import type { Anchor } from "../bindings/Anchor";
import { parseDiff, extractFilePaths } from "../diff-parser";
import type { FileDiff } from "../diff-parser";
import { useAppStore } from "../store";
import { registerCustomThemes } from "@/lib/monaco-themes";
import { AgentPanel } from "./AgentPanel";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import { invoke } from "@tauri-apps/api/core";
import {
  ArrowLeft,
  ExternalLink,
  MessageSquare,
  Upload,
  Send,
  AlertTriangle,
  Bot,
} from "lucide-react";

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

interface PortalEntry {
  readonly key: string;
  readonly domNode: HTMLDivElement;
  readonly lineNumber: number;
  readonly comments: readonly Comment[];
  readonly hasInput: boolean;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

function gateStateBadgeClass(state: GateState): string {
  switch (state) {
    case "Pending":
      return "bg-state-pending/20 text-state-pending border-state-pending/30";
    case "InReview":
      return "bg-state-in-review/20 text-state-in-review border-state-in-review/30";
    case "Dispatched":
      return "bg-state-dispatched/20 text-state-dispatched border-state-dispatched/30";
    case "Reworked":
      return "bg-state-reworked/20 text-state-reworked border-state-reworked/30";
    case "Approved":
      return "bg-state-approved/20 text-state-approved border-state-approved/30";
    default:
      return assertNever(state);
  }
}

function isDiffLineAnchor(
  anchor: Anchor,
): anchor is { readonly DiffLine: { path: string; range: [number, number] } } {
  return "DiffLine" in anchor;
}

function anchorPath(anchor: Anchor): string | null {
  if (isDiffLineAnchor(anchor)) {
    return anchor.DiffLine.path;
  }
  return null;
}

function anchorRange(
  anchor: Anchor,
): readonly [number, number] | null {
  if (isDiffLineAnchor(anchor)) {
    return anchor.DiffLine.range;
  }
  return null;
}

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

type FileStatus = "added" | "modified" | "deleted";

function fileStatus(fileDiffs: readonly FileDiff[], path: string): FileStatus {
  const fd = fileDiffs.find((d) => d.path === path);
  if (fd === undefined) return "modified";
  if (fd.original.trim() === "") return "added";
  if (fd.modified.trim() === "") return "deleted";
  return "modified";
}

function lineCounts(
  fileDiffs: readonly FileDiff[],
  path: string,
): { readonly additions: number; readonly deletions: number } {
  const fd = fileDiffs.find((d) => d.path === path);
  if (fd === undefined) return { additions: 0, deletions: 0 };
  const origLines = fd.original === "" ? 0 : fd.original.split("\n").length;
  const modLines = fd.modified === "" ? 0 : fd.modified.split("\n").length;
  if (fd.original.trim() === "") return { additions: modLines, deletions: 0 };
  if (fd.modified.trim() === "") return { additions: 0, deletions: origLines };
  const additions = Math.max(0, modLines - origLines);
  const deletions = Math.max(0, origLines - modLines);
  if (additions === 0 && deletions === 0 && fd.original !== fd.modified) {
    return { additions: 1, deletions: 1 };
  }
  return { additions, deletions };
}

function statusIndicator(
  status: FileStatus,
): { readonly label: string; readonly className: string } {
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

function fileComments(
  comments: readonly Comment[],
  filePath: string,
): readonly Comment[] {
  return comments.filter((c) => anchorPath(c.anchor) === filePath);
}

function isLocalOrigin(origin: CommentOrigin): boolean {
  return origin === "Local";
}

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

  if (ext in languageMap) {
    // Justified: ext is validated by the `in` check above
    return languageMap[ext as keyof typeof languageMap];
  }
  return "plaintext";
}

// ---------------------------------------------------------------------------
// InlineCommentThread -- rendered inside Monaco view zones via portals
// ---------------------------------------------------------------------------

function InlineCommentThread({
  comments,
  lineNumber,
  hasInput,
  onSubmit,
  onCancel,
}: {
  readonly comments: readonly Comment[];
  readonly lineNumber: number;
  readonly hasInput: boolean;
  readonly onSubmit: (body: string) => void;
  readonly onCancel: () => void;
}) {
  const [body, setBody] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (hasInput) {
      // Defer focus to avoid conflict with Monaco's click handling
      const id = requestAnimationFrame(() => {
        textareaRef.current?.focus();
      });
      return () => {
        cancelAnimationFrame(id);
      };
    }
  }, [hasInput]);

  const handleSubmit = useCallback(() => {
    const trimmed = body.trim();
    if (trimmed !== "") {
      onSubmit(trimmed);
      setBody("");
    }
  }, [body, onSubmit]);

  return (
    <div className="mx-1 my-0.5 rounded-md border border-border bg-card shadow-sm overflow-hidden text-sm">
      {/* Existing comments */}
      {comments.map((comment) => {
        const range = anchorRange(comment.anchor);
        return (
          <div
            key={comment.id}
            className="px-3 py-2 border-b border-border last:border-b-0"
          >
            <div className="flex items-center gap-2 mb-1">
              <Badge
                variant="outline"
                className="text-[10px] px-1.5 py-0 h-4"
              >
                {String(comment.origin)}
              </Badge>
              {range !== null && (
                <span className="text-[10px] text-muted-foreground">
                  L{String(range[0])}
                  {range[0] !== range[1] ? `–${String(range[1])}` : ""}
                </span>
              )}
            </div>
            <div className="whitespace-pre-wrap text-foreground text-xs leading-relaxed">
              {comment.body}
            </div>
          </div>
        );
      })}

      {/* Inline comment input */}
      {hasInput && (
        <div className="p-2 bg-muted/30">
          <textarea
            ref={textareaRef}
            value={body}
            onChange={(e) => {
              setBody(e.target.value);
            }}
            onKeyDown={(e) => {
              if (
                e.key === "Enter" &&
                (e.metaKey || e.ctrlKey) &&
                body.trim() !== ""
              ) {
                e.preventDefault();
                handleSubmit();
              }
              if (e.key === "Escape") {
                e.preventDefault();
                onCancel();
              }
            }}
            placeholder="Write a comment... (Cmd+Enter to submit, Esc to cancel)"
            className="w-full bg-background text-foreground border border-border rounded-md p-2 text-xs resize-none focus:outline-none focus:ring-1 focus:ring-ring min-h-[56px]"
            rows={3}
          />
          <div className="flex items-center justify-between mt-1.5">
            <span className="text-[10px] text-muted-foreground">
              Line {String(lineNumber)}
            </span>
            <div className="flex gap-1.5">
              <Button
                variant="ghost"
                size="sm"
                className="h-6 text-xs px-2"
                onClick={onCancel}
              >
                Cancel
              </Button>
              <Button
                size="sm"
                className="h-6 text-xs px-2"
                onClick={handleSubmit}
                disabled={body.trim() === ""}
              >
                <MessageSquare className="h-3 w-3" />
                Comment
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// DiffView
// ---------------------------------------------------------------------------

export function DiffView({
  review,
  diff,
  onBack,
  onAddComment,
  onRequestChanges,
  onMirrorComments,
}: DiffViewProps) {
  // -- Agent C: editor theme from store --
  const editorTheme = useAppStore((s) => s.editorTheme);

  // -- Diff parsing --
  const fileDiffs = useMemo(() => parseDiff(diff.raw), [diff.raw]);
  const filePaths = useMemo(() => extractFilePaths(diff.raw), [diff.raw]);

  // -- Navigation / display state --
  const [selectedFile, setSelectedFile] = useState<string>(
    filePaths[0] ?? "",
  );
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [diffMode, setDiffMode] = useState<"split" | "unified">("split");
  const [submitting, setSubmitting] = useState(false);
  const [agentPanelVisible, setAgentPanelVisible] = useState(
    review.gate_state === "Dispatched",
  );

  // -- Inline comment state --
  const [activeCommentLine, setActiveCommentLine] = useState<number | null>(
    null,
  );
  const [editorReady, setEditorReady] = useState(false);
  const [portals, setPortals] = useState<readonly PortalEntry[]>([]);

  // -- Error state for inline operations --
  const [commentError, setCommentError] = useState<string | null>(null);

  // -- Mirror state --
  const [mirrorResult, setMirrorResult] = useState<MirrorResult | null>(null);
  const [mirroring, setMirroring] = useState(false);

  // -- Refs --
  const activeFileRef = useRef<HTMLButtonElement | null>(null);
  const diffEditorRef = useRef<MonacoDiffEditor | null>(null);
  const monacoRef = useRef<Monaco | null>(null);
  const zoneIdsRef = useRef<string[]>([]);
  const domNodeCacheRef = useRef<Map<string, HTMLDivElement>>(new Map());
  const glyphDecorRef =
    useRef<MonacoEditorNs.IEditorDecorationsCollection | null>(null);
  const commentDecorRef =
    useRef<MonacoEditorNs.IEditorDecorationsCollection | null>(null);

  // -- Derived --
  const currentFileDiff = useMemo(
    () => getFileDiff(fileDiffs, selectedFile),
    [fileDiffs, selectedFile],
  );

  const commentsForFile = useMemo(
    () => fileComments(review.comments, selectedFile),
    [review.comments, selectedFile],
  );

  const hasLocalComments = useMemo(
    () => review.comments.some((c) => isLocalOrigin(c.origin)),
    [review.comments],
  );

  const canRequestChanges =
    review.gate_state === "InReview" && review.comments.length > 0;

  const canAddComments = review.gate_state === "InReview";

  // Only open the input form when the review is InReview
  const effectiveInputLine = canAddComments ? activeCommentLine : null;

  // -- Close inline form on file change --
  useEffect(() => {
    setActiveCommentLine(null);
  }, [selectedFile]);

  // -- Keyboard shortcut: `m` toggles the file tree sidebar --
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent): void {
      // Justified: e.target is EventTarget; in a DOM KeyboardEvent it is
      // always an Element or null.
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

  // -- Scroll active file into view --
  useEffect(() => {
    activeFileRef.current?.scrollIntoView({ block: "nearest" });
  }, [selectedFile]);

  // -- Editor mount handler (registers custom themes + inline comment gutter) --
  const handleEditorMount = useCallback<DiffOnMount>((editor, monaco) => {
    diffEditorRef.current = editor;
    monacoRef.current = monaco;

    // Agent C: register custom Monaco themes so they are available
    registerCustomThemes(monaco);

    const modified = editor.getModifiedEditor();
    modified.updateOptions({ glyphMargin: true });

    glyphDecorRef.current = modified.createDecorationsCollection([]);
    commentDecorRef.current = modified.createDecorationsCollection([]);

    // Hover: show "+" glyph on the hovered line
    modified.onMouseMove((e) => {
      if (glyphDecorRef.current == null) return;
      const target = e.target;
      const isHoverArea =
        target.type === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN ||
        target.type === monaco.editor.MouseTargetType.GUTTER_LINE_NUMBERS ||
        target.type ===
          monaco.editor.MouseTargetType.GUTTER_LINE_DECORATIONS ||
        target.type === monaco.editor.MouseTargetType.CONTENT_TEXT;

      if (isHoverArea && target.position != null) {
        const ln = target.position.lineNumber;
        glyphDecorRef.current.set([
          {
            range: new monaco.Range(ln, 1, ln, 1),
            options: { glyphMarginClassName: "inline-comment-glyph" },
          },
        ]);
      } else {
        glyphDecorRef.current.clear();
      }
    });

    // Click on gutter: toggle inline comment form
    modified.onMouseDown((e) => {
      const target = e.target;
      const isGutterClick =
        target.type === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN ||
        target.type === monaco.editor.MouseTargetType.GUTTER_LINE_NUMBERS;

      if (isGutterClick && target.position != null) {
        const line = target.position.lineNumber;
        setActiveCommentLine((prev) => (prev === line ? null : line));
      }
    });

    setEditorReady(true);
  }, []);

  // -- Sync view zones with comments + active input line --
  // useLayoutEffect so zones render before the browser paints
  useLayoutEffect(() => {
    if (
      !editorReady ||
      diffEditorRef.current == null ||
      monacoRef.current == null
    ) {
      return;
    }

    const modified = diffEditorRef.current.getModifiedEditor();
    const monaco = monacoRef.current;

    // Re-apply glyph margin after mode changes
    modified.updateOptions({ glyphMargin: true });

    // Group comments by their end line
    const commentsByLine = new Map<number, Comment[]>();
    for (const c of commentsForFile) {
      const range = anchorRange(c.anchor);
      if (range != null) {
        const line = range[1];
        const arr = commentsByLine.get(line) ?? [];
        arr.push(c);
        commentsByLine.set(line, arr);
      }
    }

    // All lines that need a view zone
    const zoneLines = new Set<number>(commentsByLine.keys());
    if (effectiveInputLine != null) {
      zoneLines.add(effectiveInputLine);
    }

    const newPortals: PortalEntry[] = [];
    const usedKeys = new Set<string>();

    modified.changeViewZones((accessor) => {
      // Remove all previous zones
      for (const id of zoneIdsRef.current) {
        accessor.removeZone(id);
      }
      zoneIdsRef.current = [];

      // Create zones for each line
      for (const line of Array.from(zoneLines).sort((a, b) => a - b)) {
        const comments = commentsByLine.get(line) ?? [];
        const hasInput = line === effectiveInputLine;

        const commentHeight = comments.length * 52;
        const inputHeight = hasInput ? 130 : 0;
        const padding =
          comments.length > 0 || hasInput ? 8 : 0;
        const totalHeight = commentHeight + inputHeight + padding;

        if (totalHeight === 0) continue;

        const key = `zone-${String(line)}`;
        usedKeys.add(key);

        // Reuse DOM nodes so React portals keep component state
        let domNode = domNodeCacheRef.current.get(key);
        if (domNode == null) {
          domNode = document.createElement("div");
          domNode.style.zIndex = "10";
          domNodeCacheRef.current.set(key, domNode);
        }

        const zoneId = accessor.addZone({
          afterLineNumber: line,
          heightInPx: totalHeight,
          domNode,
          suppressMouseDown: true,
        });

        zoneIdsRef.current.push(zoneId);
        newPortals.push({ key, domNode, lineNumber: line, comments, hasInput });
      }
    });

    // Purge stale cached DOM nodes
    for (const cachedKey of domNodeCacheRef.current.keys()) {
      if (!usedKeys.has(cachedKey)) {
        domNodeCacheRef.current.delete(cachedKey);
      }
    }

    // Highlight lines that have comments
    if (commentDecorRef.current != null) {
      commentDecorRef.current.set(
        Array.from(commentsByLine.keys()).map((line) => ({
          range: new monaco.Range(line, 1, line, 1),
          options: {
            isWholeLine: true,
            className: "inline-comment-line-bg",
            glyphMarginClassName: "inline-comment-line-glyph",
          },
        })),
      );
    }

    setPortals(newPortals);
  }, [editorReady, commentsForFile, effectiveInputLine, selectedFile, diffMode]);

  // -- Handlers --
  const handleInlineSubmit = useCallback(
    async (lineNumber: number, body: string) => {
      setSubmitting(true);
      setCommentError(null);
      try {
        await onAddComment(selectedFile, lineNumber, lineNumber, body);
        setActiveCommentLine(null);
      } catch (e: unknown) {
        setCommentError(String(e));
      } finally {
        setSubmitting(false);
      }
    },
    [selectedFile, onAddComment],
  );

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

  const handleRequestChanges = useCallback(async () => {
    setSubmitting(true);
    try {
      await onRequestChanges();
    } finally {
      setSubmitting(false);
    }
  }, [onRequestChanges]);

  const handleSubmitReview = useCallback(async () => {
    const commentCount = review.comments.filter((c) => isLocalOrigin(c.origin)).length;
    if (commentCount === 0) return;

    const confirmed = window.confirm(
      `Post ${String(commentCount)} comment${commentCount !== 1 ? "s" : ""} to GitHub?`,
    );
    if (!confirmed) return;

    setSubmitting(true);
    setMirrorResult(null);
    try {
      const result = await onMirrorComments();
      setMirrorResult(result);
    } finally {
      setSubmitting(false);
    }
  }, [review.comments, onMirrorComments]);

  // =========================================================================
  // Render
  // =========================================================================

  return (
    <div className="flex h-screen flex-col">
      {/* ----------------------------------------------------------------- */}
      {/* Header                                                            */}
      {/* ----------------------------------------------------------------- */}
      <header className="flex shrink-0 items-center gap-3 border-b border-border bg-card px-4 py-2">
        <Button variant="ghost" size="sm" onClick={onBack}>
          <ArrowLeft className="h-4 w-4" />
          Back
        </Button>

        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate text-sm font-semibold">
            {review.branch}
          </span>
          <span className="text-xs text-muted-foreground">{review.issue}</span>
        </div>

        <Badge
          variant="outline"
          className={cn("shrink-0", gateStateBadgeClass(review.gate_state))}
        >
          {gateStateLabel(review.gate_state)}
        </Badge>

        {review.stale && (
          <span className="inline-flex items-center gap-1 text-xs text-danger">
            <AlertTriangle className="h-3 w-3" /> Stale
          </span>
        )}

        {review.agent != null && (
          <span className="inline-flex items-center gap-1 text-xs text-warning">
            <Bot className="h-3 w-3" /> PID {review.agent.pid}
          </span>
        )}

        {/* Split / Unified toggle + actions */}
        <div className="ml-auto flex items-center gap-2">
          <div className="flex overflow-hidden rounded-md border border-border">
            <button
              type="button"
              onClick={() => {
                setDiffMode("split");
              }}
              className={cn(
                "cursor-pointer border-none px-3 py-1 text-xs",
                diffMode === "split"
                  ? "bg-accent text-accent-foreground"
                  : "bg-muted text-muted-foreground hover:bg-accent",
              )}
            >
              Split
            </button>
            <button
              type="button"
              onClick={() => {
                setDiffMode("unified");
              }}
              className={cn(
                "cursor-pointer border-none px-3 py-1 text-xs",
                diffMode === "unified"
                  ? "bg-accent text-accent-foreground"
                  : "bg-muted text-muted-foreground hover:bg-accent",
              )}
            >
              Unified
            </button>
          </div>

          {review.comments.length > 0 && (
            <span className="flex items-center gap-1 text-xs text-muted-foreground">
              <MessageSquare className="h-3 w-3" />
              {String(review.comments.length)}
            </span>
          )}

          {/* Agent B: agent panel toggle */}
          <Button
            variant={agentPanelVisible ? "default" : "outline"}
            size="sm"
            onClick={() => {
              setAgentPanelVisible((prev) => !prev);
            }}
            title="Toggle agent panel"
          >
            <Bot className="h-3.5 w-3.5" />
            Agent
          </Button>

          {review.source === "ReviewRequested" ? (
            review.gate_state === "InReview" && review.comments.length > 0 && (
              <Button
                size="sm"
                className="bg-success text-white hover:bg-success/90"
                onClick={() => void handleSubmitReview()}
                disabled={submitting}
              >
                <Upload className="h-3.5 w-3.5" />
                Submit Review ({String(review.comments.length)})
              </Button>
            )
          ) : (
            <>
              {hasLocalComments && (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => void handleMirrorComments()}
                  disabled={mirroring}
                >
                  <Upload className="h-3.5 w-3.5" />
                  {mirroring ? "Mirroring..." : "Mirror"}
                </Button>
              )}

              {canRequestChanges && (
                <Button
                  variant="destructive"
                  size="sm"
                  onClick={() => void handleRequestChanges()}
                  disabled={submitting}
                >
                  <Send className="h-3.5 w-3.5" />
                  Request Changes ({String(review.comments.length)})
                </Button>
              )}
            </>
          )}
        </div>
      </header>

      {/* ----------------------------------------------------------------- */}
      {/* Comment error banner                                              */}
      {/* ----------------------------------------------------------------- */}
      {commentError !== null && (
        <div className="flex items-center justify-between border-b border-danger bg-danger/10 px-4 py-2 text-xs text-danger">
          <span>Failed to add comment: {commentError}</span>
          <button
            type="button"
            onClick={() => { setCommentError(null); }}
            className="cursor-pointer border-none bg-transparent text-danger underline hover:no-underline"
          >
            Dismiss
          </button>
        </div>
      )}

      {/* ----------------------------------------------------------------- */}
      {/* Mirror result banner                                              */}
      {/* ----------------------------------------------------------------- */}
      {mirrorResult !== null && (
        <div
          className={cn(
            "border-b border-border px-4 py-2 text-xs",
            mirrorResult.failed.length === 0
              ? "bg-success/20 text-success"
              : "bg-danger/20 text-danger",
          )}
        >
          Mirrored: {String(mirrorResult.posted)} posted
          {mirrorResult.failed.length > 0 &&
            `, ${String(mirrorResult.failed.length)} failed`}
          <button
            type="button"
            onClick={() => {
              setMirrorResult(null);
            }}
            className="ml-3 cursor-pointer rounded border border-white/30 bg-transparent px-2 py-0.5 text-[11px] text-foreground"
          >
            Dismiss
          </button>
        </div>
      )}

      {/* ----------------------------------------------------------------- */}
      {/* File tree sidebar + Monaco Diff Editor                            */}
      {/* ----------------------------------------------------------------- */}
      <div className="flex min-h-0 flex-1">
        {/* File tree sidebar */}
        {sidebarOpen && (
          <aside className="flex w-60 shrink-0 flex-col border-r border-border bg-card">
            <div className="flex items-center justify-between border-b border-border px-3 py-2">
              <span className="text-xs font-semibold text-muted-foreground">
                Files ({String(filePaths.length)})
              </span>
              <button
                type="button"
                onClick={() => {
                  setSidebarOpen(false);
                }}
                className="cursor-pointer border-none bg-transparent text-xs text-muted-foreground hover:text-foreground"
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
                const commentCount = fileComments(
                  review.comments,
                  path,
                ).length;
                return (
                  <button
                    key={path}
                    ref={isActive ? activeFileRef : null}
                    type="button"
                    onClick={() => {
                      setSelectedFile(path);
                    }}
                    className={cn(
                      "group/file flex w-full cursor-pointer items-center gap-2 border-l-2 border-y-0 border-r-0 px-3 py-1.5 text-left text-xs",
                      isActive
                        ? "border-l-primary bg-muted"
                        : "border-l-transparent bg-transparent hover:bg-muted",
                    )}
                  >
                    <span
                      className={cn(
                        "w-4 shrink-0 text-center",
                        indicator.className,
                      )}
                    >
                      {indicator.label}
                    </span>
                    <span
                      className="flex-1 truncate text-foreground"
                      title={path}
                    >
                      {path}
                    </span>
                    <button
                      type="button"
                      title="Open in editor"
                      onClick={(e) => {
                        e.stopPropagation();
                        void invoke("open_in_editor", { filePath: path });
                      }}
                      className="shrink-0 cursor-pointer border-none bg-transparent p-0 text-muted-foreground opacity-0 transition-opacity hover:text-foreground group-hover/file:opacity-100"
                    >
                      <ExternalLink className="h-3 w-3" />
                    </button>
                    <span className="flex shrink-0 items-center gap-1.5 text-[10px]">
                      {commentCount > 0 && (
                        <span className="flex items-center gap-0.5 text-state-in-review">
                          <MessageSquare className="h-2.5 w-2.5" />
                          {String(commentCount)}
                        </span>
                      )}
                      {counts.additions > 0 && (
                        <span className="text-success">
                          +{String(counts.additions)}
                        </span>
                      )}
                      {counts.deletions > 0 && (
                        <span className="text-danger">
                          -{String(counts.deletions)}
                        </span>
                      )}
                    </span>
                  </button>
                );
              })}
              {filePaths.length === 0 && (
                <span className="block px-3 py-2 text-xs text-muted-foreground">
                  No files in diff
                </span>
              )}
            </nav>
          </aside>
        )}

        {/* Collapsed sidebar toggle */}
        {!sidebarOpen && (
          <button
            type="button"
            onClick={() => {
              setSidebarOpen(true);
            }}
            className="flex w-6 shrink-0 cursor-pointer items-center justify-center border-y-0 border-l-0 border-r border-border bg-card text-muted-foreground hover:bg-muted hover:text-foreground"
            title="Show file tree (m)"
          >
            &raquo;
          </button>
        )}

        {/* Monaco Diff Editor */}
        <div className="relative min-h-0 flex-1">
          {selectedFile !== "" ? (
            <DiffEditor
              original={currentFileDiff.original}
              modified={currentFileDiff.modified}
              language={detectLanguage(selectedFile)}
              theme={editorTheme}
              options={{
                readOnly: true,
                renderSideBySide: diffMode === "split",
                minimap: { enabled: false },
                scrollBeyondLastLine: false,
                fontSize: 13,
              }}
              onMount={handleEditorMount}
            />
          ) : (
            <div className="flex h-full items-center justify-center text-muted-foreground">
              No diff available
            </div>
          )}

          {/* Inline comment portals (rendered into Monaco view zone DOM nodes) */}
          {portals.map((entry) =>
            createPortal(
              <InlineCommentThread
                comments={entry.comments}
                lineNumber={entry.lineNumber}
                hasInput={entry.hasInput}
                onSubmit={(body) => {
                  void handleInlineSubmit(entry.lineNumber, body);
                }}
                onCancel={() => {
                  setActiveCommentLine(null);
                }}
              />,
              entry.domNode,
              entry.key,
            ),
          )}
        </div>
      </div>

      {/* ----------------------------------------------------------------- */}
      {/* Bottom action bar                                                 */}
      {/* ----------------------------------------------------------------- */}
      <div className="flex shrink-0 items-center gap-3 border-t border-border bg-card px-4 py-2">
        <MessageSquare className="h-3.5 w-3.5 text-muted-foreground" />
        <span className="text-xs text-muted-foreground">
          {review.comments.length > 0
            ? `${String(review.comments.length)} comment${review.comments.length !== 1 ? "s" : ""} total`
            : "No comments yet"}
          {commentsForFile.length > 0 &&
            review.comments.length > commentsForFile.length &&
            ` · ${String(commentsForFile.length)} on this file`}
        </span>

        {canAddComments && (
          <span className="text-[10px] text-muted-foreground">
            Click a line number to comment
          </span>
        )}

        {review.source === "ReviewRequested" && review.comments.length > 0 && (
          <span className="text-[10px] text-muted-foreground">
            Comments will be posted to GitHub on Submit
          </span>
        )}

        <div className="flex-1" />

        {/* Show count of comments on other files */}
        {review.comments.length > commentsForFile.length &&
          commentsForFile.length > 0 && (
            <span className="text-[10px] text-muted-foreground">
              +{String(review.comments.length - commentsForFile.length)} on
              other files
            </span>
          )}
      </div>

      {/* Agent B: agent activity panel */}
      <AgentPanel
        visible={agentPanelVisible}
        onClose={() => {
          setAgentPanelVisible(false);
        }}
      />
    </div>
  );
}
