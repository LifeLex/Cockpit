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
    <ul
      style={{
        margin: "4px 0 0 0",
        paddingLeft: 16,
        listStyle: "none",
      }}
    >
      {comments.map((c) => (
        <li
          key={c.id}
          style={{
            padding: "4px 8px",
            marginBottom: 4,
            backgroundColor: "#2a2a2a",
            borderLeft: "3px solid #FF9800",
            borderRadius: 2,
            fontSize: 13,
          }}
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
    <div
      style={{
        marginTop: 4,
        padding: 8,
        backgroundColor: "#1e1e1e",
        borderRadius: 4,
        border: "1px solid #444",
      }}
    >
      <div style={{ fontSize: 12, color: "#888", marginBottom: 4 }}>
        Anchor: <code>{defaultAnchor}</code>
      </div>
      <textarea
        value={body}
        onChange={(e) => {
          setBody(e.target.value);
        }}
        placeholder="Add a comment..."
        style={{
          width: "100%",
          minHeight: 60,
          backgroundColor: "#2a2a2a",
          color: "#eee",
          border: "1px solid #555",
          borderRadius: 4,
          padding: 8,
          fontSize: 13,
          resize: "vertical",
          boxSizing: "border-box",
        }}
      />
      <div style={{ marginTop: 4, display: "flex", gap: 8 }}>
        <button
          onClick={handleSubmit}
          disabled={body.trim().length === 0}
          style={{ cursor: "pointer" }}
        >
          Add Comment
        </button>
        <button onClick={onCancel} style={{ cursor: "pointer" }}>
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
    <li
      style={{
        marginBottom: 8,
        padding: 8,
        backgroundColor: "#1a1a1a",
        borderRadius: 4,
        border: "1px solid #333",
      }}
    >
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "flex-start",
          cursor: "pointer",
        }}
        onClick={() => {
          setExpanded(!expanded);
        }}
      >
        <div>
          <strong>
            {stepIndex + 1}. {title}
          </strong>
        </div>
        <span style={{ color: "#888", fontSize: 12 }}>
          {expanded ? "[-]" : "[+]"}
        </span>
      </div>

      {expanded && description.length > 0 && (
        <p
          style={{
            margin: "8px 0 0 16px",
            color: "#bbb",
            fontSize: 13,
            whiteSpace: "pre-wrap",
          }}
        >
          {description}
        </p>
      )}

      <CommentList comments={comments} />

      {canComment && !commenting && (
        <button
          onClick={() => {
            setCommenting(true);
          }}
          style={{
            marginTop: 4,
            fontSize: 12,
            cursor: "pointer",
            color: "#888",
            background: "none",
            border: "none",
            textDecoration: "underline",
          }}
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
    <li
      style={{
        marginBottom: 4,
        padding: "4px 8px",
        backgroundColor: "#1a1a1a",
        borderRadius: 4,
        border: "1px solid #333",
      }}
    >
      <code style={{ fontSize: 13 }}>{filePath}</code>

      <CommentList comments={comments} />

      {canComment && !commenting && (
        <button
          onClick={() => {
            setCommenting(true);
          }}
          style={{
            marginLeft: 8,
            fontSize: 12,
            cursor: "pointer",
            color: "#888",
            background: "none",
            border: "none",
            textDecoration: "underline",
          }}
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
        <div style={{ marginTop: 16 }}>
          <button
            onClick={onOpen}
            style={{ cursor: "pointer", marginRight: 8 }}
          >
            Open for Review
          </button>
        </div>
      );

    case "InReview":
      return (
        <div style={{ marginTop: 16 }}>
          <button
            onClick={onRequestChanges}
            disabled={commentCount === 0}
            title={
              commentCount === 0
                ? "Add at least one comment before requesting changes"
                : "Send comments to the planner agent for rework"
            }
            style={{ cursor: "pointer", marginRight: 8 }}
          >
            Request Changes ({String(commentCount)} comment
            {commentCount !== 1 ? "s" : ""})
          </button>
          <button
            onClick={onApprove}
            style={{
              cursor: "pointer",
              backgroundColor: "#4CAF50",
              color: "white",
              border: "none",
              padding: "6px 16px",
              borderRadius: 4,
            }}
            title="Approve this plan and trigger the batch build. This is an irreversible action."
          >
            Approve Plan
          </button>
        </div>
      );

    case "Dispatched":
      return (
        <div
          style={{
            marginTop: 16,
            padding: 12,
            backgroundColor: "#332200",
            borderRadius: 4,
            border: "1px solid #FF9800",
          }}
        >
          Agent working on plan revisions...
        </div>
      );

    case "Reworked":
      return (
        <div style={{ marginTop: 16 }}>
          <button
            onClick={onOpen}
            style={{ cursor: "pointer", marginRight: 8 }}
          >
            Re-review Plan
          </button>
        </div>
      );

    case "Approved":
      return (
        <div
          style={{
            marginTop: 16,
            padding: 12,
            backgroundColor: "#1b3320",
            borderRadius: 4,
            border: "1px solid #4CAF50",
            color: "#4CAF50",
          }}
        >
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
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          marginBottom: 16,
        }}
      >
        <h2 style={{ margin: 0 }}>Plan: {doc.summary}</h2>
        <span
          style={{
            padding: "4px 12px",
            borderRadius: 4,
            backgroundColor: gateStateColor(gateState),
            color: "white",
            fontSize: 12,
            fontWeight: "bold",
          }}
        >
          {gateStateLabel(gateState)}
        </span>
      </div>

      <div style={{ color: "#888", fontSize: 13, marginBottom: 16 }}>
        Project: {plan.project}
        {plan.agent != null && (
          <span style={{ marginLeft: 12, color: "#FF9800" }}>
            Agent running (PID: {plan.agent.pid})
          </span>
        )}
      </div>

      {/* Steps */}
      <section style={{ marginBottom: 24 }}>
        <h3>Steps ({String(doc.steps.length)})</h3>
        <ol
          style={{
            listStyle: "none",
            margin: 0,
            padding: 0,
          }}
        >
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
      <section style={{ marginBottom: 24 }}>
        <h3>Files ({String(doc.files.length)})</h3>
        <ul style={{ listStyle: "none", margin: 0, padding: 0 }}>
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
        <section style={{ marginBottom: 24 }}>
          <h3>Risks ({String(doc.risks.length)})</h3>
          <ul style={{ margin: 0, paddingLeft: 20 }}>
            {doc.risks.map((risk, i) => (
              <li
                key={`risk-${String(i)}`}
                style={{ marginBottom: 4, color: "#e88" }}
              >
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
