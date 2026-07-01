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
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { FolderPlus, GitBranch, Loader2 } from "lucide-react";

interface NewProjectViewProps {
  /** Called when the user cancels or finishes; routes back to Projects. */
  readonly onDone: () => void;
}

/** Which creation mode the segmented control has selected. */
type CreateMode = "blank" | "linear";

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/**
 * Create-a-project flow.
 *
 * Two modes via a segmented control: an ad-hoc blank project (`createProject`)
 * and a Linear import (`createProjectFromLinear`, lifted from the kickoff flow).
 * Linear is one optional source, not the entry point — Blank is the default.
 */
export function NewProjectView({ onDone }: NewProjectViewProps) {
  const config = useAppStore((s) => s.config);
  const error = useAppStore((s) => s.error);
  const projectsLoading = useAppStore((s) => s.projectsLoading);
  const kickoffLoading = useAppStore((s) => s.kickoffLoading);
  const kickoffResult = useAppStore((s) => s.kickoffResult);
  const createProject = useAppStore((s) => s.createProject);
  const createProjectFromLinear = useAppStore((s) => s.createProjectFromLinear);
  const fetchConfig = useAppStore((s) => s.fetchConfig);

  const [mode, setMode] = useState<CreateMode>("blank");

  // --- Blank ---
  const [name, setName] = useState("");

  // --- Linear ---
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

  const handleCreateBlank = useCallback(async () => {
    const trimmed = name.trim();
    if (trimmed === "") return;
    const project = await createProject(trimmed);
    if (project !== null) {
      onDone();
    }
  }, [name, createProject, onDone]);

  const handleCreateLinear = useCallback(() => {
    if (projectId.trim() === "") return;
    void createProjectFromLinear(projectId.trim(), skipPlan);
  }, [projectId, skipPlan, createProjectFromLinear]);

  // On a successful Linear import, kickoffResult is populated; route back.
  useEffect(() => {
    if (mode === "linear" && kickoffResult !== null) {
      onDone();
    }
  }, [mode, kickoffResult, onDone]);

  const handleModeChange = useCallback((value: unknown) => {
    if (value === "blank" || value === "linear") {
      setMode(value);
    }
  }, []);

  return (
    <div className="mx-auto max-w-2xl px-6 py-8">
      <div className="mb-6 flex items-center gap-3">
        <FolderPlus className="h-6 w-6 text-muted-foreground" />
        <div>
          <h1 className="text-xl font-semibold text-foreground">New Project</h1>
          <p className="text-sm text-muted-foreground">
            Create an ad-hoc project, or import one from Linear.
          </p>
        </div>
      </div>

      {error !== null && (
        <p className="mb-6 text-sm text-destructive">{error}</p>
      )}

      <Tabs value={mode} onValueChange={handleModeChange}>
        <TabsList className="mb-6">
          <TabsTrigger value="blank">
            <FolderPlus />
            Blank
          </TabsTrigger>
          <TabsTrigger value="linear">
            <GitBranch />
            From Linear
          </TabsTrigger>
        </TabsList>

        {/* ----------------------------------------------------------------- */}
        {/* Blank (ad-hoc)                                                    */}
        {/* ----------------------------------------------------------------- */}
        <TabsContent value="blank">
          <Card>
            <CardHeader>
              <CardTitle>Ad-hoc project</CardTitle>
              <CardDescription>
                A blank project you attach reviews to manually. No Linear
                backing required.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <form
                onSubmit={(e) => {
                  e.preventDefault();
                  void handleCreateBlank();
                }}
                className="flex flex-col gap-5"
              >
                <div className="flex flex-col gap-2">
                  <Label htmlFor="project-name">Project name</Label>
                  <Input
                    id="project-name"
                    type="text"
                    value={name}
                    onChange={(e) => {
                      setName(e.target.value);
                    }}
                    placeholder="Payments revamp"
                    disabled={projectsLoading}
                    autoFocus
                  />
                </div>

                <div className="flex justify-end gap-3 pt-2">
                  <Button
                    type="button"
                    variant="outline"
                    onClick={onDone}
                    disabled={projectsLoading}
                  >
                    Cancel
                  </Button>
                  <Button
                    type="submit"
                    disabled={projectsLoading || name.trim() === ""}
                  >
                    {projectsLoading ? (
                      <Loader2 className="animate-spin" />
                    ) : (
                      <FolderPlus />
                    )}
                    {projectsLoading ? "Creating..." : "Create project"}
                  </Button>
                </div>
              </form>
            </CardContent>
          </Card>
        </TabsContent>

        {/* ----------------------------------------------------------------- */}
        {/* From Linear                                                       */}
        {/* ----------------------------------------------------------------- */}
        <TabsContent value="linear">
          <Card>
            <CardHeader>
              <CardTitle>Import from Linear</CardTitle>
              <CardDescription>
                Fetch issues from a Linear project, compute the frontier, and
                create reviews.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <form
                onSubmit={(e) => {
                  e.preventDefault();
                  handleCreateLinear();
                }}
                className="flex flex-col gap-5"
              >
                <div className="flex flex-col gap-2">
                  <Label htmlFor="linear-project-id">Linear project ID</Label>
                  <Input
                    id="linear-project-id"
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
                    Skip plan gate
                  </Label>
                </div>

                <div className="flex justify-end gap-3 pt-2">
                  <Button
                    type="button"
                    variant="outline"
                    onClick={onDone}
                    disabled={kickoffLoading}
                  >
                    Cancel
                  </Button>
                  <Button
                    type="submit"
                    disabled={kickoffLoading || projectId.trim() === ""}
                  >
                    {kickoffLoading ? (
                      <Loader2 className="animate-spin" />
                    ) : (
                      <GitBranch />
                    )}
                    {kickoffLoading ? "Importing..." : "Import project"}
                  </Button>
                </div>
              </form>
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>

      {/* Guard against the (impossible) unhandled mode. */}
      {mode !== "blank" && mode !== "linear" ? assertNever(mode) : null}
    </div>
  );
}
