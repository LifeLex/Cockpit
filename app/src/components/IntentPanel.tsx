/**
 * Collapsible "Intent" disclosure for the diff gate header (D4).
 *
 * Surfaces the PR description (`review.body`) as plain preformatted text — the
 * body is untrusted, so it is rendered as text, never as markdown-derived HTML —
 * together with the Linear issue reference. Collapsed by default; the open state
 * is owned by the parent so it can be remembered per review.
 */

import { ChevronDown, ChevronRight, FileText, Hash } from "lucide-react";
import { cn } from "@/lib/utils";

interface IntentPanelProps {
  /** The PR description body. May be empty. */
  readonly body: string;
  /** The Linear issue reference (e.g. `NEX-123`). */
  readonly issue: string;
  /** External URL for the issue, or null if it does not look like a Linear ref. */
  readonly issueHref: string | null;
  /** Whether the disclosure is expanded. */
  readonly open: boolean;
  /** Toggle the expanded state. */
  readonly onToggle: () => void;
  /** Open the issue in the external browser. */
  readonly onOpenIssue: (href: string) => void;
}

/**
 * A single-band disclosure showing the PR intent (body + issue) beneath the
 * diff-gate header.
 */
export function IntentPanel({
  body,
  issue,
  issueHref,
  open,
  onToggle,
  onOpenIssue,
}: IntentPanelProps) {
  const trimmedBody = body.trim();

  return (
    <div className="shrink-0 border-b border-border bg-card/50">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        className={cn(
          "flex w-full cursor-pointer items-center gap-1.5 border-none bg-transparent px-4 py-1.5",
          "text-[10px] font-semibold uppercase tracking-wide text-muted-foreground hover:text-foreground",
        )}
        title="Toggle PR intent"
      >
        {open ? (
          <ChevronDown className="h-3 w-3" />
        ) : (
          <ChevronRight className="h-3 w-3" />
        )}
        <FileText className="h-3 w-3" />
        Intent
      </button>

      {open && (
        <div className="space-y-2 px-4 pb-2.5 pl-9 text-xs">
          {trimmedBody !== "" ? (
            <pre className="max-h-48 overflow-y-auto whitespace-pre-wrap break-words font-sans leading-relaxed text-foreground">
              {trimmedBody}
            </pre>
          ) : (
            <p className="italic text-muted-foreground">No description.</p>
          )}
          {issueHref !== null ? (
            <a
              href={issueHref}
              onClick={(e) => {
                e.preventDefault();
                onOpenIssue(issueHref);
              }}
              className="inline-flex items-center gap-1 font-mono text-muted-foreground hover:text-foreground hover:underline"
              title="Open issue on Linear"
            >
              <Hash className="h-3 w-3" />
              {issue}
            </a>
          ) : (
            <span className="inline-flex items-center gap-1 font-mono text-muted-foreground">
              <Hash className="h-3 w-3" />
              {issue}
            </span>
          )}
        </div>
      )}
    </div>
  );
}
