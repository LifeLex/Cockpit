/**
 * Evidence strip: one glanceable row of deterministic review signals rendered
 * above the diff (B3).
 *
 * Surfaces the [`EvidenceSummary`] the backend assembles per review — CI rollup,
 * test delta, diff size, risk paths, suspected test-weakening, and the commands
 * the agent ran — as compact chips. Each chip carries a status dot AND a text
 * label (never color alone) and uses tabular-nums mono for its telemetry.
 * Weakening chips are actionable: clicking one jumps the diff to the offending
 * hunk via {@link EvidenceStripProps.onJumpTo}.
 *
 * Renders nothing when `evidence` is null so a failed/absent evidence fetch
 * degrades gracefully.
 */

import type { EvidenceSummary } from "../bindings/EvidenceSummary";
import type { CiSummary } from "../bindings/CiSummary";
import type { CiCheck } from "../bindings/CiCheck";
import type { RiskFlag } from "../bindings/RiskFlag";
import type { SizeClass } from "../bindings/SizeClass";
import type { WeakeningKind } from "../bindings/WeakeningKind";
import type { DiffSide } from "../bindings/DiffSide";
import type { FindingsBreakdown } from "./FindingsPanel";
import { checkOutcome } from "@/lib/ci";
import { cn } from "@/lib/utils";
import { Gauge, ScanSearch, ShieldAlert, TriangleAlert } from "lucide-react";

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

interface EvidenceStripProps {
  /** The evidence bundle; when null the strip renders nothing. */
  readonly evidence: EvidenceSummary | null;
  /**
   * The raw CI checks DiffView already fetches, used only to name the failing
   * workflow/job on the CI chip. The x/y count comes from `evidence.ci`.
   */
  readonly ciChecks?: readonly CiCheck[];
  /**
   * Jump the diff to a line, side-aware. Wired for weakening chips so a
   * suspected weakening jumps straight to its hunk.
   */
  readonly onJumpTo?: (path: string, side: DiffSide, line: number) => void;
  /**
   * Per-severity counts of the non-dismissed advisory findings. When the total
   * is positive the strip shows a "N findings" chip toned by the highest
   * severity present; null or all-zero renders no chip.
   */
  readonly findingsCount?: FindingsBreakdown | null;
  /** Expand + scroll to the findings panel; wired to the findings chip. */
  readonly onShowFindings?: (() => void) | undefined;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** Semantic tone driving a chip's status-dot color. */
type ChipTone = "pass" | "fail" | "pending" | "warning" | "neutral";

function toneDotClass(tone: ChipTone): string {
  switch (tone) {
    case "pass":
      return "bg-success";
    case "fail":
      return "bg-danger";
    case "pending":
      return "bg-warning";
    case "warning":
      return "bg-warning";
    case "neutral":
      return "bg-muted-foreground";
    default:
      return assertNever(tone);
  }
}

/** Human label for a diff size bucket. */
function sizeLabel(size: SizeClass): string {
  switch (size) {
    case "S":
      return "S";
    case "M":
      return "M";
    case "L":
      return "L";
    case "Xl":
      return "XL";
    default:
      return assertNever(size);
  }
}

/** Larger diffs read as more caution-worthy. */
function sizeTone(size: SizeClass): ChipTone {
  switch (size) {
    case "S":
    case "M":
      return "neutral";
    case "L":
      return "warning";
    case "Xl":
      return "fail";
    default:
      return assertNever(size);
  }
}

/** Human label for a risk-path flag. */
function riskLabel(flag: RiskFlag): string {
  switch (flag) {
    case "Migration":
      return "Migration";
    case "Lockfile":
      return "Lockfile";
    case "CiConfig":
      return "CI config";
    case "Auth":
      return "Auth";
    case "GithubDir":
      return ".github";
    case "Dependency":
      return "Dependency";
    default:
      return assertNever(flag);
  }
}

/** Human label for a suspected test-weakening. */
function weakeningLabel(kind: WeakeningKind): string {
  switch (kind) {
    case "DeletedAssertion":
      return "Deleted assertion";
    case "IgnoreAdded":
      return "#[ignore] added";
    case "OrTrue":
      return "|| true added";
    case "SkipOrTodo":
      return "Skip / only";
    case "DeletedTestFn":
      return "Deleted test fn";
    case "DeletedTestFile":
      return "Deleted test file";
    case "SnapshotRewrite":
      return "Snapshot rewrite";
    default:
      return assertNever(kind);
  }
}

// ---------------------------------------------------------------------------
// Chip
// ---------------------------------------------------------------------------

function Chip({
  tone,
  title,
  onClick,
  children,
}: {
  readonly tone: ChipTone;
  readonly title?: string | undefined;
  readonly onClick?: (() => void) | undefined;
  readonly children: React.ReactNode;
}) {
  const dot = (
    <span
      className={cn("h-1.5 w-1.5 shrink-0 rounded-full", toneDotClass(tone))}
      aria-hidden="true"
    />
  );
  const base =
    "inline-flex shrink-0 items-center gap-1.5 rounded-md border border-border bg-muted/30 px-1.5 py-0.5 text-xs";
  if (onClick !== undefined) {
    return (
      <button
        type="button"
        onClick={onClick}
        title={title}
        className={cn(base, "cursor-pointer hover:bg-muted")}
      >
        {dot}
        {children}
      </button>
    );
  }
  return (
    <span title={title} className={base}>
      {dot}
      {children}
    </span>
  );
}

/** Overall CI tone from the rollup counts. */
function ciTone(ci: CiSummary): ChipTone {
  if (ci.failed > 0) return "fail";
  if (ci.pending > 0) return "pending";
  return "pass";
}

/** The name of the first failing check (workflow when set, else job name). */
function failingCheckName(checks: readonly CiCheck[]): string | null {
  const failing = checks.find((c) => checkOutcome(c) === "fail");
  if (failing === undefined) return null;
  return failing.workflow !== "" ? failing.workflow : failing.name;
}

/** Chip tone for the findings chip: the highest severity present drives it. */
function findingsTone(breakdown: FindingsBreakdown): ChipTone {
  if (breakdown.critical > 0) return "fail";
  if (breakdown.warning > 0) return "warning";
  return "neutral";
}

// ---------------------------------------------------------------------------
// EvidenceStrip
// ---------------------------------------------------------------------------

export function EvidenceStrip({
  evidence,
  ciChecks,
  onJumpTo,
  findingsCount,
  onShowFindings,
}: EvidenceStripProps) {
  if (evidence === null) return null;

  const { signals, ci, agent_ran: agentRan } = evidence;
  const { test_delta: testDelta } = signals;
  const showTests =
    testDelta.test_files_changed > 0 ||
    testDelta.assertions_added > 0 ||
    testDelta.assertions_removed > 0;
  const failingName = ci !== null && ciChecks ? failingCheckName(ciChecks) : null;

  const findingsTotal =
    findingsCount != null
      ? findingsCount.info + findingsCount.warning + findingsCount.critical
      : 0;

  return (
    <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-border bg-card/60 px-4 py-1.5">
      <span className="mr-0.5 inline-flex items-center gap-1 text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">
        <Gauge className="h-3 w-3" />
        Evidence
      </span>

      {/* Advisory pre-review findings — a triage jump-off, never a verdict. */}
      {findingsCount != null && findingsTotal > 0 && (
        <Chip
          tone={findingsTone(findingsCount)}
          title={`${String(findingsTotal)} pre-review finding${
            findingsTotal === 1 ? "" : "s"
          } — advisory`}
          onClick={onShowFindings}
        >
          <ScanSearch className="h-3 w-3" />
          <span className="text-foreground">
            {String(findingsTotal)} finding{findingsTotal === 1 ? "" : "s"}
          </span>
        </Chip>
      )}

      {/* CI rollup (x/y + failing workflow name when available). */}
      {ci !== null && ci.total > 0 && (
        <Chip
          tone={ciTone(ci)}
          title={`CI: ${String(ci.passed)} passed, ${String(ci.failed)} failed, ${String(ci.pending)} pending`}
        >
          <span className="text-muted-foreground">CI</span>
          <span className="font-mono tabular-nums text-foreground">
            {String(ci.passed)}/{String(ci.total)}
          </span>
          {failingName !== null && (
            <span className="max-w-[12rem] truncate text-danger">
              {failingName}
            </span>
          )}
        </Chip>
      )}

      {/* Diff size + file count. */}
      <Chip
        tone={sizeTone(signals.size_class)}
        title={`${String(signals.additions)} added, ${String(signals.deletions)} removed across ${String(signals.files_changed)} file(s)`}
      >
        <span className="font-semibold text-foreground">
          {sizeLabel(signals.size_class)}
        </span>
        <span className="font-mono tabular-nums text-muted-foreground">
          {String(signals.files_changed)} files
        </span>
      </Chip>

      {/* Test delta. */}
      {showTests && (
        <Chip
          tone={
            testDelta.assertions_removed > testDelta.assertions_added
              ? "warning"
              : "neutral"
          }
          title={`${String(testDelta.test_files_changed)} test file(s); ${String(testDelta.assertions_added)} assertions added, ${String(testDelta.assertions_removed)} removed`}
        >
          <span className="text-muted-foreground">Tests</span>
          <span className="font-mono tabular-nums">
            <span className="text-success">
              +{String(testDelta.assertions_added)}
            </span>
            <span className="text-muted-foreground">/</span>
            <span className="text-danger">
              −{String(testDelta.assertions_removed)}
            </span>
          </span>
        </Chip>
      )}

      {/* Risk paths — one chip per flag, path in the tooltip. */}
      {signals.risk_paths.map((risk, idx) => (
        <Chip
          key={`risk-${String(idx)}-${risk.path}`}
          tone={risk.flag === "Auth" ? "fail" : "warning"}
          title={risk.path}
        >
          <ShieldAlert className="h-3 w-3 text-warning" />
          <span className="text-foreground">{riskLabel(risk.flag)}</span>
        </Chip>
      ))}

      {/* Weakening flags — danger-toned, jump to the offending hunk on click. */}
      {signals.weakening.map((flag, idx) => (
        <Chip
          key={`weak-${String(idx)}-${flag.path}-${String(flag.line)}`}
          tone="fail"
          title={`${flag.path}:${String(flag.line)} — ${flag.excerpt}`}
          onClick={
            onJumpTo === undefined
              ? undefined
              : () => {
                  onJumpTo(flag.path, flag.side, flag.line);
                }
          }
        >
          <TriangleAlert className="h-3 w-3 text-danger" />
          <span className="text-foreground">{weakeningLabel(flag.kind)}</span>
        </Chip>
      ))}

      {/* Commands the agent ran (Phase D fills this; empty until then). */}
      {agentRan.map((run, idx) => (
        <Chip
          key={`cmd-${String(idx)}-${run.command}`}
          tone={run.ok ? "pass" : "fail"}
          title={run.command}
        >
          <span
            className={cn(
              "font-mono",
              run.ok ? "text-success" : "text-danger",
            )}
            aria-hidden="true"
          >
            {run.ok ? "✓" : "✗"}
          </span>
          <span className="max-w-[14rem] truncate font-mono text-muted-foreground">
            {run.command}
          </span>
        </Chip>
      ))}
    </div>
  );
}
