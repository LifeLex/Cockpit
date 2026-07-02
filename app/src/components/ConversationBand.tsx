/**
 * Read-only GitHub conversation band for the diff gate (E1).
 *
 * Surfaces a review-requested PR's existing GitHub conversation — teammates'
 * reviews, inline review comments, and issue comments — as read-only context so
 * cockpit is not blind to the discussion already happening on the PR. It is
 * deliberately NOT a reply surface: v1 shows context only, keeping cockpit's own
 * ephemeral comments (Invariant §0.4) distinct from GitHub's durable threads.
 *
 * Bodies are external and untrusted, so they render as plain preformatted text —
 * never as markdown-derived HTML.
 */

import { useEffect, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  Check,
  ExternalLink,
  Loader2,
  MessagesSquare,
  RefreshCw,
} from "lucide-react";
import type { ConversationItem } from "../bindings/ConversationItem";
import type { ConversationKind } from "../bindings/ConversationKind";
import { elapsedSince } from "@/lib/relative-time";
import { openExternal } from "@/lib/open";
import { cn } from "@/lib/utils";

interface ConversationBandProps {
  /** The GitHub conversation items, in chronological order. */
  readonly items: readonly ConversationItem[];
  /** Whether a conversation fetch/refresh is in flight. */
  readonly loading: boolean;
  /** Re-fetch the conversation from GitHub (explicit user refresh). */
  readonly onRefresh: () => void;
}

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** Presentation tone for an item's kind/state label. */
type LabelTone = "approved" | "changes" | "neutral";

/** A rendered kind/state label: its text, tone, and whether it is monospace. */
interface ItemLabel {
  readonly text: string;
  readonly tone: LabelTone;
  readonly mono: boolean;
}

/**
 * Derive the human-readable kind/state label for a conversation item. A `Review`
 * reflects its verdict (approved / requested changes / reviewed); a
 * `ReviewComment` shows its `path:line` anchor when present; an `IssueComment`
 * is a plain top-level comment.
 */
function itemLabel(item: ConversationItem): ItemLabel {
  const kind: ConversationKind = item.kind;
  switch (kind) {
    case "Review": {
      if (item.state === "APPROVED") {
        return { text: "approved", tone: "approved", mono: false };
      }
      if (item.state === "CHANGES_REQUESTED") {
        return { text: "requested changes", tone: "changes", mono: false };
      }
      return { text: "reviewed", tone: "neutral", mono: false };
    }
    case "ReviewComment": {
      if (item.path !== null) {
        const loc =
          item.line !== null
            ? `${item.path}:${String(item.line)}`
            : item.path;
        return { text: loc, tone: "neutral", mono: true };
      }
      return { text: "comment", tone: "neutral", mono: false };
    }
    case "IssueComment":
      return { text: "comment", tone: "neutral", mono: false };
    default:
      return assertNever(kind);
  }
}

/** Avatar initials from an author login: first two chars uppercased. */
function initials(author: string): string {
  const trimmed = author.trim();
  if (trimmed === "") return "?";
  return trimmed.slice(0, 2).toUpperCase();
}

/**
 * Compact relative time for a GitHub ISO-8601 timestamp. Reuses the shared
 * elapsed-time helper by converting the ISO string to epoch seconds; returns
 * null for an unparseable timestamp so the caller can omit the readout.
 */
function relativeCreated(createdAt: string): string | null {
  const ms = Date.parse(createdAt);
  if (Number.isNaN(ms)) return null;
  return elapsedSince({ secs_since_epoch: Math.floor(ms / 1000) });
}

/** The colored verdict dot preceding an approved / changes-requested label. */
function VerdictDot({ tone }: { readonly tone: LabelTone }) {
  if (tone === "approved") {
    return (
      <span className="inline-flex items-center gap-0.5 text-state-approved">
        <span
          className="h-1.5 w-1.5 shrink-0 rounded-full bg-state-approved"
          aria-hidden="true"
        />
        <Check className="h-3 w-3" />
      </span>
    );
  }
  if (tone === "changes") {
    return (
      <span
        className="h-1.5 w-1.5 shrink-0 rounded-full bg-danger"
        aria-hidden="true"
      />
    );
  }
  return null;
}

/** A single conversation item row: avatar, author, label, time, body. */
function ConversationRow({ item }: { readonly item: ConversationItem }) {
  const label = itemLabel(item);
  const when = relativeCreated(item.created_at);
  const labelColor =
    label.tone === "approved"
      ? "text-state-approved"
      : label.tone === "changes"
        ? "text-danger"
        : "text-muted-foreground";

  return (
    <li className="flex gap-2 rounded-md border border-border bg-background/40 px-2.5 py-1.5">
      <span
        className="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-muted font-mono text-[10px] font-semibold uppercase text-muted-foreground"
        aria-hidden="true"
        title={item.author}
      >
        {initials(item.author)}
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
          <span className="font-mono text-[11px] text-foreground">
            @{item.author}
          </span>
          <span
            className={cn(
              "inline-flex items-center gap-1 text-[10px]",
              label.mono && "font-mono",
              labelColor,
            )}
          >
            <VerdictDot tone={label.tone} />
            {label.text}
          </span>
          {when !== null && (
            <span className="font-mono text-[10px] text-muted-foreground/70">
              {when}
            </span>
          )}
          {item.url !== null && (
            <button
              type="button"
              onClick={() => {
                // INVARIANT: guarded by the `item.url !== null` check above.
                const href = item.url;
                if (href !== null) void openExternal(href);
              }}
              aria-label="Open on GitHub"
              title="Open on GitHub"
              className="ml-auto shrink-0 cursor-pointer border-none bg-transparent p-0.5 text-muted-foreground hover:text-foreground"
            >
              <ExternalLink className="h-3 w-3" />
            </button>
          )}
        </div>
        {item.body.trim() !== "" && (
          <pre className="mt-1 max-h-40 overflow-y-auto whitespace-pre-wrap break-words font-sans text-xs leading-relaxed text-foreground">
            {item.body}
          </pre>
        )}
      </div>
    </li>
  );
}

/**
 * A collapsible band listing a PR's GitHub conversation as read-only context.
 * Collapsed by default when empty, expanded when items exist (mirrors the
 * Intent disclosure's collapse mechanics).
 */
export function ConversationBand({
  items,
  loading,
  onRefresh,
}: ConversationBandProps) {
  // Collapsed when empty, expanded when items exist. The conversation arrives
  // asynchronously, so track whether items are present and auto-follow that
  // until the user manually toggles — after which their choice is respected.
  const [open, setOpen] = useState(items.length > 0);
  const [userToggled, setUserToggled] = useState(false);
  const hasItems = items.length > 0;
  useEffect(() => {
    if (!userToggled) setOpen(hasItems);
  }, [hasItems, userToggled]);

  return (
    <div className="shrink-0 border-b border-border bg-card/50">
      <div className="flex items-center gap-1 px-4 py-1.5">
        <button
          type="button"
          onClick={() => {
            setUserToggled(true);
            setOpen((prev) => !prev);
          }}
          aria-expanded={open}
          className={cn(
            "flex flex-1 cursor-pointer items-center gap-1.5 border-none bg-transparent p-0 text-left",
            "font-mono text-[10px] font-semibold uppercase tracking-wide text-muted-foreground hover:text-foreground",
          )}
          title="Toggle GitHub conversation"
        >
          {open ? (
            <ChevronDown className="h-3 w-3" />
          ) : (
            <ChevronRight className="h-3 w-3" />
          )}
          <MessagesSquare className="h-3 w-3" />
          CONVERSATION ON GITHUB · READ-ONLY ({String(items.length)})
        </button>
        <button
          type="button"
          onClick={onRefresh}
          disabled={loading}
          aria-label="Refresh conversation"
          title="Refresh the GitHub conversation"
          className="shrink-0 cursor-pointer border-none bg-transparent p-0.5 text-muted-foreground hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
        >
          {loading ? (
            <Loader2 className="h-3 w-3 animate-spin" />
          ) : (
            <RefreshCw className="h-3 w-3" />
          )}
        </button>
      </div>

      {open && (
        <ul className="space-y-1.5 px-4 pb-2.5 pl-9 text-xs">
          {items.map((item) => (
            <ConversationRow key={item.id} item={item} />
          ))}
          {items.length === 0 && (
            <li className="italic text-muted-foreground">
              {loading
                ? "Loading conversation…"
                : "No GitHub conversation yet."}
            </li>
          )}
        </ul>
      )}
    </div>
  );
}
