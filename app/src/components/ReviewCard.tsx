import { useMemo } from "react";
import { Button } from "@/components/ui/button";
import { Layers, ShieldAlert } from "lucide-react";
import { cn } from "@/lib/utils";
import type { Review } from "../bindings/Review";
import type { GateState } from "../bindings/GateState";
import type { SizeClass } from "../bindings/SizeClass";
import type { RiskFlag } from "../bindings/RiskFlag";
import type { CiSummary } from "../bindings/CiSummary";
import { cardSignal } from "../lib/card-signal";
import type { SignalTone } from "../lib/card-signal";
import { diffStats } from "../diff-parser";
import { ciState } from "../lib/ci";
import {
  diffTotals,
  sizeClass,
  sensitiveFlags,
  touchesTests,
} from "../lib/diff-signals";

/** Presentation density for the board. */
export type CardDensity = "cards" | "compact";

interface ReviewCardProps {
  readonly review: Review;
  readonly onAction: (pr: string) => void;
  /**
   * Restack a stale review onto its parent's new head. Wired only for stale
   * reviews; explicit user action operating on the review's own branch.
   */
  readonly onRestack: (pr: string) => void;
  /** Presentation density; defaults to the roomy card layout. */
  readonly density?: CardDensity;
  /**
   * Whether the card is rendered inside a stack container. When true the text
   * stack-relation hint is suppressed, since the container already shows the
   * relationship visually via its rail and indentation.
   */
  readonly inStack?: boolean;
}

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** Status-LED color for a gate state, using the `--color-state-*` tokens. */
function ledColorClass(state: GateState): string {
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

/** Text color for the reason line, keyed off its semantic tone. */
function toneTextClass(tone: SignalTone): string {
  switch (tone) {
    case "attention":
      return "text-state-in-review";
    case "running":
      return "text-state-dispatched";
    case "warning":
      return "text-warning";
    case "done":
      return "text-state-approved";
    case "neutral":
      return "text-muted-foreground";
    default:
      return assertNever(tone);
  }
}

/** Button variant type matching the Button component's variant prop. */
type ButtonVariant = "default" | "outline" | "ghost";

interface ActionConfig {
  readonly label: string;
  readonly variant: ButtonVariant;
  readonly muted: boolean;
}

/** Determines the action button label, variant, and muted state from the gate state. */
function actionConfigForState(state: GateState): ActionConfig {
  switch (state) {
    case "Pending":
      return { label: "Review", variant: "default", muted: false };
    case "InReview":
      return { label: "Review", variant: "default", muted: false };
    case "Dispatched":
      return { label: "Watch", variant: "outline", muted: true };
    case "Reworked":
      return { label: "Re-review", variant: "default", muted: false };
    case "Approved":
      return { label: "View", variant: "ghost", muted: false };
    case "Merged":
      return { label: "View", variant: "ghost", muted: false };
    default:
      return assertNever(state);
  }
}

function parsePrDisplay(pr: string): { repo: string; number: string } {
  const match = /github\.com\/([^/]+\/[^/]+)\/pull\/(\d+)/.exec(pr);
  if (match !== null) {
    const [, repo, num] = match;
    if (repo !== undefined && num !== undefined) {
      return { repo, number: num };
    }
  }
  return { repo: "", number: pr };
}

/**
 * Whether the LED should pulse. Only non-terminal running work (an actively
 * dispatched agent) pulses; everything else is steady.
 */
function ledPulses(review: Review): boolean {
  return review.gate_state === "Dispatched" && !review.stale;
}

/**
 * Whether the whole card is de-emphasized. Approved and stale reviews are
 * settled or blocked, so they dim to let attention items stand out.
 */
function isDimmed(review: Review): boolean {
  return review.stale || review.gate_state === "Approved";
}

/** The stack relationship line, or null when the review is not in a stack. */
function stackRelation(review: Review): string | null {
  const parts: string[] = [];
  if (review.parents.length > 0) {
    parts.push(
      review.parents.length === 1
        ? "on parent"
        : `on ${String(review.parents.length)} parents`,
    );
  }
  if (review.children.length > 0) {
    parts.push(`parent of ${String(review.children.length)}`);
  }
  return parts.length > 0 ? parts.join(" · ") : null;
}

/** LED dot; pulses for running work. */
function StatusLed({ review }: { readonly review: Review }) {
  return (
    <span className="relative flex h-2.5 w-2.5 shrink-0" aria-hidden="true">
      {ledPulses(review) && (
        <span
          className={cn(
            "absolute inline-flex h-full w-full animate-ping rounded-full opacity-60",
            ledColorClass(review.gate_state),
          )}
        />
      )}
      <span
        className={cn(
          "relative inline-flex h-2.5 w-2.5 rounded-full",
          ledColorClass(review.gate_state),
        )}
      />
    </span>
  );
}

/** Numeric telemetry chips (comments, diff +/-) shared by both densities. */
function TelemetryChips({ review }: { readonly review: Review }) {
  const { additions, deletions } = diffStats(review.diff.raw);
  const hasDiff = additions > 0 || deletions > 0;
  return (
    <>
      {review.comments.length > 0 && (
        <span className="inline-flex items-center gap-1 font-mono tabular-nums text-muted-foreground">
          <span className="text-state-in-review">{"●"}</span>
          {review.comments.length}
        </span>
      )}
      {hasDiff && (
        <span className="inline-flex items-center gap-1.5 font-mono tabular-nums">
          <span className="text-success">+{additions}</span>
          <span className="text-danger">
            {"−"}
            {deletions}
          </span>
        </span>
      )}
    </>
  );
}

/** Semantic tone for a risk chip's status dot. */
type ChipTone = "pass" | "fail" | "pending" | "warning" | "neutral";

/** Dot color for a risk chip, using the semantic `--color-*` tokens. */
function chipDotClass(tone: ChipTone): string {
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

/** Overall CI tone for the x/y chip, from the rolled-up summary. */
function ciSummaryTone(ci: CiSummary): ChipTone {
  const state = ciState(ci);
  switch (state) {
    case "pass":
      return "pass";
    case "fail":
      return "fail";
    case "pending":
      return "pending";
    case "none":
      return "neutral";
    default:
      return assertNever(state);
  }
}

/** Short label for a diff size bucket (`Xl` renders as `XL`). */
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

/** Short label for a sensitive-path risk flag. */
function riskLabel(flag: RiskFlag): string {
  switch (flag) {
    case "Migration":
      return "migrations";
    case "Lockfile":
      return "lockfile";
    case "CiConfig":
      return "CI config";
    case "Auth":
      return "auth";
    case "GithubDir":
      return ".github";
    case "Dependency":
      return "deps";
    default:
      return assertNever(flag);
  }
}

/** A small, calm status chip: a semantic dot plus a mono label. */
function RiskChip({
  tone,
  title,
  children,
}: {
  readonly tone: ChipTone;
  readonly title?: string | undefined;
  readonly children: React.ReactNode;
}) {
  return (
    <span
      title={title}
      className="inline-flex items-center gap-1 font-mono tabular-nums text-muted-foreground"
    >
      <span
        className={cn("h-1.5 w-1.5 shrink-0 rounded-full", chipDotClass(tone))}
        aria-hidden="true"
      />
      {children}
    </span>
  );
}

/**
 * Deterministic diff-derived risk chips shared by both densities: CI x/y, diff
 * size class, a sensitive-path flag, and a test-touch marker. The four signals
 * the research says carry most of the routing value (C3). All are parsed from
 * `review.diff.raw` once per render (memoized on the raw diff). The size chip
 * carries the F6 ">400 changed lines" splitting nudge as a warning tone.
 */
function RiskChips({ review }: { readonly review: Review }) {
  const signals = useMemo(() => {
    const totals = diffTotals(review.diff.raw);
    const flags = sensitiveFlags(review.diff.raw);
    return {
      total: totals.additions + totals.deletions,
      size: sizeClass(totals.additions, totals.deletions),
      firstFlag: flags[0] ?? null,
      hasTests: touchesTests(review.diff.raw),
    };
  }, [review.diff.raw]);

  const ci = review.ci_summary;
  // F6: nudge splitting once a diff crosses 400 changed lines.
  const oversized = signals.total > 400;

  return (
    <>
      {ci !== null && ci.total > 0 && (
        <RiskChip
          tone={ciSummaryTone(ci)}
          title={`CI: ${String(ci.passed)} passed, ${String(ci.failed)} failed, ${String(ci.pending)} pending`}
        >
          <span className="text-muted-foreground">CI</span>
          <span>
            {String(ci.passed)}/{String(ci.total)}
          </span>
        </RiskChip>
      )}

      {signals.total > 0 && (
        <RiskChip
          tone={oversized ? "warning" : "neutral"}
          title={
            oversized ? "Consider splitting (>400 changed lines)" : undefined
          }
        >
          <span className="font-semibold text-foreground">
            {sizeLabel(signals.size)}
          </span>
        </RiskChip>
      )}

      {signals.firstFlag !== null && (
        <span
          title={`Touches ${riskLabel(signals.firstFlag)}`}
          className="inline-flex items-center gap-1 font-mono text-xs text-warning"
        >
          <ShieldAlert className="h-3 w-3 shrink-0" aria-hidden="true" />
          {riskLabel(signals.firstFlag)}
        </span>
      )}

      {signals.hasTests && (
        <RiskChip tone="neutral">
          <span className="text-muted-foreground">tests</span>
        </RiskChip>
      )}
    </>
  );
}

/** Restack button; only meaningful for stale reviews. */
function RestackButton({
  review,
  onRestack,
}: {
  readonly review: Review;
  readonly onRestack: (pr: string) => void;
}) {
  const running = review.agent !== null;
  return (
    <Button
      variant="outline"
      size="sm"
      className="border-warning/40 text-warning hover:bg-warning/10"
      disabled={running}
      onClick={() => {
        onRestack(review.pr);
      }}
      title="Rebase this review onto its parent's new head"
    >
      <Layers className="h-3.5 w-3.5" />
      {running ? "Restacking…" : "Restack"}
    </Button>
  );
}

/**
 * A review-forward PR card. Leads with a status LED and the state-derived
 * reason ("why this needs you"), then the title and secondary refs, telemetry
 * chips, and a single primary action. Supports a dense `compact` density that
 * renders the same data as an aligned telemetry row.
 */
export function ReviewCard({
  review,
  onAction,
  onRestack,
  density = "cards",
  inStack = false,
}: ReviewCardProps) {
  const { repo, number: prNumber } = parsePrDisplay(review.pr);
  const action = actionConfigForState(review.gate_state);
  const signal = cardSignal(review);
  // Inside a stack container the relationship is shown by the rail/indent, so
  // the redundant text hint is suppressed there but kept in flat lists.
  const relation = inStack ? null : stackRelation(review);
  const dimmed = isDimmed(review);

  if (density === "compact") {
    return (
      <div
        className={cn(
          "group flex items-center gap-3 rounded-lg border border-border bg-card px-3 py-2 transition-colors hover:bg-card/60",
          dimmed && "opacity-60 hover:opacity-100",
        )}
      >
        <StatusLed review={review} />
        <span className="min-w-0 flex-1 truncate text-sm font-medium text-foreground">
          {review.branch}
        </span>
        <span className={cn("shrink-0 text-xs font-medium", toneTextClass(signal.tone))}>
          {signal.reason}
        </span>
        {signal.note !== undefined && (
          <span className="hidden shrink-0 text-xs text-muted-foreground lg:inline">
            · {signal.note}
          </span>
        )}
        <span className="hidden shrink-0 items-center gap-3 text-xs sm:flex">
          <TelemetryChips review={review} />
          <RiskChips review={review} />
        </span>
        {repo !== "" && (
          <span className="hidden shrink-0 font-mono text-xs text-muted-foreground md:inline">
            {repo}#{prNumber}
          </span>
        )}
        <div className="flex shrink-0 items-center gap-2 opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100">
          {review.stale && <RestackButton review={review} onRestack={onRestack} />}
          <Button
            variant={action.variant}
            size="sm"
            className={action.muted ? "opacity-60" : undefined}
            onClick={() => {
              onAction(review.pr);
            }}
          >
            {action.label}
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div
      className={cn(
        "rounded-xl border border-border bg-card p-4 transition-colors hover:bg-card/60",
        dimmed && "opacity-70 hover:opacity-100",
      )}
    >
      <div className="flex items-start gap-3">
        <span className="mt-1">
          <StatusLed review={review} />
        </span>

        <div className="min-w-0 flex-1">
          {/* Reason line — the card's headline signal, with an optional risk
              note layered under the gate reason on the same line. */}
          <div className="flex flex-wrap items-baseline gap-x-1.5 text-xs font-semibold uppercase tracking-wide">
            <span className={toneTextClass(signal.tone)}>{signal.reason}</span>
            {signal.note !== undefined && (
              <span className="font-normal normal-case tracking-normal text-muted-foreground">
                · {signal.note}
              </span>
            )}
          </div>

          {/* Title = the branch/PR subject. */}
          <div className="mt-1 truncate text-sm font-semibold text-foreground">
            {review.branch}
          </div>

          {/* Secondary refs (mono, faint). */}
          <div className="mt-0.5 flex flex-wrap items-center gap-x-2.5 gap-y-0.5 font-mono text-xs text-muted-foreground">
            {repo !== "" && (
              <span>
                {repo}#{prNumber}
              </span>
            )}
            <span>{review.issue}</span>
            {relation !== null && (
              <span className="inline-flex items-center gap-1 text-muted-foreground">
                <Layers className="h-3 w-3" />
                {relation}
              </span>
            )}
          </div>

          {/* Telemetry + risk chips. */}
          <div className="mt-2.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
            <TelemetryChips review={review} />
            <RiskChips review={review} />
          </div>
        </div>

        {/* Primary action; secondary (Restack) as ghost when stale. */}
        <div className="flex shrink-0 flex-col items-end gap-2">
          <Button
            variant={action.variant}
            size="sm"
            className={action.muted ? "opacity-60" : undefined}
            onClick={() => {
              onAction(review.pr);
            }}
          >
            {action.label}
          </Button>
          {review.stale && <RestackButton review={review} onRestack={onRestack} />}
        </div>
      </div>
    </div>
  );
}
