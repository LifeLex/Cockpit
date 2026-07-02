/**
 * Pre-review findings band — a collapsible triage list of every advisory
 * finding the read-only pre-pass reviewer produced for this review.
 *
 * It lives in the diff band stack directly below the {@link EvidenceStrip} and
 * exists because the Monaco finding pins only appear once the reviewer selects
 * the finding's file — findings on unselected files (and file-level findings
 * whose lines never map into a visible hunk) were otherwise invisible. This
 * panel makes every finding reachable from one place regardless of the selected
 * file.
 *
 * Findings are advisory: a triage layer, never a verdict (see
 * `RESEARCH_INTERACTION_PATTERNS.md` §3). They never advance or block the gate,
 * no confidence is displayed, and dismissing one here shares the same set the
 * Monaco pins use so it vanishes from both surfaces at once.
 */

import { useMemo, useState } from "react";
import { ChevronDown, ChevronRight, ScanSearch, X } from "lucide-react";
import type { ReviewFinding } from "../bindings/ReviewFinding";
import type { FindingSeverity } from "../bindings/FindingSeverity";
import type { DiffSide } from "../bindings/DiffSide";
import { cn } from "@/lib/utils";

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

interface FindingsPanelProps {
  /** Every finding for the review; dismissed ones are filtered out here. */
  readonly findings: readonly ReviewFinding[];
  /** Finding ids the reviewer dismissed (shared with the Monaco pins). */
  readonly dismissed: ReadonlySet<string>;
  /** Whether the panel body is expanded (owned by the enclosing DiffView). */
  readonly open: boolean;
  /** Toggle the expanded state. */
  readonly onToggle: () => void;
  /** Dismiss a finding by id (removes it from the panel and the pins). */
  readonly onDismiss: (id: string) => void;
  /** Scroll the diff to a finding's line, side-aware. */
  readonly onJumpTo: (path: string, side: DiffSide, line: number) => void;
}

// ---------------------------------------------------------------------------
// Helpers (exported for reuse by DiffView + the evidence-strip chip)
// ---------------------------------------------------------------------------

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** Per-severity counts of the non-dismissed findings. */
export interface FindingsBreakdown {
  readonly info: number;
  readonly warning: number;
  readonly critical: number;
}

/** Count non-dismissed findings per severity class. */
export function findingsBreakdown(
  findings: readonly ReviewFinding[],
  dismissed: ReadonlySet<string>,
): FindingsBreakdown {
  let info = 0;
  let warning = 0;
  let critical = 0;
  for (const f of findings) {
    if (dismissed.has(f.id)) continue;
    switch (f.severity) {
      case "Info":
        info += 1;
        break;
      case "Warning":
        warning += 1;
        break;
      case "Critical":
        critical += 1;
        break;
      default:
        return assertNever(f.severity);
    }
  }
  return { info, warning, critical };
}

/**
 * Whether the panel should be expanded by default: true only when at least one
 * non-dismissed Critical finding exists. Critical findings are the ones worth
 * interrupting the reviewer for; everything else stays a visible-but-collapsed
 * header they can open at will.
 */
export function findingsAutoExpand(
  findings: readonly ReviewFinding[],
  dismissed: ReadonlySet<string>,
): boolean {
  return findingsBreakdown(findings, dismissed).critical > 0;
}

interface SeverityMeta {
  readonly label: string;
  readonly dot: string;
  readonly text: string;
  /** Sort rank: Critical outranks Warning outranks Info. */
  readonly rank: number;
}

function severityMeta(severity: FindingSeverity): SeverityMeta {
  switch (severity) {
    case "Info":
      return {
        label: "Info",
        dot: "bg-muted-foreground",
        text: "text-muted-foreground",
        rank: 1,
      };
    case "Warning":
      return {
        label: "Warning",
        dot: "bg-warning",
        text: "text-warning",
        rank: 2,
      };
    case "Critical":
      return {
        label: "Critical",
        dot: "bg-danger",
        text: "text-danger",
        rank: 3,
      };
    default:
      return assertNever(severity);
  }
}

/** `path:start–end` (or just `path` for a file-level finding at line 0). */
function locationLabel(finding: ReviewFinding): string {
  const [start, end] = finding.range;
  if (start <= 0) return finding.path;
  return start === end
    ? `${finding.path}:${String(start)}`
    : `${finding.path}:${String(start)}–${String(end)}`;
}

/** Render the non-zero severity classes as `1 critical · 2 warning · 1 info`. */
function breakdownLabel(breakdown: FindingsBreakdown): string {
  return [
    breakdown.critical > 0 ? `${String(breakdown.critical)} critical` : null,
    breakdown.warning > 0 ? `${String(breakdown.warning)} warning` : null,
    breakdown.info > 0 ? `${String(breakdown.info)} info` : null,
  ]
    .filter((s): s is string => s !== null)
    .join(" · ");
}

// ---------------------------------------------------------------------------
// FindingRow
// ---------------------------------------------------------------------------

function FindingRow({
  finding,
  onDismiss,
  onJumpTo,
}: {
  readonly finding: ReviewFinding;
  readonly onDismiss: (id: string) => void;
  readonly onJumpTo: (path: string, side: DiffSide, line: number) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const meta = severityMeta(finding.severity);
  const [start] = finding.range;
  const hasLine = start > 0;
  // A short single-line rationale never needs the expand toggle.
  const clampable =
    finding.rationale.length > 100 || finding.rationale.includes("\n");

  return (
    <li className="rounded-md border border-border bg-background/40 px-2.5 py-1.5">
      <div className="flex items-center gap-2">
        <span className="inline-flex shrink-0 items-center gap-1.5">
          <span
            className={cn("h-2 w-2 shrink-0 rounded-full", meta.dot)}
            aria-hidden="true"
          />
          <span
            className={cn(
              "text-[10px] font-semibold uppercase tracking-wide",
              meta.text,
            )}
          >
            {meta.label}
          </span>
        </span>
        <span
          className="min-w-0 flex-1 truncate font-mono text-[10px] text-muted-foreground"
          title={locationLabel(finding)}
        >
          {locationLabel(finding)}
        </span>
        {hasLine ? (
          <button
            type="button"
            onClick={() => {
              onJumpTo(finding.path, finding.side, start);
            }}
            className="shrink-0 cursor-pointer rounded border border-border bg-transparent px-1.5 py-0.5 text-[10px] text-muted-foreground hover:border-primary/50 hover:text-foreground"
            title={`Jump to ${locationLabel(finding)}`}
          >
            Jump
          </button>
        ) : (
          <span
            className="shrink-0 rounded border border-border/60 px-1.5 py-0.5 text-[10px] text-muted-foreground/70"
            title="This finding is file-level and has no specific line"
          >
            file-level
          </span>
        )}
        <button
          type="button"
          onClick={() => {
            onDismiss(finding.id);
          }}
          aria-label="Dismiss finding"
          title="Dismiss finding"
          className="shrink-0 cursor-pointer rounded border-none bg-transparent p-0.5 text-muted-foreground hover:text-foreground"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>
      <div className="mt-1 font-medium text-foreground">{finding.title}</div>
      <div
        className={cn(
          "mt-0.5 whitespace-pre-wrap leading-relaxed text-muted-foreground",
          clampable && !expanded && "line-clamp-2",
        )}
      >
        {finding.rationale}
      </div>
      {clampable && (
        <button
          type="button"
          onClick={() => {
            setExpanded((prev) => !prev);
          }}
          className="mt-0.5 cursor-pointer border-none bg-transparent p-0 text-[10px] text-primary hover:underline"
        >
          {expanded ? "Show less" : "Show more"}
        </button>
      )}
    </li>
  );
}

// ---------------------------------------------------------------------------
// FindingsPanel
// ---------------------------------------------------------------------------

/**
 * A collapsible band listing every non-dismissed advisory finding for the
 * review, sorted highest-severity first. Renders nothing when no findings
 * remain (all dismissed, or none produced).
 */
export function FindingsPanel({
  findings,
  dismissed,
  open,
  onToggle,
  onDismiss,
  onJumpTo,
}: FindingsPanelProps) {
  const visible = useMemo(
    () =>
      findings
        .filter((f) => !dismissed.has(f.id))
        .slice()
        .sort(
          (a, b) => severityMeta(b.severity).rank - severityMeta(a.severity).rank,
        ),
    [findings, dismissed],
  );

  const breakdown = useMemo(
    () => findingsBreakdown(findings, dismissed),
    [findings, dismissed],
  );

  if (visible.length === 0) return null;

  return (
    <div className="shrink-0 border-b border-border bg-card/50">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        className="flex w-full cursor-pointer items-center gap-1.5 border-none bg-transparent px-4 py-1.5 text-[10px] font-semibold tracking-wide text-muted-foreground hover:text-foreground"
        title="Toggle pre-review findings"
      >
        {open ? (
          <ChevronDown className="h-3 w-3" />
        ) : (
          <ChevronRight className="h-3 w-3" />
        )}
        <ScanSearch className="h-3 w-3" />
        <span className="font-mono">
          PRE-REVIEW FINDINGS ({String(visible.length)})
        </span>
        <span className="font-mono normal-case tracking-normal text-muted-foreground/70">
          {breakdownLabel(breakdown)}
        </span>
        <span className="ml-1 font-sans font-normal normal-case tracking-normal text-muted-foreground/60">
          advisory — never blocks
        </span>
      </button>

      {open && (
        <ul className="space-y-1.5 px-4 pb-2.5 pl-9 text-xs">
          {visible.map((finding) => (
            <FindingRow
              key={finding.id}
              finding={finding}
              onDismiss={onDismiss}
              onJumpTo={onJumpTo}
            />
          ))}
        </ul>
      )}
    </div>
  );
}
