import { useState, useEffect, useCallback } from "react";
import type { ProjectPlan } from "../bindings/ProjectPlan";
import type { ProjectId } from "../bindings/ProjectId";
import type { GateState } from "../bindings/GateState";
import type { Comment } from "../bindings/Comment";
import type { Anchor } from "../bindings/Anchor";
import type { BatchStatus } from "../bindings/BatchStatus";
import { Card, CardContent } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { GatePill } from "./GatePill";
import { Textarea } from "@/components/ui/textarea";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import {
  ClipboardList,
  FileCode,
  AlertTriangle,
  MessageSquarePlus,
  Bot,
  Plus,
  Minus,
  CheckCheck,
  Loader2,
} from "lucide-react";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** Check whether an anchor targets a specific plan step by index. */
function isStepAnchor(anchor: Anchor, stepIndex: number): boolean {
  if ("PlanStep" in anchor) {
    return anchor.PlanStep === stepIndex;
  }
  return false;
}

/** Check whether an anchor targets a specific file path. */
function isFileAnchor(anchor: Anchor, filePath: string): boolean {
  if ("PlanFile" in anchor) {
    return anchor.PlanFile === filePath;
  }
  return false;
}

/** Filter comments for a given step index. */
function commentsForStep(
  comments: readonly Comment[],
  stepIndex: number,
): readonly Comment[] {
  return comments.filter((c) => isStepAnchor(c.anchor, stepIndex));
}

/** Filter comments for a given file path. */
function commentsForFile(
  comments: readonly Comment[],
  filePath: string,
): readonly Comment[] {
  return comments.filter((c) => isFileAnchor(c.anchor, filePath));
}

// ---------------------------------------------------------------------------
// Inline comment list
// ---------------------------------------------------------------------------

interface CommentListProps {
  readonly comments: readonly Comment[];
}

function CommentList({ comments }: CommentListProps) {
  if (comments.length === 0) {
    return null;
  }
  return (
    <ul className="mt-2 flex flex-col gap-1.5">
      {comments.map((c) => (
        <li
          key={c.id}
          className="rounded-md border-l-2 border-l-warning bg-muted px-2.5 py-1.5 text-[13px] text-foreground"
        >
          {c.body}
        </li>
      ))}
    </ul>
  );
}

// ---------------------------------------------------------------------------
// Comment form
// ---------------------------------------------------------------------------

interface CommentFormProps {
  /** The pre-filled anchor string (e.g. "step:0" or "file:src/lib.rs"). */
  readonly defaultAnchor: string;
  readonly onSubmit: (anchor: string, body: string) => void;
  readonly onCancel: () => void;
}

function CommentForm({ defaultAnchor, onSubmit, onCancel }: CommentFormProps) {
  const [body, setBody] = useState("");

  const handleSubmit = () => {
    const trimmed = body.trim();
    if (trimmed.length === 0) {
      return;
    }
    onSubmit(defaultAnchor, trimmed);
    setBody("");
  };

  return (
    <div className="mt-2 rounded-lg border border-border bg-card p-2.5">
      <div className="mb-1.5 text-xs text-muted-foreground">
        Anchor:{" "}
        <code className="rounded bg-muted px-1 py-0.5 text-[11px]">
          {defaultAnchor}
        </code>
      </div>
      <Textarea
        value={body}
        onChange={(e) => {
          setBody(e.target.value);
        }}
        placeholder="Add a comment..."
        className="min-h-[60px] text-[13px]"
        autoFocus
      />
      <div className="mt-2 flex gap-2">
        <Button
          size="sm"
          onClick={handleSubmit}
          disabled={body.trim().length === 0}
        >
          Add comment
        </Button>
        <Button size="sm" variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Step item
// ---------------------------------------------------------------------------

interface StepItemProps {
  readonly stepIndex: number;
  readonly title: string;
  readonly description: string;
  readonly comments: readonly Comment[];
  readonly canComment: boolean;
  readonly onAddComment: (anchor: string, body: string) => void;
}

function StepItem({
  stepIndex,
  title,
  description,
  comments,
  canComment,
  onAddComment,
}: StepItemProps) {
  const [expanded, setExpanded] = useState(false);
  const [commenting, setCommenting] = useState(false);

  return (
    <li className="rounded-lg border border-border bg-card p-3">
      <button
        type="button"
        className="flex w-full items-start justify-between gap-3 text-left"
        onClick={() => {
          setExpanded((prev) => !prev);
        }}
      >
        <span className="text-sm font-medium text-foreground">
          <span className="mr-1.5 text-muted-foreground">{stepIndex + 1}.</span>
          {title}
        </span>
        <span className="shrink-0 text-muted-foreground">
          {expanded ? (
            <Minus className="h-3.5 w-3.5" />
          ) : (
            <Plus className="h-3.5 w-3.5" />
          )}
        </span>
      </button>

      {expanded && description.length > 0 && (
        <p className="mt-2 whitespace-pre-wrap text-[13px] leading-relaxed text-muted-foreground">
          {description}
        </p>
      )}

      <CommentList comments={comments} />

      {canComment && !commenting && (
        <button
          type="button"
          onClick={() => {
            setCommenting(true);
          }}
          className="mt-2 inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
        >
          <MessageSquarePlus className="h-3 w-3" />
          Comment on step {stepIndex + 1}
        </button>
      )}

      {commenting && (
        <CommentForm
          defaultAnchor={`step:${String(stepIndex)}`}
          onSubmit={(anchor, body) => {
            onAddComment(anchor, body);
            setCommenting(false);
          }}
          onCancel={() => {
            setCommenting(false);
          }}
        />
      )}
    </li>
  );
}

// ---------------------------------------------------------------------------
// File item
// ---------------------------------------------------------------------------

interface FileItemProps {
  readonly filePath: string;
  readonly comments: readonly Comment[];
  readonly canComment: boolean;
  readonly onAddComment: (anchor: string, body: string) => void;
}

function FileItem({
  filePath,
  comments,
  canComment,
  onAddComment,
}: FileItemProps) {
  const [commenting, setCommenting] = useState(false);

  return (
    <li className="rounded-lg border border-border bg-card px-3 py-2.5">
      <div className="flex items-center gap-2">
        <FileCode className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <code className="truncate text-[13px] text-foreground">{filePath}</code>
        {canComment && !commenting && (
          <button
            type="button"
            onClick={() => {
              setCommenting(true);
            }}
            className="ml-auto inline-flex shrink-0 items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
          >
            <MessageSquarePlus className="h-3 w-3" />
            Comment
          </button>
        )}
      </div>

      <CommentList comments={comments} />

      {commenting && (
        <CommentForm
          defaultAnchor={`file:${filePath}`}
          onSubmit={(anchor, body) => {
            onAddComment(anchor, body);
            setCommenting(false);
          }}
          onCancel={() => {
            setCommenting(false);
          }}
        />
      )}
    </li>
  );
}

// ---------------------------------------------------------------------------
// Batch progress (post-approval)
// ---------------------------------------------------------------------------

interface BatchProgressProps {
  readonly status: BatchStatus;
}

function BatchProgress({ status }: BatchProgressProps) {
  const cells: readonly { readonly value: number; readonly label: string }[] = [
    { value: status.total, label: "Total" },
    { value: status.building, label: "Building" },
    { value: status.ready, label: "Ready" },
    { value: status.approved, label: "Approved" },
  ];
  return (
    <div className="grid grid-cols-4 gap-3">
      {cells.map((cell) => (
        <div
          key={cell.label}
          className="rounded-lg border border-border bg-muted px-4 py-3 text-center"
        >
          <p className="text-2xl font-semibold text-foreground">{cell.value}</p>
          <p className="text-xs text-muted-foreground">{cell.label}</p>
        </div>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Gate action bar
// ---------------------------------------------------------------------------

interface GateActionsProps {
  readonly gateState: GateState;
  readonly commentCount: number;
  readonly onRequestChanges: () => void;
  readonly onApproveRequest: () => void;
  readonly onOpen: () => void;
}

function GateActions({
  gateState,
  commentCount,
  onRequestChanges,
  onApproveRequest,
  onOpen,
}: GateActionsProps) {
  switch (gateState) {
    case "Pending":
      return (
        <div className="flex items-center gap-3">
          <Button onClick={onOpen}>Open for review</Button>
        </div>
      );

    case "InReview":
      return (
        <div className="flex items-center gap-3">
          <Button
            variant="outline"
            onClick={onRequestChanges}
            disabled={commentCount === 0}
            title={
              commentCount === 0
                ? "Add at least one comment before requesting changes"
                : "Send comments to the planner agent for rework"
            }
          >
            Request changes
            {commentCount > 0 ? ` (${String(commentCount)})` : ""}
          </Button>
          <Button onClick={onApproveRequest}>
            <CheckCheck />
            Approve &amp; build
          </Button>
        </div>
      );

    case "Dispatched":
      return (
        <div className="flex items-center gap-2 rounded-lg border border-warning/40 bg-warning/10 px-4 py-3 text-sm text-warning">
          <Loader2 className="h-4 w-4 animate-spin" />
          Agent working on plan revisions...
        </div>
      );

    case "Reworked":
      return (
        <div className="flex items-center gap-3">
          <Button onClick={onOpen}>Re-review plan</Button>
        </div>
      );

    case "Approved":
      return (
        <div className="flex items-center gap-2 rounded-lg border border-success/40 bg-success/10 px-4 py-3 text-sm text-success">
          <CheckCheck className="h-4 w-4" />
          Plan approved — batch build triggered.
        </div>
      );

    // Plans never merge (Merged is a Review-only terminal state); this case
    // exists only to keep the GateState switch exhaustive.
    case "Merged":
      return (
        <div className="flex items-center gap-2 rounded-lg border border-border bg-muted/30 px-4 py-3 text-sm text-muted-foreground">
          <CheckCheck className="h-4 w-4" />
          Merged.
        </div>
      );

    default:
      return assertNever(gateState);
  }
}

// ---------------------------------------------------------------------------
// PlanView (main export)
// ---------------------------------------------------------------------------

interface PlanViewProps {
  /** The project whose plan gate is being shown. */
  readonly projectId: ProjectId;
  /** The project's plan, or `null` when it has none yet. */
  readonly plan: ProjectPlan | null;
  /** Generate the plan via the planner agent (when the project has none). */
  readonly onGenerate: () => void;
  readonly onAddComment: (anchor: string, body: string) => void;
  readonly onRequestChanges: () => void;
  readonly onApprove: () => void;
  readonly onOpen: () => void;
  /** Fetch aggregate batch progress once the plan is approved. */
  readonly onFetchBatchStatus: () => Promise<BatchStatus | null>;
}

/**
 * The plan gate for a single project.
 *
 * When the project has no plan yet, renders a "Generate plan" affordance that
 * spawns the planner agent. Otherwise renders the plan document with the
 * standard gate actions (open / request changes / approve-and-build).
 */
export function PlanView(props: PlanViewProps) {
  if (props.plan === null) {
    return <GeneratePlanState onGenerate={props.onGenerate} />;
  }
  return <PlanGate {...props} plan={props.plan} />;
}

/** Empty state shown when a project has no plan; offers to generate one. */
function GeneratePlanState({ onGenerate }: { readonly onGenerate: () => void }) {
  return (
    <div className="flex flex-col items-center justify-center gap-4 rounded-lg border border-dashed border-border bg-card px-6 py-16 text-center">
      <ClipboardList className="h-10 w-10 text-muted-foreground" />
      <div>
        <h1 className="text-lg font-semibold text-foreground">No plan yet</h1>
        <p className="mt-1 max-w-md text-sm text-muted-foreground">
          Generate a project plan with the planner agent. It runs against your
          repo and writes a plan you can review, comment on, and approve.
        </p>
      </div>
      <Button onClick={onGenerate}>
        <Bot />
        Generate plan
      </Button>
    </div>
  );
}

interface PlanGateProps extends PlanViewProps {
  readonly plan: ProjectPlan;
}

function PlanGate({
  plan,
  onAddComment,
  onRequestChanges,
  onApprove,
  onOpen,
  onFetchBatchStatus,
}: PlanGateProps) {
  const { doc, gate_state: gateState, comments } = plan;
  const canComment = gateState === "InReview";

  const [confirmApprove, setConfirmApprove] = useState(false);
  const [batch, setBatch] = useState<BatchStatus | null>(null);

  // Once approved, load batch progress so the fan-out is visible.
  useEffect(() => {
    if (gateState !== "Approved") {
      setBatch(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      const status = await onFetchBatchStatus();
      if (!cancelled) {
        setBatch(status);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [gateState, onFetchBatchStatus]);

  const handleConfirmApprove = useCallback(() => {
    setConfirmApprove(false);
    onApprove();
  }, [onApprove]);

  return (
    <div className="flex flex-col gap-6">
      {/* Header */}
      <div className="flex items-start justify-between gap-3">
        <div className="flex items-center gap-3">
          <ClipboardList className="h-6 w-6 shrink-0 text-muted-foreground" />
          <div>
            <h1 className="text-xl font-semibold text-foreground">
              {doc.summary}
            </h1>
            <p className="mt-0.5 flex items-center gap-3 text-sm text-muted-foreground">
              <span>Project: {plan.project}</span>
              {plan.agent != null && (
                <span className="inline-flex items-center gap-1 text-warning">
                  <Bot className="h-3.5 w-3.5" />
                  PID {plan.agent.pid}
                </span>
              )}
            </p>
          </div>
        </div>
        <GatePill state={gateState} />
      </div>

      {/* Batch progress (after approval) */}
      {gateState === "Approved" && batch !== null && (
        <BatchProgress status={batch} />
      )}

      {/* Steps */}
      <section className="flex flex-col gap-2">
        <h2 className="text-sm font-semibold uppercase tracking-wide text-muted-foreground">
          Steps ({String(doc.steps.length)})
        </h2>
        {doc.steps.length === 0 ? (
          <p className="text-sm text-muted-foreground">No steps in this plan.</p>
        ) : (
          <ol className="flex flex-col gap-2">
            {doc.steps.map((step) => (
              <StepItem
                key={step.index}
                stepIndex={step.index}
                title={step.title}
                description={step.description}
                comments={commentsForStep(comments, step.index)}
                canComment={canComment}
                onAddComment={onAddComment}
              />
            ))}
          </ol>
        )}
      </section>

      {/* Files */}
      {doc.files.length > 0 && (
        <section className="flex flex-col gap-2">
          <h2 className="text-sm font-semibold uppercase tracking-wide text-muted-foreground">
            Files ({String(doc.files.length)})
          </h2>
          <ul className="flex flex-col gap-2">
            {doc.files.map((file) => (
              <FileItem
                key={file}
                filePath={file}
                comments={commentsForFile(comments, file)}
                canComment={canComment}
                onAddComment={onAddComment}
              />
            ))}
          </ul>
        </section>
      )}

      {/* Risks */}
      {doc.risks.length > 0 && (
        <section className="flex flex-col gap-2">
          <h2 className="text-sm font-semibold uppercase tracking-wide text-muted-foreground">
            Risks ({String(doc.risks.length)})
          </h2>
          <Card>
            <CardContent>
              <ul className="flex flex-col gap-2">
                {doc.risks.map((risk, i) => (
                  <li
                    key={`risk-${String(i)}`}
                    className="flex items-start gap-2 text-sm text-danger"
                  >
                    <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                    <span>{risk}</span>
                  </li>
                ))}
              </ul>
            </CardContent>
          </Card>
        </section>
      )}

      {/* Gate actions */}
      <div className="border-t border-border pt-4">
        <GateActions
          gateState={gateState}
          commentCount={comments.length}
          onRequestChanges={onRequestChanges}
          onApproveRequest={() => {
            setConfirmApprove(true);
          }}
          onOpen={onOpen}
        />
      </div>

      {/* Approve confirmation (guarded fan-out, Invariant §9). */}
      <Dialog
        open={confirmApprove}
        onOpenChange={(open) => {
          if (!open) setConfirmApprove(false);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Approve plan &amp; build the batch</DialogTitle>
            <DialogDescription>
              Approving this plan spawns an implementer agent for every review in
              the batch. This is an explicit, irreversible action.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => {
                setConfirmApprove(false);
              }}
            >
              Cancel
            </Button>
            <Button onClick={handleConfirmApprove}>
              <CheckCheck />
              Approve &amp; build
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
