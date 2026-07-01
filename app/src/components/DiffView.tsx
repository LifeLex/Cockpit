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
  useRef,
} from "react";
import { createPortal } from "react-dom";
import {
  DiffEditor,
  type DiffBeforeMount,
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
import type { CiSummary } from "../bindings/CiSummary";
import { summarizeChecks, ciState, parseCiUpdate } from "@/lib/ci";
import { parseDiff, extractFilePaths } from "../diff-parser";
import type { FileDiff } from "../diff-parser";
import { elapsedSince } from "@/lib/relative-time";
import { useAppStore } from "../store";
import { registerCustomThemes } from "@/lib/monaco-themes";
import { attachLspClient, type LspAttachment } from "@/lib/lsp-client";
import { AgentPanel } from "./AgentPanel";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openExternal } from "@/lib/open";
import {
  ArrowLeft,
  ExternalLink,
  MessageSquare,
  Upload,
  Send,
  AlertTriangle,
  Bot,
  Hash,
  GitBranch,
  CheckCircle2,
  XCircle,
  Loader2,
  Wrench,
  Layers,
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

/** Status-LED background color for a gate state, using `--color-state-*`. */
function gateLedColorClass(state: GateState): string {
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

/**
 * The status LED for the identity zone: a gate-state-colored dot that pulses
 * only while an agent is actively dispatched (mirrors the card LED).
 */
function StatusLed({ review }: { readonly review: Review }) {
  const pulses = review.gate_state === "Dispatched" && !review.stale;
  const color = gateLedColorClass(review.gate_state);
  return (
    <span className="relative flex h-2.5 w-2.5 shrink-0" aria-hidden="true">
      {pulses && (
        <span
          className={cn(
            "absolute inline-flex h-full w-full animate-ping rounded-full opacity-60",
            color,
          )}
        />
      )}
      <span
        className={cn("relative inline-flex h-2.5 w-2.5 rounded-full", color)}
      />
    </span>
  );
}

/**
 * The human-readable agent reason line shown in place of a raw PID: e.g.
 * `Agent working · 3m`. Returns null when no agent is attached. `now` is
 * injected for deterministic tests and defaults to the wall clock.
 */
function agentReasonLine(review: Review, now: number = Date.now()): string | null {
  if (review.agent === null) return null;
  const elapsed = elapsedSince(review.agent.started_at, now);
  return `Agent working · ${elapsed}`;
}

/**
 * Build a GitHub PR URL from a PrRef like `owner/repo#42`.
 * Returns null if the format does not match.
 */
function prUrl(pr: string): string | null {
  const match = /^([^#]+)#(\d+)$/.exec(pr);
  if (match === null) return null;
  // INVARIANT: regex matched with two capture groups
  return `https://github.com/${match[1]}/pull/${match[2]}`;
}

/**
 * Build a Linear issue URL from an IssueRef like `NEX-123`.
 * Returns null if the ref does not look like a Linear identifier.
 */
function issueUrl(issue: string): string | null {
  const match = /^([A-Za-z]+)-(\d+)$/.exec(issue);
  if (match === null) return null;
  // INVARIANT: regex matched with two capture groups
  return `https://linear.app/issue/${match[1]}-${match[2]}`;
}

/**
 * Build a GitHub repository URL from a repo slug like `owner/repo`.
 * Returns null if the slug does not match `owner/repo`.
 */
function repoUrl(slug: string): string | null {
  const match = /^[^/]+\/[^/]+$/.exec(slug);
  if (match === null) return null;
  return `https://github.com/${slug}`;
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
): { readonly label: string; readonly className: string; readonly title: string } {
  switch (status) {
    case "added":
      return { label: "+", className: "text-success", title: "Added" };
    case "modified":
      return { label: "±", className: "text-warning", title: "Modified" };
    case "deleted":
      return { label: "−", className: "text-danger", title: "Deleted" };
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

  // -- LSP: workspace root + enable toggle from config --
  const lspRootPath = useAppStore((s) => s.config?.repo_path ?? null);
  const lspEnabled = useAppStore((s) => s.config?.lsp_servers.enabled ?? true);

  // -- Diff parsing --
  const fileDiffs = useMemo(() => parseDiff(diff.raw), [diff.raw]);
  const filePaths = useMemo(() => extractFilePaths(diff.raw), [diff.raw]);

  // -- Navigation / display state --
  const [selectedFile, setSelectedFile] = useState<string>(
    filePaths[0] ?? "",
  );
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [diffMode, setDiffMode] = useState<"split" | "unified">("split");
  const [stackOpen, setStackOpen] = useState(false);
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

  // -- CI checks state --
  const [ciSummary, setCiSummary] = useState<CiSummary | null>(null);
  const [fixingCi, setFixingCi] = useState(false);

  // -- Restack state --
  const restackPr = useAppStore((s) => s.restackPr);
  const [restacking, setRestacking] = useState(false);

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
  const lspAttachmentRef = useRef<LspAttachment | null>(null);

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

  // -- Relocated PR-info: external reference links --
  const prHref = useMemo(() => prUrl(review.pr), [review.pr]);
  const issueHref = useMemo(() => issueUrl(review.issue), [review.issue]);
  const repoHref = useMemo(
    () => (review.repo_slug !== null ? repoUrl(review.repo_slug) : null),
    [review.repo_slug],
  );

  // -- Relocated PR-info: stack parents/children --
  const hasStack = review.parents.length > 0 || review.children.length > 0;

  // -- Agent reason line (replaces the raw PID readout) --
  const agentReason = useMemo(() => agentReasonLine(review), [review]);

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

  // -- Editor before-mount handler (registers custom themes) --
  // Themes must be defined BEFORE the editor is instantiated; registering in
  // onMount races with `<DiffEditor theme>` and falls back to vs-dark on the
  // first load. registerCustomThemes is idempotent, so calling it here (and
  // again in onMount as belt-and-suspenders) is safe.
  const handleBeforeMount = useCallback<DiffBeforeMount>((monaco) => {
    registerCustomThemes(monaco);
  }, []);

  // -- Editor mount handler (registers custom themes + inline comment gutter) --
  const handleEditorMount = useCallback<DiffOnMount>(
    (editor, monaco) => {
      diffEditorRef.current = editor;
      monacoRef.current = monaco;

      // Belt-and-suspenders: ensure themes exist and the selected theme is
      // applied even if beforeMount timing ever changes.
      registerCustomThemes(monaco);
      monaco.editor.setTheme(editorTheme);

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
    },
    [editorTheme],
  );

  // -- Sync view zones with comments + active input line --
  // useEffect (not useLayoutEffect) so zones are created AFTER Monaco's
  // internal useEffect updates the editor models on file switch.
  useEffect(() => {
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
          suppressMouseDown: false,
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

  // -- LSP: attach a language client to the modified (right-hand) model --
  // Runs once the editor is ready and whenever the selected file (and thus its
  // language) changes. Only languages with a configured server (typescript /
  // javascript / python) attach; everything else keeps plain highlighting.
  // The attachment is torn down on file change and on unmount (didClose +
  // socket close) so no stale server session or diagnostics linger.
  useEffect(() => {
    // Always tear down the previous attachment before (maybe) opening a new one.
    lspAttachmentRef.current?.dispose();
    lspAttachmentRef.current = null;

    if (
      !editorReady ||
      !lspEnabled ||
      lspRootPath === null ||
      selectedFile === "" ||
      diffEditorRef.current === null ||
      monacoRef.current === null
    ) {
      return;
    }

    const languageId = detectLanguage(selectedFile);
    const model = diffEditorRef.current.getModifiedEditor().getModel();
    if (model === null) return;

    const monaco = monacoRef.current;
    let cancelled = false;

    void attachLspClient({ monaco, model, languageId, rootPath: lspRootPath })
      .then((attachment) => {
        if (attachment === null) return;
        if (cancelled) {
          // The effect was torn down while the socket was opening.
          attachment.dispose();
          return;
        }
        lspAttachmentRef.current = attachment;
      })
      .catch((e: unknown) => {
        console.error("attachLspClient failed", e);
      });

    return () => {
      cancelled = true;
      lspAttachmentRef.current?.dispose();
      lspAttachmentRef.current = null;
    };
  }, [editorReady, lspEnabled, lspRootPath, selectedFile]);

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

  // -- CI checks: fetch on load and update via the `ci-updated` event --
  const prRef = review.pr;
  useEffect(() => {
    let cancelled = false;

    // Initial fetch (STATUS tier). Non-fatal: a fetch failure leaves the badge
    // empty rather than surfacing an error — CI never blocks the review loop.
    void invoke<CiSummary>("fetch_ci_checks", { pr: prRef })
      .then((summary) => {
        if (!cancelled) setCiSummary(summary);
      })
      .catch((e: unknown) => {
        console.error("fetch_ci_checks failed", e);
      });

    // Live updates: the backend pushes the full checks list via `ci-updated`.
    const unlisten = listen<unknown>("ci-updated", (event) => {
      const update = parseCiUpdate(event.payload);
      if (update === null || update.pr !== prRef) return;
      setCiSummary(summarizeChecks(update.checks));
    });

    return () => {
      cancelled = true;
      void unlisten.then((f) => {
        f();
      });
    };
  }, [prRef]);

  const ciBadgeState = useMemo(
    () => (ciSummary !== null ? ciState(ciSummary) : "none"),
    [ciSummary],
  );

  const handleFixCi = useCallback(async () => {
    const confirmed = window.confirm(
      "Dispatch an agent to fix the failing CI checks? This transitions the review to Dispatched and runs the fixer agent.",
    );
    if (!confirmed) return;
    setFixingCi(true);
    try {
      await invoke("fix_ci", { pr: prRef });
    } catch (e: unknown) {
      console.error("fix_ci failed", e);
    } finally {
      setFixingCi(false);
    }
  }, [prRef]);

  const handleRestack = useCallback(async () => {
    setRestacking(true);
    try {
      await restackPr(prRef);
    } finally {
      setRestacking(false);
    }
  }, [restackPr, prRef]);

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
    <div className="flex h-full flex-col">
      {/* ----------------------------------------------------------------- */}
      {/* Header                                                            */}
      {/* ----------------------------------------------------------------- */}
      <header className="flex shrink-0 items-center gap-4 border-b border-border bg-card px-4 py-2">
        {/* ============================================================= */}
        {/* Zone 1 — Identity (left)                                       */}
        {/* ============================================================= */}
        <div className="flex min-w-0 flex-1 items-center gap-3">
          <Button
            variant="ghost"
            size="sm"
            onClick={onBack}
            title="Back to the board"
          >
            <ArrowLeft className="h-4 w-4" />
            Back
          </Button>

          <StatusLed review={review} />

          <div className="flex min-w-0 flex-col">
            {/* Title row: PR subject + gate-state pill + inline flags. */}
            <div className="flex min-w-0 items-center gap-2">
              <span className="truncate font-display text-sm font-semibold text-foreground">
                {review.branch}
              </span>
              <Badge
                variant="outline"
                className={cn(
                  "shrink-0",
                  gateStateBadgeClass(review.gate_state),
                )}
              >
                {gateStateLabel(review.gate_state)}
              </Badge>
              {review.stale && (
                <span className="inline-flex shrink-0 items-center gap-1 text-xs text-danger">
                  <AlertTriangle className="h-3 w-3" /> Stale
                </span>
              )}
            </div>

            {/* Refs line: mono, faint; PR / issue / repo links via opener. */}
            <div className="flex min-w-0 flex-wrap items-center gap-x-2.5 gap-y-0.5 font-mono text-xs text-muted-foreground">
              {prHref !== null ? (
                <a
                  href={prHref}
                  onClick={(e) => {
                    e.preventDefault();
                    void openExternal(prHref);
                  }}
                  className="inline-flex shrink-0 items-center gap-1 text-primary hover:underline"
                  title="Open PR on GitHub"
                >
                  {review.pr}
                  <ExternalLink className="h-3 w-3" />
                </a>
              ) : (
                <span className="shrink-0">{review.pr}</span>
              )}

              {issueHref !== null ? (
                <a
                  href={issueHref}
                  onClick={(e) => {
                    e.preventDefault();
                    void openExternal(issueHref);
                  }}
                  className="inline-flex shrink-0 items-center gap-1 hover:text-foreground hover:underline"
                  title="Open issue on Linear"
                >
                  <Hash className="h-3 w-3" />
                  {review.issue}
                </a>
              ) : (
                <span className="inline-flex shrink-0 items-center gap-1">
                  <Hash className="h-3 w-3" />
                  {review.issue}
                </span>
              )}

              {review.repo_slug !== null &&
                (repoHref !== null ? (
                  <a
                    href={repoHref}
                    onClick={(e) => {
                      e.preventDefault();
                      void openExternal(repoHref);
                    }}
                    className="inline-flex shrink-0 items-center gap-1 hover:text-foreground hover:underline"
                    title="Open repository on GitHub"
                  >
                    {review.repo_slug}
                    <ExternalLink className="h-3 w-3" />
                  </a>
                ) : (
                  <span className="shrink-0">{review.repo_slug}</span>
                ))}

              {/* Agent reason line replaces the raw PID readout. */}
              {agentReason !== null && (
                <span className="inline-flex shrink-0 items-center gap-1 text-state-dispatched">
                  <Bot className="h-3 w-3" />
                  {agentReason}
                </span>
              )}
            </div>
          </div>
        </div>

        {/* ============================================================= */}
        {/* Zone 2 — View controls (center)                               */}
        {/* ============================================================= */}
        <div className="flex shrink-0 items-center gap-2 rounded-lg border border-border bg-muted/30 px-2 py-1">
          {/* Split / Unified segmented control. */}
          <div className="flex overflow-hidden rounded-md border border-border">
            <button
              type="button"
              onClick={() => {
                setDiffMode("split");
              }}
              aria-pressed={diffMode === "split"}
              className={cn(
                "cursor-pointer border-none px-3 py-1 text-xs font-medium transition-colors",
                diffMode === "split"
                  ? "bg-accent text-accent-foreground"
                  : "bg-transparent text-muted-foreground hover:bg-accent/50",
              )}
            >
              Split
            </button>
            <button
              type="button"
              onClick={() => {
                setDiffMode("unified");
              }}
              aria-pressed={diffMode === "unified"}
              className={cn(
                "cursor-pointer border-none px-3 py-1 text-xs font-medium transition-colors",
                diffMode === "unified"
                  ? "bg-accent text-accent-foreground"
                  : "bg-transparent text-muted-foreground hover:bg-accent/50",
              )}
            >
              Unified
            </button>
          </div>

          {/* CI checks badge. */}
          {ciSummary !== null && ciSummary.total > 0 && (
            <span
              className={cn(
                "inline-flex shrink-0 items-center gap-1 rounded-md border px-1.5 py-0.5 font-mono text-xs tabular-nums",
                ciBadgeState === "pass" &&
                  "border-success/30 bg-success/15 text-success",
                ciBadgeState === "fail" &&
                  "border-danger/30 bg-danger/15 text-danger",
                ciBadgeState === "pending" &&
                  "border-warning/30 bg-warning/15 text-warning",
              )}
              title={`CI: ${String(ciSummary.passed)} passed, ${String(ciSummary.failed)} failed, ${String(ciSummary.pending)} pending`}
            >
              {ciBadgeState === "pass" && <CheckCircle2 className="h-3 w-3" />}
              {ciBadgeState === "fail" && <XCircle className="h-3 w-3" />}
              {ciBadgeState === "pending" && (
                <Loader2 className="h-3 w-3 animate-spin" />
              )}
              {String(ciSummary.passed)}/{String(ciSummary.total)}
            </span>
          )}

          {/* Stack strip toggle. */}
          {hasStack && (
            <button
              type="button"
              onClick={() => {
                setStackOpen((prev) => !prev);
              }}
              aria-expanded={stackOpen}
              className={cn(
                "inline-flex shrink-0 cursor-pointer items-center gap-1 rounded-md border px-1.5 py-0.5 text-xs transition-colors",
                stackOpen
                  ? "border-border bg-accent text-accent-foreground"
                  : "border-transparent bg-transparent text-muted-foreground hover:bg-accent/50",
              )}
              title="Toggle stack"
            >
              <GitBranch className="h-3 w-3" />
              Stack
            </button>
          )}
        </div>

        {/* ============================================================= */}
        {/* Zone 3 — Actions (far right)                                   */}
        {/* ============================================================= */}
        <div className="flex shrink-0 items-center gap-2">
          {/* Secondary cluster: ghost/outline actions. */}
          <div className="flex items-center gap-1.5">
            {/* Agent panel toggle. */}
            <Button
              variant={agentPanelVisible ? "outline" : "ghost"}
              size="sm"
              onClick={() => {
                setAgentPanelVisible((prev) => !prev);
              }}
              title="Toggle agent panel"
            >
              <Bot className="h-3.5 w-3.5" />
              Agent
            </Button>

            {/* Restack — explicit user action; only when the review is stale.
                Operates only on the review's own branch (Invariant 5 / §9). */}
            {review.stale && (
              <Button
                variant="outline"
                size="sm"
                className="border-danger/40 text-danger hover:bg-danger/10"
                onClick={() => void handleRestack()}
                disabled={restacking || review.agent != null}
                title="Rebase this review onto its parent's new head"
              >
                <Layers className="h-3.5 w-3.5" />
                {restacking || review.agent != null ? "Restacking…" : "Restack"}
              </Button>
            )}

            {/* Fix CI failures — explicit user action; only when CI is failing. */}
            {ciSummary !== null && ciSummary.failed > 0 && (
              <Button
                variant="outline"
                size="sm"
                className="border-danger/40 text-danger hover:bg-danger/10"
                onClick={() => void handleFixCi()}
                disabled={fixingCi || review.gate_state === "Dispatched"}
                title="Dispatch an agent to fix the failing CI checks"
              >
                <Wrench className="h-3.5 w-3.5" />
                {fixingCi ? "Dispatching..." : "Fix CI"}
              </Button>
            )}

            {/* Mirror — outline secondary; authored reviews with local
                comments. Submit path uses the primary action below. */}
            {review.source !== "ReviewRequested" && hasLocalComments && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => void handleMirrorComments()}
                disabled={mirroring}
                title="Mirror local comments to the GitHub PR thread"
              >
                <Upload className="h-3.5 w-3.5" />
                {mirroring ? "Mirroring..." : "Mirror"}
              </Button>
            )}
          </div>

          {/* Primary action — one clear call to action by state. */}
          {review.source === "ReviewRequested"
            ? review.gate_state === "InReview" &&
              review.comments.length > 0 && (
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
            : canRequestChanges && (
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
        </div>
      </header>

      {/* ----------------------------------------------------------------- */}
      {/* Stack strip (collapsible) — relocated from PR Info tab            */}
      {/* ----------------------------------------------------------------- */}
      {hasStack && stackOpen && (
        <div className="shrink-0 border-b border-border bg-card/50 px-4 py-1.5 text-xs">
          <div className="flex items-center gap-1.5 text-[10px] uppercase tracking-wide text-muted-foreground">
            <GitBranch className="h-3 w-3" />
            Stack ({String(review.parents.length)} up ·{" "}
            {String(review.children.length)} down)
          </div>
          <div className="mt-1.5 space-y-1.5 pl-5">
            {review.parents.length > 0 && (
              <div className="flex items-start gap-2">
                <span className="w-14 shrink-0 text-[10px] uppercase tracking-wide text-muted-foreground">
                  Parents
                </span>
                <div className="flex flex-wrap gap-1">
                  {review.parents.map((p) => (
                    <Badge
                      key={p}
                      variant="outline"
                      className="font-mono text-[10px]"
                    >
                      {p}
                    </Badge>
                  ))}
                </div>
              </div>
            )}
            {review.children.length > 0 && (
              <div className="flex items-start gap-2">
                <span className="w-14 shrink-0 text-[10px] uppercase tracking-wide text-muted-foreground">
                  Children
                </span>
                <div className="flex flex-wrap gap-1">
                  {review.children.map((c) => (
                    <Badge
                      key={c}
                      variant="outline"
                      className="font-mono text-[10px]"
                    >
                      {c}
                    </Badge>
                  ))}
                </div>
              </div>
            )}
          </div>
        </div>
      )}

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
              <span className="font-display text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                Files{" "}
                <span className="font-mono tabular-nums text-muted-foreground/70">
                  {String(filePaths.length)}
                </span>
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
                        "w-4 shrink-0 text-center font-mono font-semibold",
                        indicator.className,
                      )}
                      title={indicator.title}
                      aria-label={indicator.title}
                    >
                      {indicator.label}
                    </span>
                    <span
                      className="flex-1 truncate font-mono text-foreground"
                      title={path}
                    >
                      {path}
                    </span>
                    <span
                      role="button"
                      tabIndex={0}
                      title="Open in editor"
                      onClick={(e) => {
                        e.stopPropagation();
                        void invoke("open_in_editor", {
                          filePath: path,
                          repoSlug: review.repo_slug,
                          branch: review.branch,
                        });
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          e.stopPropagation();
                          void invoke("open_in_editor", {
                            filePath: path,
                            repoSlug: review.repo_slug,
                            branch: review.branch,
                          });
                        }
                      }}
                      className="shrink-0 cursor-pointer border-none bg-transparent p-0 text-muted-foreground opacity-0 transition-opacity hover:text-foreground group-hover/file:opacity-100"
                    >
                      <ExternalLink className="h-3 w-3" />
                    </span>
                    <span className="flex shrink-0 items-center gap-1.5 font-mono text-[10px] tabular-nums">
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
                          {"−"}
                          {String(counts.deletions)}
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
              beforeMount={handleBeforeMount}
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
