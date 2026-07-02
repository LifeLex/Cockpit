/**
 * Global amber strip announcing that one or more spawned agents are BLOCKED
 * waiting for a tool-permission decision (Approve mode blocks up to 5 minutes,
 * then auto-denies).
 *
 * Pinned above the main content, it is the app-wide "someone is waiting on you"
 * signal for when the reviewer is not on the relevant review's Agent tab. It
 * shows the pending count and the first request's summary, plus a Review button
 * that routes to where that request can be resolved: a review's workspace (PR
 * ref) or a project's plan view (project id). An object that matches neither
 * shows no button rather than navigating somewhere wrong.
 */

import { useCallback } from "react";
import { useAppStore } from "../store";
import { Button } from "@/components/ui/button";

/**
 * Slim amber banner shown while any tool-permission request is pending; renders
 * nothing when the queue is empty.
 */
export function PermissionBanner() {
  const pendingPermissions = useAppStore((s) => s.pendingPermissions);
  const reviews = useAppStore((s) => s.reviews);
  const authoredPrs = useAppStore((s) => s.authoredPrs);
  const reviewRequests = useAppStore((s) => s.reviewRequests);
  const projects = useAppStore((s) => s.projects);
  const navigateToDiff = useAppStore((s) => s.navigateToDiff);
  const navigateToPlan = useAppStore((s) => s.navigateToPlan);

  const first = pendingPermissions[0];

  // Route the first request's object id: a PR ref navigates to that review's
  // workspace (where AgentPanel shows the Allow/Deny entry); a project id
  // navigates to the plan view; anything unmatched yields no navigation.
  const handleReview = useCallback(() => {
    if (first === undefined) return;
    const objectId = first.object_id;
    const review = [...reviews, ...authoredPrs, ...reviewRequests].find(
      (r) => r.pr === objectId,
    );
    if (review !== undefined) {
      void navigateToDiff(review.pr);
      return;
    }
    const project = projects.find((p) => p.id === objectId);
    if (project !== undefined) {
      navigateToPlan(project.id);
    }
  }, [
    first,
    reviews,
    authoredPrs,
    reviewRequests,
    projects,
    navigateToDiff,
    navigateToPlan,
  ]);

  if (first === undefined) return null;

  const count = pendingPermissions.length;
  const routable =
    [...reviews, ...authoredPrs, ...reviewRequests].some(
      (r) => r.pr === first.object_id,
    ) || projects.some((p) => p.id === first.object_id);

  return (
    <div className="flex shrink-0 items-center gap-3 border-b border-warning/30 bg-warning/10 px-4 py-2 text-sm">
      {/* Amber status dot; pulses while a decision is outstanding. */}
      <span className="relative flex h-2.5 w-2.5 shrink-0" aria-hidden="true">
        <span className="absolute inline-flex h-full w-full rounded-full bg-warning opacity-60 animate-ping motion-reduce:hidden" />
        <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-warning" />
      </span>

      <span className="font-medium text-warning">
        {count} {count === 1 ? "agent" : "agents"} waiting for permission
      </span>

      <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground">
        {first.summary}
      </span>

      {routable && (
        <Button
          size="sm"
          variant="outline"
          onClick={handleReview}
          className="shrink-0"
        >
          Review
        </Button>
      )}
    </div>
  );
}
