/**
 * Compact GitHub review submission control for review-requested PRs (D9).
 *
 * Lets the reviewer pick a verdict (Approve / Request changes / Comment) and an
 * optional summary body, then submit a single real GitHub PR review carrying the
 * inline Local comments. Submission is a guarded outward side effect
 * (Invariant §0.5 / §9): the parent confirms with the user before publishing.
 */

import { useState, useCallback } from "react";
import { Upload, Check, X, MessageSquare } from "lucide-react";
import type { ReviewEvent } from "../bindings/ReviewEvent";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** The selectable verdicts, in display order, mapped to `ReviewEvent`. */
const VERDICTS = [
  { event: "Comment", label: "Comment" },
  { event: "Approve", label: "Approve" },
  { event: "RequestChanges", label: "Request changes" },
] as const satisfies readonly { readonly event: ReviewEvent; readonly label: string }[];

interface SubmitReviewControlProps {
  /** Number of inline Local comments that will accompany the review. */
  readonly commentCount: number;
  /** Whether a submission is currently in flight. */
  readonly pending: boolean;
  /**
   * Submit the review. Returns `true` when the review was posted (so the popover
   * closes and the body resets), `false` when the user cancelled or it failed.
   */
  readonly onSubmit: (event: ReviewEvent, body: string) => Promise<boolean>;
}

/**
 * A popover-triggering "Submit Review" button that collects a verdict + summary
 * and posts one GitHub PR review.
 */
export function SubmitReviewControl({
  commentCount,
  pending,
  onSubmit,
}: SubmitReviewControlProps) {
  const [open, setOpen] = useState(false);
  const [verdict, setVerdict] = useState<ReviewEvent>("Comment");
  const [body, setBody] = useState("");

  const handleSubmit = useCallback(async () => {
    const posted = await onSubmit(verdict, body);
    if (posted) {
      setOpen(false);
      setBody("");
    }
  }, [onSubmit, verdict, body]);

  return (
    <div className="relative">
      <Button
        size="sm"
        className="bg-success text-white hover:bg-success/90"
        onClick={() => {
          setOpen((prev) => !prev);
        }}
        aria-expanded={open}
        disabled={pending}
      >
        <Upload className="h-3.5 w-3.5" />
        Submit Review
        {commentCount > 0 && (
          <span className="ml-1 inline-flex items-center gap-0.5 tabular-nums">
            <MessageSquare className="h-3 w-3" />
            {String(commentCount)}
          </span>
        )}
      </Button>

      {open && (
        <>
          {/* Backdrop closes the popover on an outside click. */}
          <button
            type="button"
            aria-label="Close"
            onClick={() => {
              setOpen(false);
            }}
            className="fixed inset-0 z-40 cursor-default border-none bg-transparent"
          />
          <div className="absolute right-0 top-full z-50 mt-1.5 w-72 rounded-md border border-border bg-card p-2.5 shadow-lg">
            <div className="mb-2 flex items-center justify-between">
              <span className="text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">
                Verdict
              </span>
              <button
                type="button"
                onClick={() => {
                  setOpen(false);
                }}
                className="cursor-pointer border-none bg-transparent text-muted-foreground hover:text-foreground"
                title="Close"
              >
                <X className="h-3.5 w-3.5" />
              </button>
            </div>

            <div className="mb-2 flex overflow-hidden rounded-md border border-border">
              {VERDICTS.map((option) => (
                <button
                  key={option.event}
                  type="button"
                  onClick={() => {
                    setVerdict(option.event);
                  }}
                  aria-pressed={verdict === option.event}
                  className={cn(
                    "flex-1 cursor-pointer border-none px-2 py-1 text-[11px] font-medium transition-colors",
                    verdict === option.event
                      ? "bg-accent text-accent-foreground"
                      : "bg-transparent text-muted-foreground hover:bg-accent/50",
                  )}
                >
                  {option.label}
                </button>
              ))}
            </div>

            <textarea
              value={body}
              onChange={(e) => {
                setBody(e.target.value);
              }}
              placeholder="Optional summary comment…"
              className="mb-2 min-h-[64px] w-full resize-none rounded-md border border-border bg-background p-2 text-xs text-foreground focus:outline-none focus:ring-1 focus:ring-ring"
              rows={3}
            />

            <div className="flex items-center justify-between">
              <span className="text-[10px] text-muted-foreground">
                {commentCount > 0
                  ? `${String(commentCount)} line comment${commentCount === 1 ? "" : "s"}`
                  : "No line comments"}
              </span>
              <Button
                size="sm"
                className="h-7 text-xs"
                onClick={() => void handleSubmit()}
                disabled={pending}
              >
                <Check className="h-3.5 w-3.5" />
                {pending ? "Posting…" : "Post to GitHub"}
              </Button>
            </div>
          </div>
        </>
      )}
    </div>
  );
}
