import { useState, useEffect, useCallback } from "react";
import { useAppStore } from "../store";
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Rocket, Loader2 } from "lucide-react";
import { cn } from "@/lib/utils";

export function KickoffView() {
  const config = useAppStore((s) => s.config);
  const error = useAppStore((s) => s.error);
  const kickoffLoading = useAppStore((s) => s.kickoffLoading);
  const kickoffResult = useAppStore((s) => s.kickoffResult);
  const runKickoff = useAppStore((s) => s.runKickoff);
  const fetchConfig = useAppStore((s) => s.fetchConfig);

  const [projectId, setProjectId] = useState("");
  const [skipPlan, setSkipPlan] = useState(false);

  useEffect(() => {
    void fetchConfig();
  }, [fetchConfig]);

  useEffect(() => {
    if (config !== null && projectId === "") {
      setProjectId(config.linear_project_id ?? "");
    }
  }, [config, projectId]);

  const handleKickoff = useCallback(() => {
    if (projectId.trim() === "") return;
    void runKickoff(projectId.trim(), skipPlan);
  }, [projectId, skipPlan, runKickoff]);

  return (
    <div className="mx-auto max-w-2xl px-6 py-8">
      <div className="mb-6 flex items-center gap-3">
        <Rocket className="h-6 w-6 text-text-secondary" />
        <div>
          <h1 className="text-xl font-semibold text-text-primary">Kickoff</h1>
          <p className="text-sm text-text-secondary">
            Fetch issues from Linear, compute the frontier, and create reviews.
          </p>
        </div>
      </div>

      {error !== null && (
        <p className="mb-6 text-sm text-destructive">{error}</p>
      )}

      <Card>
        <CardHeader>
          <CardTitle>New Project Import</CardTitle>
          <CardDescription>
            Enter your Linear project ID to import issues and start the review
            flow.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              handleKickoff();
            }}
            className="flex flex-col gap-5"
          >
            <div className="flex flex-col gap-2">
              <Label htmlFor="project-id">Linear Project ID</Label>
              <Input
                id="project-id"
                type="text"
                value={projectId}
                onChange={(e) => {
                  setProjectId(e.target.value);
                }}
                placeholder="PRJ-123"
                disabled={kickoffLoading}
              />
            </div>

            <div className="flex items-center gap-3">
              <Switch
                id="skip-plan"
                checked={skipPlan}
                onCheckedChange={setSkipPlan}
                disabled={kickoffLoading}
              />
              <Label htmlFor="skip-plan" className="cursor-pointer">
                Skip Plan Gate
              </Label>
            </div>

            <div className="pt-2">
              <Button
                type="submit"
                disabled={kickoffLoading || projectId.trim() === ""}
              >
                {kickoffLoading ? (
                  <Loader2 className="animate-spin" />
                ) : (
                  <Rocket />
                )}
                {kickoffLoading ? "Running..." : "Kick Off"}
              </Button>
            </div>
          </form>
        </CardContent>
      </Card>

      {kickoffResult !== null && (
        <Card className="mt-6">
          <CardHeader>
            <CardTitle>Kickoff Complete</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-6">
            <div className="grid grid-cols-3 gap-4">
              <StatCard
                value={kickoffResult.issue_count}
                label="Issues fetched"
              />
              <StatCard
                value={kickoffResult.frontier.length}
                label="Frontier issues"
              />
              <StatCard
                value={kickoffResult.reviews.length}
                label="Reviews created"
              />
            </div>

            <p className="text-sm text-muted-foreground">
              Plan:{" "}
              <span className="font-medium text-foreground">
                {kickoffResult.plan !== null ? "Created" : "Skipped"}
              </span>
            </p>

            {kickoffResult.reviews.length > 0 && (
              <div className="border-t border-border pt-4">
                <h3 className="mb-2 text-sm font-medium text-text-secondary">
                  Created Reviews
                </h3>
                <ul className="flex flex-col gap-1">
                  {kickoffResult.reviews.map((review) => (
                    <li
                      key={review.id}
                      className="rounded-md bg-surface-2 px-3 py-2 text-sm text-text-primary"
                    >
                      <span className="font-medium">{review.pr}</span>
                      <span className="ml-2 text-text-muted">
                        {review.branch}
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </CardContent>
        </Card>
      )}
    </div>
  );
}

interface StatCardProps {
  readonly value: number;
  readonly label: string;
}

function StatCard({ value, label }: StatCardProps) {
  return (
    <div
      className={cn(
        "rounded-lg border border-border bg-surface-2 px-4 py-3 text-center",
      )}
    >
      <p className="text-2xl font-semibold text-text-primary">{value}</p>
      <p className="text-xs text-muted-foreground">{label}</p>
    </div>
  );
}
