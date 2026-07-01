import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import {
  FolderGit2,
  GitBranch,
  GitPullRequest,
  ClipboardList,
  ChevronRight,
} from "lucide-react";
import { cn } from "@/lib/utils";
import type { Project } from "../bindings/Project";
import type { ProjectSource } from "../bindings/ProjectSource";
import type { GateState } from "../bindings/GateState";

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** Human-facing label for a project source. */
function sourceLabel(source: ProjectSource): string {
  if (source === "AdHoc") {
    return "Ad-hoc";
  }
  if ("Linear" in source) {
    return "Linear";
  }
  return assertNever(source);
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

function gateStateBadgeClass(state: GateState): string {
  switch (state) {
    case "Pending":
      return "bg-state-pending/20 text-state-pending border-state-pending/30";
    case "InReview":
      return "bg-state-in-review/20 text-state-in-review border-state-in-review/30";
    case "Dispatched":
      return "bg-state-dispatched/20 text-state-dispatched border-state-dispatched/30";
    case "Reworked":
      return "bg-state-reworked/20 text-state-reworked border-state-reworked/30";
    case "Approved":
      return "bg-state-approved/20 text-state-approved border-state-approved/30";
    default:
      return assertNever(state);
  }
}

interface ProjectCardProps {
  readonly project: Project;
  /** Number of reviews belonging to this project. */
  readonly prCount: number;
  /** Open this project's plan gate (which offers to generate a plan if none). */
  readonly onOpen: (project: Project) => void;
}

/**
 * A single project entry in the Projects list.
 *
 * Surfaces the project name, its source (Linear / ad-hoc), the plan gate state
 * (if a plan exists), and the member-PR count. The whole card is clickable and
 * routes to the project's plan gate; when the project has no plan yet, the plan
 * gate offers to generate one.
 */
export function ProjectCard({ project, prCount, onOpen }: ProjectCardProps) {
  const plan = project.plan;
  return (
    <Card className="p-0 transition-colors hover:bg-card/50">
      <CardContent className="p-4">
        <button
          type="button"
          onClick={() => {
            onOpen(project);
          }}
          className="flex w-full items-center gap-3 text-left"
        >
          <FolderGit2 className="h-4 w-4 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2.5">
              <span className="truncate text-sm font-semibold text-foreground">
                {project.name}
              </span>
              <Badge variant="outline" className="shrink-0">
                {project.source === "AdHoc" ? <FolderGit2 /> : <GitBranch />}
                {sourceLabel(project.source)}
              </Badge>
              {plan !== null && (
                <Badge
                  variant="outline"
                  className={cn("shrink-0", gateStateBadgeClass(plan.gate_state))}
                >
                  <ClipboardList />
                  {gateStateLabel(plan.gate_state)}
                </Badge>
              )}
            </div>
            <div className="mt-1.5 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
              <span className="inline-flex items-center gap-1">
                <GitPullRequest className="h-3 w-3" />
                {prCount} {prCount === 1 ? "PR" : "PRs"}
              </span>
              {plan !== null && (
                <span>{plan.doc.steps.length} plan steps</span>
              )}
            </div>
          </div>
          <ChevronRight className="h-4 w-4 shrink-0 text-muted-foreground" />
        </button>
      </CardContent>
    </Card>
  );
}
