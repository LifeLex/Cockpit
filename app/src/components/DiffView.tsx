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
import type { DiffSide } from "../bindings/DiffSide";
import type { CiSummary } from "../bindings/CiSummary";
import type { CiCheck } from "../bindings/CiCheck";
import { summarizeChecks, ciState, parseCiUpdate } from "@/lib/ci";
import type { ReviewEvent } from "../bindings/ReviewEvent";
import type { SubmitReviewResult } from "../bindings/SubmitReviewResult";
import type { EvidenceSummary } from "../bindings/EvidenceSummary";
import type { FilePair } from "../bindings/FilePair";
import type { ReviewFinding } from "../bindings/ReviewFinding";
import type { FindingSeverity } from "../bindings/FindingSeverity";
import {
  parseDiff,
  extractFilePaths,
  fragmentToReal,
  realToFragment,
  identityLineMap,
} from "../diff-parser";
import type { FileDiff } from "../diff-parser";
import { pairRequests } from "@/lib/request-pairing";
import { elapsedSince } from "@/lib/relative-time";
import { useAppStore } from "../store";
import { registerCustomThemes } from "@/lib/monaco-themes";
import { attachLspClient, type LspAttachment } from "@/lib/lsp-client";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { GatePill } from "./GatePill";
import { IntentPanel } from "./IntentPanel";
import { AddressedRequests } from "./AddressedRequests";
import { EvidenceStrip } from "./EvidenceStrip";
import { SubmitReviewControl } from "./SubmitReviewControl";
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
  BotMessageSquare,
  Hash,
  GitBranch,
  CheckCircle2,
  XCircle,
  Loader2,
  Wrench,
  Layers,
  Check,
  GitMerge,
  X,
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
    side: DiffSide,
  ) => Promise<void>;
  readonly onRequestChanges: () => Promise<void>;
  readonly onMirrorComments: () => Promise<MirrorResult | null>;
  /** Switch the enclosing workspace to the canonical Agent tab. */
  readonly onOpenAgent: () => void;
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

interface PortalEntry {
  readonly key: string;
  readonly domNode: HTMLDivElement;
  /**
   * The REAL file line, used for display and as the comment anchor when
   * submitting (D1). Zone placement uses the fragment line instead.
   */
  readonly lineNumber: number;
  /** Which diff side this zone lives on: `New` (modified) or `Old` (original). */
  readonly side: DiffSide;
  readonly comments: readonly Comment[];
  readonly hasInput: boolean;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
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
    case "Merged":
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
): anchor is {
  readonly DiffLine: { path: string; range: [number, number]; side: DiffSide };
} {
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

/**
 * Which diff side a comment anchors to. Defaults to `New` for non-diff anchors
 * and legacy data (matching the server-side `serde` default).
 */
function anchorSide(anchor: Anchor): DiffSide {
  if (isDiffLineAnchor(anchor)) {
    return anchor.DiffLine.side;
  }
  return "New";
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
  side,
  hasInput,
  onSubmit,
  onCancel,
}: {
  readonly comments: readonly Comment[];
  readonly lineNumber: number;
  readonly side: DiffSide;
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
              {side === "Old" ? "old line " : "Line "}
              {String(lineNumber)}
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
// FindingPin -- advisory review findings, rendered in Monaco view zones
// ---------------------------------------------------------------------------

/** Presentation for a finding severity: dot / border / text color + label. */
interface SeverityMeta {
  readonly label: string;
  readonly dot: string;
  readonly border: string;
  readonly text: string;
  /** The Monaco glyph-margin class that draws the dashed severity rail. */
  readonly glyph: string;
}

function severityMeta(severity: FindingSeverity): SeverityMeta {
  switch (severity) {
    case "Info":
      return {
        label: "Info",
        dot: "bg-muted-foreground",
        border: "border-muted-foreground/50",
        text: "text-muted-foreground",
        glyph: "finding-line-info",
      };
    case "Warning":
      return {
        label: "Warning",
        dot: "bg-warning",
        border: "border-warning/60",
        text: "text-warning",
        glyph: "finding-line-warning",
      };
    case "Critical":
      return {
        label: "Critical",
        dot: "bg-danger",
        border: "border-danger/60",
        text: "text-danger",
        glyph: "finding-line-critical",
      };
    default:
      return assertNever(severity);
  }
}

/**
 * An advisory finding from the read-only pre-pass reviewer, rendered inline as a
 * Monaco view zone. Deliberately distinct from a human [`InlineCommentThread`]:
 * a dashed severity-colored border plus a severity label, and a dismiss control.
 * Findings never count toward the Request Changes requirement.
 */
function FindingPin({
  finding,
  onDismiss,
}: {
  readonly finding: ReviewFinding;
  readonly onDismiss: () => void;
}) {
  const meta = severityMeta(finding.severity);
  return (
    <div
      className={cn(
        "mx-1 my-0.5 overflow-hidden rounded-md border border-dashed bg-card/80 px-3 py-2 text-xs shadow-sm",
        meta.border,
      )}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="inline-flex items-center gap-1.5">
          <span
            className={cn("h-2 w-2 shrink-0 rounded-full", meta.dot)}
            aria-hidden="true"
          />
          <span
            className={cn(
              "font-semibold uppercase tracking-wide",
              meta.text,
            )}
          >
            {meta.label}
          </span>
          <span className="text-[10px] uppercase tracking-wide text-muted-foreground">
            · advisory finding
          </span>
        </span>
        <button
          type="button"
          onClick={onDismiss}
          aria-label="Dismiss finding"
          title="Dismiss finding"
          className="shrink-0 cursor-pointer rounded border-none bg-transparent p-0.5 text-muted-foreground hover:text-foreground"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>
      <div className="mt-1 font-medium text-foreground">{finding.title}</div>
      <div className="mt-0.5 whitespace-pre-wrap leading-relaxed text-muted-foreground">
        {finding.rationale}
      </div>
    </div>
  );
}

/** A portal entry for a finding zone (mirrors {@link PortalEntry} for comments). */
interface FindingPortalEntry {
  readonly key: string;
  readonly domNode: HTMLDivElement;
  readonly finding: ReviewFinding;
}

/**
 * Estimate a finding zone's pixel height from its rationale, so the view zone
 * reserves enough room. A rough character-per-line wrap is good enough; Monaco
 * clips gracefully if the estimate is short.
 */
function findingZoneHeight(finding: ReviewFinding): number {
  const perLine = 88;
  const rationaleLines = finding.rationale
    .split("\n")
    .reduce((n, l) => n + Math.max(1, Math.ceil(l.length / perLine)), 0);
  return 52 + rationaleLines * 18;
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
  onOpenAgent,
}: DiffViewProps) {
  // -- Agent C: editor theme from store --
  const editorTheme = useAppStore((s) => s.editorTheme);

  // -- LSP: workspace root + enable toggle from config --
  const lspRootPath = useAppStore((s) => s.config?.repo_path ?? null);
  const lspEnabled = useAppStore((s) => s.config?.lsp_servers.enabled ?? true);

  // -- Close-the-loop store actions (D2 / D9 / D10) --
  const approveReview = useAppStore((s) => s.approveReview);
  const mergeReview = useAppStore((s) => s.mergeReview);
  const submitGithubReview = useAppStore((s) => s.submitGithubReview);
  const fetchInterdiff = useAppStore((s) => s.fetchInterdiff);

  // -- Phase B store actions (evidence / pre-review / full-file) --
  const fetchEvidence = useAppStore((s) => s.fetchEvidence);
  const preReview = useAppStore((s) => s.preReview);
  const fetchFilePair = useAppStore((s) => s.fetchFilePair);
  const listCiChecks = useAppStore((s) => s.listCiChecks);

  // -- D10: interdiff (changes since the last review dispatch) --
  // A dispatch snapshot survives the Reworked→InReview reopen (it is cleared
  // only on the next dispatch), so the "changes since your review" view and the
  // D1 request pairing apply in BOTH states: Reworked (before reopen) and
  // InReview (after reopen, where the Approve button lives).
  const hasInterdiff =
    review.dispatch_snapshot != null &&
    (review.gate_state === "Reworked" || review.gate_state === "InReview");
  const [interdiff, setInterdiff] = useState<DiffData | null>(null);
  const [diffSource, setDiffSource] = useState<"interdiff" | "full">("full");

  // Whether the interdiff is the active view (it exempts the full-file toggle).
  const interdiffActive = diffSource === "interdiff" && interdiff !== null;

  // The raw diff currently shown: the interdiff when selected and available,
  // otherwise the full review diff.
  const activeDiffRaw = interdiffActive ? interdiff.raw : diff.raw;

  // -- B1/B3: review-time evidence bundle (refetched on head change) --
  const [evidence, setEvidence] = useState<EvidenceSummary | null>(null);
  // Raw CI checks (for the evidence strip's failing-job name), kept alongside
  // the summarized badge; populated from the initial fetch and `ci-updated`.
  const [ciChecks, setCiChecks] = useState<readonly CiCheck[]>([]);

  // -- B2: advisory pre-pass reviewer + component-local finding dismissals --
  const [preReviewing, setPreReviewing] = useState(false);
  const [dismissedFindings, setDismissedFindings] = useState<
    ReadonlySet<string>
  >(new Set());
  const [findingPortals, setFindingPortals] = useState<
    readonly FindingPortalEntry[]
  >([]);

  // -- B4: Hunks vs Full-file view + the resolved full-file pair --
  const [viewMode, setViewMode] = useState<"hunks" | "full">("hunks");
  const [fullPair, setFullPair] = useState<FilePair | null>(null);

  // Bumped on each jump-to-line request so the apply effect re-runs even when
  // the target file is already selected.
  const [jumpNonce, setJumpNonce] = useState(0);

  // -- Diff parsing --
  const fileDiffs = useMemo(() => parseDiff(activeDiffRaw), [activeDiffRaw]);
  const filePaths = useMemo(
    () => extractFilePaths(activeDiffRaw),
    [activeDiffRaw],
  );

  // -- Navigation / display state --
  const [selectedFile, setSelectedFile] = useState<string>(
    filePaths[0] ?? "",
  );
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [diffMode, setDiffMode] = useState<"split" | "unified">("split");
  const [stackOpen, setStackOpen] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  // -- Inline comment state --
  // Holds the FRAGMENT (Monaco) line and which diff side its editor belongs to,
  // so the New (modified) and Old (original) gutters each open their own form.
  const [activeComment, setActiveComment] = useState<{
    readonly side: DiffSide;
    readonly line: number;
  } | null>(null);
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

  // -- D2: approve / merge state --
  const [approving, setApproving] = useState(false);
  const [merging, setMerging] = useState(false);

  // -- D9: GitHub review submission state --
  const [githubSubmitting, setGithubSubmitting] = useState(false);
  const [githubSubmitResult, setGithubSubmitResult] =
    useState<SubmitReviewResult | null>(null);

  // -- D4: per-review Intent disclosure open state (component-only memory) --
  const [intentOpenByPr, setIntentOpenByPr] = useState<
    Readonly<Record<string, boolean>>
  >({});

  // -- D10: Addressed-requests panel open state (open by default on entry) --
  const [addressedOpen, setAddressedOpen] = useState(true);

  // -- Refs --
  const activeFileRef = useRef<HTMLButtonElement | null>(null);
  const diffEditorRef = useRef<MonacoDiffEditor | null>(null);
  const monacoRef = useRef<Monaco | null>(null);
  const zoneIdsRef = useRef<string[]>([]);
  const originalZoneIdsRef = useRef<string[]>([]);
  const domNodeCacheRef = useRef<Map<string, HTMLDivElement>>(new Map());
  const findingZoneIdsRef = useRef<string[]>([]);
  const originalFindingZoneIdsRef = useRef<string[]>([]);
  const findingNodeCacheRef = useRef<Map<string, HTMLDivElement>>(new Map());
  const glyphDecorRef =
    useRef<MonacoEditorNs.IEditorDecorationsCollection | null>(null);
  const commentDecorRef =
    useRef<MonacoEditorNs.IEditorDecorationsCollection | null>(null);
  const originalGlyphDecorRef =
    useRef<MonacoEditorNs.IEditorDecorationsCollection | null>(null);
  const originalCommentDecorRef =
    useRef<MonacoEditorNs.IEditorDecorationsCollection | null>(null);
  const findingDecorRef =
    useRef<MonacoEditorNs.IEditorDecorationsCollection | null>(null);
  const originalFindingDecorRef =
    useRef<MonacoEditorNs.IEditorDecorationsCollection | null>(null);
  const lspAttachmentRef = useRef<LspAttachment | null>(null);
  // Pending jump-to-line request (from evidence-strip weakening chips), applied
  // once the target file's editor + line map are ready.
  const pendingJumpRef = useRef<{
    readonly path: string;
    readonly side: DiffSide;
    readonly line: number;
  } | null>(null);

  // -- Derived --
  const currentFileDiff = useMemo(
    () => getFileDiff(fileDiffs, selectedFile),
    [fileDiffs, selectedFile],
  );

  // -- B4: full-file view is active only when the pair resolved and the
  // interdiff is not showing (the interdiff is exempt from full-file). --
  const fullFileActive =
    viewMode === "full" &&
    !interdiffActive &&
    fullPair !== null &&
    fullPair.full;

  // The file diff the editor + zones actually consume. In full-file mode this is
  // the complete file text with an identity line map, so the comment/zone code
  // path (fragmentToReal / realToFragment) works unchanged on real lines.
  const effectiveFileDiff = useMemo<FileDiff>(() => {
    if (fullFileActive && fullPair !== null) {
      return {
        path: selectedFile,
        original: fullPair.original,
        modified: fullPair.modified,
        lineMap: identityLineMap(fullPair.original, fullPair.modified),
      };
    }
    return currentFileDiff;
  }, [fullFileActive, fullPair, selectedFile, currentFileDiff]);

  // Keep the effective file diff reachable from the (mount-time) Monaco mouse
  // handlers, which capture nothing else from render scope. Used to map a
  // clicked fragment line to its real file line for the comment anchor (D1).
  const currentFileDiffRef = useRef(effectiveFileDiff);
  useEffect(() => {
    currentFileDiffRef.current = effectiveFileDiff;
  }, [effectiveFileDiff]);

  const commentsForFile = useMemo(
    () => fileComments(review.comments, selectedFile),
    [review.comments, selectedFile],
  );

  // -- B2: advisory findings for the current file, minus dismissed ones. --
  const findingsForFile = useMemo(
    () =>
      review.review_findings.filter(
        (f) => f.path === selectedFile && !dismissedFindings.has(f.id),
      ),
    [review.review_findings, selectedFile, dismissedFindings],
  );

  const hasLocalComments = useMemo(
    () => review.comments.some((c) => isLocalOrigin(c.origin)),
    [review.comments],
  );

  const canRequestChanges =
    review.gate_state === "InReview" && review.comments.length > 0;

  const canAddComments = review.gate_state === "InReview";

  // Only open the input form when the review is InReview
  const effectiveActiveComment = canAddComments ? activeComment : null;

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

  // -- D4: PR intent header + collapsible disclosure --
  const headerTitle = review.title.trim() !== "" ? review.title : review.branch;
  const showIntent = review.title.trim() !== "" || review.body.trim() !== "";
  const intentOpen = intentOpenByPr[review.pr] ?? false;
  const toggleIntent = useCallback(() => {
    setIntentOpenByPr((prev) => ({
      ...prev,
      [review.pr]: !(prev[review.pr] ?? false),
    }));
  }, [review.pr]);

  // -- D1: pair each dispatched request to the interdiff region that answers it.
  // Paired against the interdiff specifically (not the toggled diff source) so
  // the checklist + the approve warning always reflect "changes since your
  // review". Advisory only — the pairing never blocks the gate (§9). --
  const interdiffFiles = useMemo(
    () => (interdiff !== null ? parseDiff(interdiff.raw) : []),
    [interdiff],
  );
  const pairings = useMemo(
    () =>
      review.dispatch_snapshot != null
        ? pairRequests(review.dispatch_snapshot, interdiffFiles)
        : [],
    [review.dispatch_snapshot, interdiffFiles],
  );
  const unmatchedCount = useMemo(
    () => pairings.filter((p) => p.match === null).length,
    [pairings],
  );
  const showAddressed = hasInterdiff && pairings.length > 0;

  // -- Close inline form on file change --
  useEffect(() => {
    setActiveComment(null);
  }, [selectedFile]);

  // -- D10: default a reworked review to its interdiff and fetch it. A fetch
  // failure falls back to the full diff (the store surfaces the error). --
  const reviewPr = review.pr;
  useEffect(() => {
    if (!hasInterdiff) {
      setInterdiff(null);
      setDiffSource("full");
      return;
    }
    setDiffSource("interdiff");
    let cancelled = false;
    void fetchInterdiff(reviewPr).then((data) => {
      if (cancelled) return;
      if (data === null) {
        // Fetch failed; keep the full diff visible.
        setDiffSource("full");
        return;
      }
      setInterdiff(data);
    });
    return () => {
      cancelled = true;
    };
  }, [reviewPr, hasInterdiff, fetchInterdiff]);

  // -- Keep the selected file valid when the file set changes (e.g. toggling
  // between the interdiff and the full diff). --
  useEffect(() => {
    if (filePaths.length === 0) return;
    if (!filePaths.includes(selectedFile)) {
      setSelectedFile(filePaths[0] ?? "");
    }
  }, [filePaths, selectedFile]);

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
      findingDecorRef.current = modified.createDecorationsCollection([]);

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

      // Click on gutter: toggle inline comment form. `activeComment` holds the
      // FRAGMENT (Monaco) line + side for zone placement; the real file line is
      // resolved when the comment is submitted (D1).
      modified.onMouseDown((e) => {
        const target = e.target;
        const isGutterClick =
          target.type === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN ||
          target.type === monaco.editor.MouseTargetType.GUTTER_LINE_NUMBERS;

        if (isGutterClick && target.position != null) {
          const line = target.position.lineNumber;
          // D1: only lines that map to a real file line are commentable. This
          // should always hold for a clickable gutter line; guard anyway.
          if (
            fragmentToReal(currentFileDiffRef.current, "New", line) === undefined
          ) {
            return;
          }
          setActiveComment((prev) =>
            prev !== null && prev.side === "New" && prev.line === line
              ? null
              : { side: "New", line },
          );
        }
      });

      // -- Original (left/old-side) editor: mirror the hover + click gutter so
      // reviewers can comment on removed / pre-change lines (D12). Old-side
      // comments render in this editor; New-side stay in the modified editor. --
      const original = editor.getOriginalEditor();
      original.updateOptions({ glyphMargin: true });

      originalGlyphDecorRef.current = original.createDecorationsCollection([]);
      originalCommentDecorRef.current = original.createDecorationsCollection([]);
      originalFindingDecorRef.current =
        original.createDecorationsCollection([]);

      original.onMouseMove((e) => {
        if (originalGlyphDecorRef.current == null) return;
        const target = e.target;
        const isHoverArea =
          target.type === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN ||
          target.type === monaco.editor.MouseTargetType.GUTTER_LINE_NUMBERS ||
          target.type ===
            monaco.editor.MouseTargetType.GUTTER_LINE_DECORATIONS ||
          target.type === monaco.editor.MouseTargetType.CONTENT_TEXT;

        if (isHoverArea && target.position != null) {
          const ln = target.position.lineNumber;
          originalGlyphDecorRef.current.set([
            {
              range: new monaco.Range(ln, 1, ln, 1),
              options: { glyphMarginClassName: "inline-comment-glyph" },
            },
          ]);
        } else {
          originalGlyphDecorRef.current.clear();
        }
      });

      original.onMouseDown((e) => {
        const target = e.target;
        const isGutterClick =
          target.type === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN ||
          target.type === monaco.editor.MouseTargetType.GUTTER_LINE_NUMBERS;

        if (isGutterClick && target.position != null) {
          const line = target.position.lineNumber;
          if (
            fragmentToReal(currentFileDiffRef.current, "Old", line) === undefined
          ) {
            return;
          }
          setActiveComment((prev) =>
            prev !== null && prev.side === "Old" && prev.line === line
              ? null
              : { side: "Old", line },
          );
        }
      });

      setEditorReady(true);
    },
    [editorTheme],
  );

  // -- Sync view zones with comments + active input line --
  // useEffect (not useLayoutEffect) so zones are created AFTER Monaco's
  // internal useEffect updates the editor models on file switch. Both sides are
  // synced: New-side comments live in the modified editor, Old-side comments in
  // the original editor (D12).
  useEffect(() => {
    if (
      !editorReady ||
      diffEditorRef.current == null ||
      monacoRef.current == null
    ) {
      return;
    }

    const monaco = monacoRef.current;

    // Build (and place) the view zones for one diff side in its editor,
    // returning the portal entries and the DOM-node cache keys it used. Anchors
    // store REAL file lines (D1), so each is translated back to the current
    // fragment; comments whose real line is not present in the fragment (e.g.
    // after a diff refresh, or an interdiff that no longer touches that line)
    // are skipped gracefully.
    const syncSide = (
      ed: MonacoEditorNs.IStandaloneCodeEditor,
      side: DiffSide,
      zoneIds: { current: string[] },
      commentDecor: MonacoEditorNs.IEditorDecorationsCollection | null,
    ): { readonly portals: PortalEntry[]; readonly usedKeys: Set<string> } => {
      // Re-apply glyph margin after mode changes.
      ed.updateOptions({ glyphMargin: true });

      const commentsByLine = new Map<number, Comment[]>();
      for (const c of commentsForFile) {
        if (anchorSide(c.anchor) !== side) continue;
        const range = anchorRange(c.anchor);
        if (range === null) continue;
        const fragLine = realToFragment(effectiveFileDiff, side, range[1]);
        if (fragLine === undefined) continue;
        const arr = commentsByLine.get(fragLine) ?? [];
        arr.push(c);
        commentsByLine.set(fragLine, arr);
      }

      const inputLine =
        effectiveActiveComment?.side === side
          ? effectiveActiveComment.line
          : null;

      const zoneLines = new Set<number>(commentsByLine.keys());
      if (inputLine != null) {
        zoneLines.add(inputLine);
      }

      const portals: PortalEntry[] = [];
      const usedKeys = new Set<string>();

      ed.changeViewZones((accessor) => {
        for (const id of zoneIds.current) {
          accessor.removeZone(id);
        }
        zoneIds.current = [];

        for (const line of Array.from(zoneLines).sort((a, b) => a - b)) {
          const comments = commentsByLine.get(line) ?? [];
          const hasInput = line === inputLine;

          const commentHeight = comments.length * 52;
          const inputHeight = hasInput ? 130 : 0;
          const padding = comments.length > 0 || hasInput ? 8 : 0;
          const totalHeight = commentHeight + inputHeight + padding;

          if (totalHeight === 0) continue;

          // Key by side so the two editors never collide in the DOM cache.
          const key = `zone-${side}-${String(line)}`;
          usedKeys.add(key);

          // Reuse DOM nodes so React portals keep component state.
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

          // Display + anchor use the real file line; placement used the fragment.
          const realLine =
            fragmentToReal(effectiveFileDiff, side, line) ?? line;
          zoneIds.current.push(zoneId);
          portals.push({
            key,
            domNode,
            lineNumber: realLine,
            side,
            comments,
            hasInput,
          });
        }
      });

      // Highlight lines that have comments on this side.
      if (commentDecor != null) {
        commentDecor.set(
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

      return { portals, usedKeys };
    };

    const modified = diffEditorRef.current.getModifiedEditor();
    const original = diffEditorRef.current.getOriginalEditor();

    const newResult = syncSide(
      modified,
      "New",
      zoneIdsRef,
      commentDecorRef.current,
    );
    const oldResult = syncSide(
      original,
      "Old",
      originalZoneIdsRef,
      originalCommentDecorRef.current,
    );

    const newPortals = [...newResult.portals, ...oldResult.portals];

    // Purge stale cached DOM nodes across both sides.
    const usedKeys = new Set<string>([
      ...newResult.usedKeys,
      ...oldResult.usedKeys,
    ]);
    for (const cachedKey of domNodeCacheRef.current.keys()) {
      if (!usedKeys.has(cachedKey)) {
        domNodeCacheRef.current.delete(cachedKey);
      }
    }

    setPortals(newPortals);
  }, [
    editorReady,
    commentsForFile,
    effectiveFileDiff,
    effectiveActiveComment,
    selectedFile,
    diffMode,
  ]);

  // -- B2: sync advisory finding pins as their OWN view zones + a dashed
  // severity rail, kept entirely separate from the comment machinery. Finding
  // zones use dedicated zone-id arrays, DOM-node cache, and decoration
  // collections keyed by finding id, so they never collide with a human comment
  // on the same line — a finding and a comment on one line simply stack. --
  useEffect(() => {
    if (
      !editorReady ||
      diffEditorRef.current == null ||
      monacoRef.current == null
    ) {
      return;
    }
    const monaco = monacoRef.current;

    const syncSide = (
      ed: MonacoEditorNs.IStandaloneCodeEditor,
      side: DiffSide,
      zoneIds: { current: string[] },
      decor: MonacoEditorNs.IEditorDecorationsCollection | null,
    ): {
      readonly portals: FindingPortalEntry[];
      readonly usedKeys: Set<string>;
    } => {
      const portals: FindingPortalEntry[] = [];
      const usedKeys = new Set<string>();
      const decorations: MonacoEditorNs.IModelDeltaDecoration[] = [];

      ed.changeViewZones((accessor) => {
        for (const id of zoneIds.current) {
          accessor.removeZone(id);
        }
        zoneIds.current = [];

        for (const finding of findingsForFile) {
          if (finding.side !== side) continue;
          const fragLine = realToFragment(
            effectiveFileDiff,
            side,
            finding.range[1],
          );
          if (fragLine === undefined) continue;

          const key = `finding-${finding.id}`;
          usedKeys.add(key);
          let domNode = findingNodeCacheRef.current.get(key);
          if (domNode == null) {
            domNode = document.createElement("div");
            domNode.style.zIndex = "10";
            findingNodeCacheRef.current.set(key, domNode);
          }

          const zoneId = accessor.addZone({
            afterLineNumber: fragLine,
            heightInPx: findingZoneHeight(finding),
            domNode,
            suppressMouseDown: false,
          });
          zoneIds.current.push(zoneId);
          portals.push({ key, domNode, finding });

          decorations.push({
            range: new monaco.Range(fragLine, 1, fragLine, 1),
            options: {
              isWholeLine: true,
              glyphMarginClassName: severityMeta(finding.severity).glyph,
            },
          });
        }
      });

      decor?.set(decorations);
      return { portals, usedKeys };
    };

    const modified = diffEditorRef.current.getModifiedEditor();
    const original = diffEditorRef.current.getOriginalEditor();
    modified.updateOptions({ glyphMargin: true });
    original.updateOptions({ glyphMargin: true });

    const newResult = syncSide(
      modified,
      "New",
      findingZoneIdsRef,
      findingDecorRef.current,
    );
    const oldResult = syncSide(
      original,
      "Old",
      originalFindingZoneIdsRef,
      originalFindingDecorRef.current,
    );

    const usedKeys = new Set<string>([
      ...newResult.usedKeys,
      ...oldResult.usedKeys,
    ]);
    for (const cachedKey of findingNodeCacheRef.current.keys()) {
      if (!usedKeys.has(cachedKey)) {
        findingNodeCacheRef.current.delete(cachedKey);
      }
    }

    setFindingPortals([...newResult.portals, ...oldResult.portals]);
  }, [editorReady, findingsForFile, effectiveFileDiff, diffMode]);

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
    async (lineNumber: number, body: string, side: DiffSide) => {
      setSubmitting(true);
      setCommentError(null);
      try {
        await onAddComment(selectedFile, lineNumber, lineNumber, body, side);
        setActiveComment(null);
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

    // Raw checks (for the evidence strip's failing-job name). Best-effort.
    void listCiChecks(prRef).then((checks) => {
      if (!cancelled) setCiChecks(checks);
    });

    // Live updates: the backend pushes the full checks list via `ci-updated`.
    const unlisten = listen<unknown>("ci-updated", (event) => {
      const update = parseCiUpdate(event.payload);
      if (update === null || update.pr !== prRef) return;
      setCiSummary(summarizeChecks(update.checks));
      setCiChecks(update.checks);
    });

    return () => {
      cancelled = true;
      void unlisten.then((f) => {
        f();
      });
    };
  }, [prRef, listCiChecks]);

  // -- B1/B3: fetch the evidence bundle on entry and on each head change. --
  const reviewHead = review.head_sha;
  useEffect(() => {
    let cancelled = false;
    void fetchEvidence(reviewPr).then((ev) => {
      if (!cancelled) setEvidence(ev);
    });
    return () => {
      cancelled = true;
    };
  }, [reviewPr, reviewHead, fetchEvidence]);

  // -- B4: resolve the full-file pair when Full-file mode is active. The pair is
  // reset on file/mode switch so stale content never flashes; the store memoizes
  // by `pr:path:head` so re-entry (and a return to the same file) is cheap. --
  useEffect(() => {
    if (viewMode !== "full" || interdiffActive || selectedFile === "") {
      setFullPair(null);
      return;
    }
    let cancelled = false;
    setFullPair(null);
    void fetchFilePair(reviewPr, selectedFile).then((pair) => {
      if (!cancelled) setFullPair(pair);
    });
    return () => {
      cancelled = true;
    };
  }, [
    viewMode,
    interdiffActive,
    selectedFile,
    reviewPr,
    reviewHead,
    fetchFilePair,
  ]);

  // -- B3: apply a pending jump-to-line once the target file's editor + line
  // map are ready (from evidence-strip weakening chips). --
  useEffect(() => {
    const jump = pendingJumpRef.current;
    if (jump === null) return;
    if (jump.path !== selectedFile) return;
    if (!editorReady || diffEditorRef.current === null) return;
    const frag = realToFragment(effectiveFileDiff, jump.side, jump.line);
    pendingJumpRef.current = null;
    if (frag === undefined) return;
    const ed =
      jump.side === "New"
        ? diffEditorRef.current.getModifiedEditor()
        : diffEditorRef.current.getOriginalEditor();
    ed.revealLineInCenter(frag);
    ed.setPosition({ lineNumber: frag, column: 1 });
  }, [selectedFile, effectiveFileDiff, editorReady, jumpNonce]);

  const handleJumpTo = useCallback(
    (path: string, side: DiffSide, line: number) => {
      pendingJumpRef.current = { path, side, line };
      setSelectedFile(path);
      setJumpNonce((n) => n + 1);
    },
    [],
  );

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

  // -- D2: approve is a local gate transition (InReview -> Approved); no
  // confirm, since it publishes nothing. --
  const handleApprove = useCallback(async () => {
    setApproving(true);
    try {
      await approveReview(review.pr);
    } finally {
      setApproving(false);
    }
  }, [approveReview, review.pr]);

  // -- B2: run the advisory read-only pre-pass reviewer. Advisory only — never
  // touches the gate; refuses while another agent is attached (enforced core-
  // side too). The header's "Agent working" line keys off review.agent. --
  const handlePreReview = useCallback(async () => {
    setPreReviewing(true);
    try {
      await preReview(review.pr);
    } finally {
      setPreReviewing(false);
    }
  }, [preReview, review.pr]);

  // -- D2: merge is a guarded, confirmed side effect (Invariant 5 / §9). --
  const handleMerge = useCallback(async () => {
    const confirmed = window.confirm(
      `Squash-merge ${review.pr} on GitHub and delete the branch?`,
    );
    if (!confirmed) return;
    setMerging(true);
    try {
      await mergeReview(review.pr);
    } finally {
      setMerging(false);
    }
  }, [mergeReview, review.pr]);

  // -- D9: the inline Local comments carried by a submitted GitHub review. --
  const localCommentCount = useMemo(
    () => review.comments.filter((c) => isLocalOrigin(c.origin)).length,
    [review.comments],
  );

  const verdictLabel = useCallback((event: ReviewEvent): string => {
    switch (event) {
      case "Approve":
        return "Approve";
      case "RequestChanges":
        return "Request changes";
      case "Comment":
        return "Comment";
      default:
        return assertNever(event);
    }
  }, []);

  // -- D9: submit a real GitHub PR review (guarded, confirmed side effect;
  // Invariant 5 / §9). Returns whether the review was posted so the control can
  // close its popover. --
  const handleGithubSubmit = useCallback(
    async (event: ReviewEvent, body: string): Promise<boolean> => {
      const confirmed = window.confirm(
        `Post review (${verdictLabel(event)}, ${String(localCommentCount)} line comment${
          localCommentCount === 1 ? "" : "s"
        }) to GitHub?`,
      );
      if (!confirmed) return false;
      setGithubSubmitting(true);
      setGithubSubmitResult(null);
      try {
        const result = await submitGithubReview(review.pr, event, body);
        if (result !== null) {
          setGithubSubmitResult(result);
          return true;
        }
        return false;
      } finally {
        setGithubSubmitting(false);
      }
    },
    [submitGithubReview, review.pr, localCommentCount, verdictLabel],
  );

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
              <span
                className="truncate font-display text-sm font-semibold text-foreground"
                title={headerTitle}
              >
                {headerTitle}
              </span>
              <GatePill state={review.gate_state} />
              {review.stale && (
                <span className="inline-flex shrink-0 items-center gap-1 text-xs text-danger">
                  <AlertTriangle className="h-3 w-3" /> Stale
                </span>
              )}
            </div>

            {/* Refs line: mono, faint; branch + PR / issue / repo links. */}
            <div className="flex min-w-0 flex-wrap items-center gap-x-2.5 gap-y-0.5 font-mono text-xs text-muted-foreground">
              <span
                className="inline-flex min-w-0 shrink items-center gap-1"
                title={review.branch}
              >
                <GitBranch className="h-3 w-3 shrink-0" />
                <span className="truncate">{review.branch}</span>
              </span>

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

          {/* Hunks / Full-file toggle (B4). The interdiff is exempt, so the
              toggle hides while the interdiff is the active view. */}
          {!interdiffActive && (
            <div className="flex overflow-hidden rounded-md border border-border">
              <button
                type="button"
                onClick={() => {
                  setViewMode("hunks");
                }}
                aria-pressed={viewMode === "hunks"}
                className={cn(
                  "cursor-pointer border-none px-3 py-1 text-xs font-medium transition-colors",
                  viewMode === "hunks"
                    ? "bg-accent text-accent-foreground"
                    : "bg-transparent text-muted-foreground hover:bg-accent/50",
                )}
                title="Show only the changed hunks"
              >
                Hunks
              </button>
              <button
                type="button"
                onClick={() => {
                  setViewMode("full");
                }}
                aria-pressed={viewMode === "full"}
                className={cn(
                  "cursor-pointer border-none px-3 py-1 text-xs font-medium transition-colors",
                  viewMode === "full"
                    ? "bg-accent text-accent-foreground"
                    : "bg-transparent text-muted-foreground hover:bg-accent/50",
                )}
                title="Show the full file with changes in context"
              >
                Full file
              </button>
            </div>
          )}

          {/* Diff-source toggle: interdiff vs full diff (D10). */}
          {hasInterdiff && (
            <div className="flex overflow-hidden rounded-md border border-border">
              <button
                type="button"
                onClick={() => {
                  setDiffSource("interdiff");
                }}
                aria-pressed={diffSource === "interdiff"}
                disabled={interdiff === null}
                className={cn(
                  "cursor-pointer border-none px-3 py-1 text-xs font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50",
                  diffSource === "interdiff"
                    ? "bg-accent text-accent-foreground"
                    : "bg-transparent text-muted-foreground hover:bg-accent/50",
                )}
                title="Show only what changed since your last review"
              >
                Since your review
              </button>
              <button
                type="button"
                onClick={() => {
                  setDiffSource("full");
                }}
                aria-pressed={diffSource === "full"}
                className={cn(
                  "cursor-pointer border-none px-3 py-1 text-xs font-medium transition-colors",
                  diffSource === "full"
                    ? "bg-accent text-accent-foreground"
                    : "bg-transparent text-muted-foreground hover:bg-accent/50",
                )}
                title="Show the full PR diff"
              >
                Full diff
              </button>
            </div>
          )}

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
            {/* Jump to the canonical Agent tab (activity timeline). */}
            <Button
              variant="ghost"
              size="sm"
              onClick={onOpenAgent}
              title="Open the agent activity timeline"
            >
              <Bot className="h-3.5 w-3.5" />
              Agent
            </Button>

            {/* Pre-review (B2) — advisory read-only pre-pass; any source while
                InReview. Disabled while an agent is attached (the header's
                "Agent working" line reflects the running Review agent). */}
            {review.gate_state === "InReview" && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => void handlePreReview()}
                disabled={preReviewing || review.agent != null}
                title="Run the advisory read-only pre-pass reviewer"
              >
                <BotMessageSquare className="h-3.5 w-3.5" />
                Pre-review
              </Button>
            )}

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
                onClick={() => {
                  // Outward publish — explicit confirmation per SPEC §12.
                  if (
                    window.confirm(
                      `Post ${String(localCommentCount)} comment${
                        localCommentCount === 1 ? "" : "s"
                      } to the GitHub PR thread?`,
                    )
                  ) {
                    void handleMirrorComments();
                  }
                }}
                disabled={mirroring}
                title="Mirror local comments to the GitHub PR thread"
              >
                <Upload className="h-3.5 w-3.5" />
                {mirroring ? "Mirroring..." : "Mirror"}
              </Button>
            )}
          </div>

          {/* Primary action — one clear call to action by state. */}
          {review.source === "ReviewRequested" ? (
            // D9: review-requested PRs publish a real GitHub review.
            review.gate_state !== "Merged" && (
              <SubmitReviewControl
                commentCount={localCommentCount}
                pending={githubSubmitting}
                onSubmit={handleGithubSubmit}
              />
            )
          ) : (
            // D2: authored reviews close the loop locally — request changes /
            // approve while InReview, merge once Approved, read-only when Merged.
            <>
              {review.gate_state === "InReview" && (
                <>
                  {/* D1: informational nudge only — some dispatched requests have
                      no matching interdiff change. Never blocks Approve and never
                      adds a confirm; approve authority is unchanged (§9). Shown
                      only once the interdiff is resolved so it can't false-alarm
                      before the pairing data exists. */}
                  {interdiff !== null && unmatchedCount > 0 && (
                    <span
                      className="inline-flex shrink-0 items-center gap-1.5 text-xs text-warning"
                      title="Some requests you dispatched have no detected change in the interdiff — check them before approving."
                    >
                      <span
                        className="h-1.5 w-1.5 shrink-0 rounded-full bg-warning"
                        aria-hidden="true"
                      />
                      {String(unmatchedCount)} request
                      {unmatchedCount === 1 ? "" : "s"} without a matching change
                      — check before approving
                    </span>
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
                  <Button
                    size="sm"
                    onClick={() => void handleApprove()}
                    disabled={approving}
                  >
                    <Check className="h-3.5 w-3.5" />
                    {approving ? "Approving…" : "Approve"}
                  </Button>
                </>
              )}

              {review.gate_state === "Approved" && (
                <Button
                  size="sm"
                  className="bg-success text-white hover:bg-success/90"
                  onClick={() => void handleMerge()}
                  disabled={merging}
                >
                  <GitMerge className="h-3.5 w-3.5" />
                  {merging ? "Merging…" : "Merge PR"}
                </Button>
              )}

              {review.gate_state === "Merged" && (
                <span className="inline-flex items-center gap-1.5 rounded-md border border-success/30 bg-success/15 px-2.5 py-1 text-xs font-medium text-success">
                  <CheckCircle2 className="h-3.5 w-3.5" />
                  Merged
                </span>
              )}
            </>
          )}
        </div>
      </header>

      {/* ----------------------------------------------------------------- */}
      {/* Intent disclosure (D4)                                            */}
      {/* ----------------------------------------------------------------- */}
      {showIntent && (
        <IntentPanel
          body={review.body}
          issue={review.issue}
          issueHref={issueHref}
          open={intentOpen}
          onToggle={toggleIntent}
          onOpenIssue={(href) => {
            void openExternal(href);
          }}
        />
      )}

      {/* ----------------------------------------------------------------- */}
      {/* Evidence strip — deterministic review signals (B3)                */}
      {/* ----------------------------------------------------------------- */}
      <EvidenceStrip
        evidence={evidence}
        ciChecks={ciChecks}
        onJumpTo={handleJumpTo}
      />

      {/* ----------------------------------------------------------------- */}
      {/* Addressed requests — read-only interdiff history (D10)            */}
      {/* ----------------------------------------------------------------- */}
      {showAddressed && (
        <AddressedRequests
          pairings={pairings}
          open={addressedOpen}
          onToggle={() => {
            setAddressedOpen((prev) => !prev);
          }}
          onJumpTo={handleJumpTo}
        />
      )}

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
      {/* GitHub review submission result (D9)                              */}
      {/* ----------------------------------------------------------------- */}
      {githubSubmitResult !== null && (
        <div className="flex items-center justify-between border-b border-success/40 bg-success/15 px-4 py-2 text-xs text-success">
          <span>
            Submitted review with {String(githubSubmitResult.submitted)} line
            comment{githubSubmitResult.submitted === 1 ? "" : "s"} to GitHub
            {githubSubmitResult.skipped.length > 0 &&
              ` · ${String(githubSubmitResult.skipped.length)} skipped`}
          </span>
          <button
            type="button"
            onClick={() => {
              setGithubSubmitResult(null);
            }}
            className="cursor-pointer border-none bg-transparent text-success underline hover:no-underline"
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
          {/* B4: full-file was requested but could not be resolved — fall back
              to the changed hunks with a subtle note. */}
          {viewMode === "full" &&
            !interdiffActive &&
            fullPair !== null &&
            !fullPair.full && (
              <div className="border-b border-border bg-muted/40 px-4 py-1 text-[10px] text-muted-foreground">
                Full file unavailable — showing changed hunks
              </div>
            )}
          {selectedFile !== "" ? (
            <DiffEditor
              original={effectiveFileDiff.original}
              modified={effectiveFileDiff.modified}
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
                side={entry.side}
                hasInput={entry.hasInput}
                onSubmit={(body) => {
                  void handleInlineSubmit(entry.lineNumber, body, entry.side);
                }}
                onCancel={() => {
                  setActiveComment(null);
                }}
              />,
              entry.domNode,
              entry.key,
            ),
          )}

          {/* Advisory finding pins (B2) — rendered into their own view zones. */}
          {findingPortals.map((entry) =>
            createPortal(
              <FindingPin
                finding={entry.finding}
                onDismiss={() => {
                  setDismissedFindings((prev) => {
                    const next = new Set(prev);
                    next.add(entry.finding.id);
                    return next;
                  });
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
    </div>
  );
}
