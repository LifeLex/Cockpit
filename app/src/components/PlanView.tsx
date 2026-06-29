import { useState } from "react";
import type { ProjectPlan } from "../bindings/ProjectPlan";
import type { GateState } from "../bindings/GateState";
import type { Comment } from "../bindings/Comment";
import type { Anchor } from "../bindings/Anchor";

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
    <ul className="mt-1 pl-4 list-none">
      {comments.map((c) => (
        <li
          key={c.id}
          className="px-2 py-1 mb-1 bg-surface-2 border-l-[3px] border-l-warning rounded-sm text-[13px]"
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
    <div className="mt-1 p-2 bg-surface-1 rounded border border-border">
      <div className="text-xs text-text-muted mb-1">
        Anchor: <code>{defaultAnchor}</code>
      </div>
      <textarea
        value={body}
        onChange={(e) => {
          setBody(e.target.value);
        }}
        placeholder="Add a comment..."
        className="w-full min-h-[60px] bg-surface-2 text-text-primary border border-border rounded p-2 text-[13px] resize-y box-border"
      />
      <div className="mt-1 flex gap-2">
        <button
          onClick={handleSubmit}
          disabled={body.trim().length === 0}
          className="cursor-pointer"
        >
          Add Comment
        </button>
        <button onClick={onCancel} className="cursor-pointer">
          Cancel
        </button>
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
    <li className="mb-2 p-2 bg-surface-1 rounded border border-border">
      <div
        className="flex justify-between items-start cursor-pointer"
        onClick={() => {
          setExpanded(!expanded);
        }}
      >
        <div>
          <strong>
            {stepIndex + 1}. {title}
          </strong>
        </div>
        <span className="text-text-muted text-xs">
          {expanded ? "[-]" : "[+]"}
        </span>
      </div>

      {expanded && description.length > 0 && (
        <p className="mt-2 ml-4 text-text-secondary text-[13px] whitespace-pre-wrap">
          {description}
        </p>
      )}

      <CommentList comments={comments} />

      {canComment && !commenting && (
        <button
          onClick={() => {
            setCommenting(true);
          }}
          className="mt-1 text-xs cursor-pointer text-text-muted bg-transparent border-none underline"
        >
          + Comment on step {stepIndex + 1}
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
    <li className="mb-1 px-2 py-1 bg-surface-1 rounded border border-border">
      <code className="text-[13px]">{filePath}</code>

      <CommentList comments={comments} />

      {canComment && !commenting && (
        <button
          onClick={() => {
            setCommenting(true);
          }}
          className="ml-2 text-xs cursor-pointer text-text-muted bg-transparent border-none underline"
        >
          + Comment
        </button>
      )}

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
// Gate action bar
// ---------------------------------------------------------------------------

interface GateActionsProps {
  readonly gateState: GateState;
  readonly commentCount: number;
  readonly onRequestChanges: () => void;
  readonly onApprove: () => void;
  readonly onOpen: () => void;
}

function GateActions({
  gateState,
  commentCount,
  onRequestChanges,
  onApprove,
  onOpen,
}: GateActionsProps) {
  switch (gateState) {
    case "Pending":
      return (
        <div className="mt-4">
          <button onClick={onOpen} className="cursor-pointer mr-2">
            Open for Review
          </button>
        </div>
      );

    case "InReview":
      return (
        <div className="mt-4">
          <button
            onClick={onRequestChanges}
            disabled={commentCount === 0}
            title={
              commentCount === 0
                ? "Add at least one comment before requesting changes"
                : "Send comments to the planner agent for rework"
            }
            className="cursor-pointer mr-2"
          >
            Request Changes ({String(commentCount)} comment
            {commentCount !== 1 ? "s" : ""})
          </button>
          <button
            onClick={onApprove}
            className="cursor-pointer bg-success text-white border-none px-4 py-1.5 rounded"
            title="Approve this plan and trigger the batch build. This is an irreversible action."
          >
            Approve Plan
          </button>
        </div>
      );

    case "Dispatched":
      return (
        <div className="mt-4 p-3 bg-warning/10 rounded border border-warning text-warning">
          Agent working on plan revisions...
        </div>
      );

    case "Reworked":
      return (
        <div className="mt-4">
          <button onClick={onOpen} className="cursor-pointer mr-2">
            Re-review Plan
          </button>
        </div>
      );

    case "Approved":
      return (
        <div className="mt-4 p-3 bg-success/10 rounded border border-success text-success">
          Plan Approved -- batch build triggered.
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
  readonly plan: ProjectPlan;
  readonly onAddComment: (anchor: string, body: string) => void;
  readonly onRequestChanges: () => void;
  readonly onApprove: () => void;
  readonly onOpen: () => void;
}

export function PlanView({
  plan,
  onAddComment,
  onRequestChanges,
  onApprove,
  onOpen,
}: PlanViewProps) {
  const { doc, gate_state: gateState, comments } = plan;
  const canComment = gateState === "InReview";

  return (
    <div>
      {/* Header */}
      <div className="flex justify-between items-center mb-4">
        <h2 className="m-0 text-text-primary">Plan: {doc.summary}</h2>
        <span
          className={`px-3 py-1 rounded text-xs font-bold text-white ${gateStateBgClass(gateState)}`}
        >
          {gateStateLabel(gateState)}
        </span>
      </div>

      <div className="text-text-muted text-[13px] mb-4">
        Project: {plan.project}
        {plan.agent != null && (
          <span className="ml-3 text-warning">
            Agent running (PID: {plan.agent.pid})
          </span>
        )}
      </div>

      {/* Steps */}
      <section className="mb-6">
        <h3 className="text-text-primary mb-2">
          Steps ({String(doc.steps.length)})
        </h3>
        <ol className="list-none m-0 p-0">
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
      </section>

      {/* Files */}
      <section className="mb-6">
        <h3 className="text-text-primary mb-2">
          Files ({String(doc.files.length)})
        </h3>
        <ul className="list-none m-0 p-0">
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

      {/* Risks */}
      {doc.risks.length > 0 && (
        <section className="mb-6">
          <h3 className="text-text-primary mb-2">
            Risks ({String(doc.risks.length)})
          </h3>
          <ul className="m-0 pl-5">
            {doc.risks.map((risk, i) => (
              <li key={`risk-${String(i)}`} className="mb-1 text-danger">
                {risk}
              </li>
            ))}
          </ul>
        </section>
      )}

      {/* Gate actions */}
      <GateActions
        gateState={gateState}
        commentCount={comments.length}
        onRequestChanges={onRequestChanges}
        onApprove={onApprove}
        onOpen={onOpen}
      />
    </div>
  );
}
