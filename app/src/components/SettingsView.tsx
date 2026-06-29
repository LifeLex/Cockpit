import { useState, useEffect, useCallback } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "../store";
import type { Config } from "../bindings/Config";
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
  CardFooter,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { Save, FolderOpen, Eye, EyeOff, Settings } from "lucide-react";
import { cn } from "@/lib/utils";

type Feedback =
  | { readonly kind: "success"; readonly message: string }
  | { readonly kind: "error"; readonly message: string };

export function SettingsView() {
  const config = useAppStore((s) => s.config);
  const configLoading = useAppStore((s) => s.configLoading);
  const configError = useAppStore((s) => s.configError);
  const fetchConfig = useAppStore((s) => s.fetchConfig);
  const saveConfig = useAppStore((s) => s.saveConfig);

  const [linearApiKey, setLinearApiKey] = useState("");
  const [linearProjectId, setLinearProjectId] = useState("");
  const [repoPath, setRepoPath] = useState("");
  const [agentCommand, setAgentCommand] = useState("claude");
  const [hookPort, setHookPort] = useState(19876);
  const [showApiKey, setShowApiKey] = useState(false);
  const [feedback, setFeedback] = useState<Feedback | null>(null);

  useEffect(() => {
    void fetchConfig();
  }, [fetchConfig]);

  useEffect(() => {
    if (config !== null) {
      setLinearApiKey(config.linear_api_key ?? "");
      setLinearProjectId(config.linear_project_id ?? "");
      setRepoPath(config.repo_path ?? "");
      setAgentCommand(config.agent_command);
      setHookPort(config.hook_port);
    }
  }, [config]);

  const handleBrowse = useCallback(async () => {
    const selected = await open({ directory: true });
    if (selected !== null) {
      setRepoPath(selected);
    }
  }, []);

  const handleSave = useCallback(async () => {
    setFeedback(null);
    const next: Config = {
      linear_api_key: linearApiKey || null,
      linear_project_id: linearProjectId || null,
      repo_path: repoPath || null,
      agent_command: agentCommand,
      hook_port: hookPort,
    };
    try {
      await saveConfig(next);
      setFeedback({ kind: "success", message: "Settings saved." });
    } catch (e: unknown) {
      setFeedback({ kind: "error", message: String(e) });
    }
  }, [linearApiKey, linearProjectId, repoPath, agentCommand, hookPort, saveConfig]);

  useEffect(() => {
    if (configError !== null) {
      setFeedback({ kind: "error", message: configError });
    }
  }, [configError]);

  return (
    <div className="mx-auto max-w-2xl px-6 py-8">
      <div className="mb-6 flex items-center gap-3">
        <Settings className="h-6 w-6 text-text-secondary" />
        <div>
          <h1 className="text-xl font-semibold text-text-primary">Settings</h1>
          <p className="text-sm text-text-secondary">
            Configure credentials, repository path, and agent settings.
          </p>
        </div>
      </div>

      {feedback !== null && (
        <p
          className={cn(
            "mb-6 text-sm",
            feedback.kind === "success"
              ? "text-success"
              : "text-destructive",
          )}
        >
          {feedback.message}
        </p>
      )}

      <Card>
        {configLoading && config === null ? (
          <CardContent className="py-8">
            <p className="text-sm text-muted-foreground">
              Loading configuration...
            </p>
          </CardContent>
        ) : (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void handleSave();
            }}
          >
            <CardHeader>
              <CardTitle>Linear Integration</CardTitle>
              <CardDescription>
                Connect to your Linear workspace for issue tracking.
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-5">
              <div className="flex flex-col gap-2">
                <Label htmlFor="linear-api-key">Linear API Key</Label>
                <div className="flex gap-2">
                  <Input
                    id="linear-api-key"
                    type={showApiKey ? "text" : "password"}
                    value={linearApiKey}
                    onChange={(e) => {
                      setLinearApiKey(e.target.value);
                    }}
                    placeholder="lin_api_..."
                    className="flex-1"
                  />
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    onClick={() => {
                      setShowApiKey((prev) => !prev);
                    }}
                    aria-label={showApiKey ? "Hide API key" : "Show API key"}
                  >
                    {showApiKey ? <EyeOff /> : <Eye />}
                  </Button>
                </div>
              </div>

              <div className="flex flex-col gap-2">
                <Label htmlFor="linear-project-id">Linear Project ID</Label>
                <Input
                  id="linear-project-id"
                  type="text"
                  value={linearProjectId}
                  onChange={(e) => {
                    setLinearProjectId(e.target.value);
                  }}
                  placeholder="PRJ-123"
                />
              </div>
            </CardContent>

            <div className="px-6">
              <Separator />
            </div>

            <CardHeader>
              <CardTitle>Development</CardTitle>
              <CardDescription>
                Repository, agent command, and hook server settings.
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-5">
              <div className="flex flex-col gap-2">
                <Label htmlFor="repo-path">Repository Path</Label>
                <div className="flex gap-2">
                  <Input
                    id="repo-path"
                    type="text"
                    value={repoPath}
                    onChange={(e) => {
                      setRepoPath(e.target.value);
                    }}
                    placeholder="/path/to/repo"
                    className="flex-1"
                  />
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => {
                      void handleBrowse();
                    }}
                  >
                    <FolderOpen />
                    Browse
                  </Button>
                </div>
              </div>

              <div className="flex flex-col gap-2">
                <Label htmlFor="agent-command">Agent Command</Label>
                <Input
                  id="agent-command"
                  type="text"
                  value={agentCommand}
                  onChange={(e) => {
                    setAgentCommand(e.target.value);
                  }}
                  placeholder="claude"
                />
              </div>

              <div className="flex flex-col gap-2">
                <Label htmlFor="hook-port">Hook Port</Label>
                <Input
                  id="hook-port"
                  type="number"
                  value={hookPort}
                  onChange={(e) => {
                    const parsed = parseInt(e.target.value, 10);
                    if (!Number.isNaN(parsed)) {
                      setHookPort(parsed);
                    }
                  }}
                  placeholder="19876"
                />
              </div>
            </CardContent>

            <CardFooter className="justify-end">
              <Button type="submit" disabled={configLoading}>
                <Save />
                {configLoading ? "Saving..." : "Save"}
              </Button>
            </CardFooter>
          </form>
        )}
      </Card>
    </div>
  );
}
