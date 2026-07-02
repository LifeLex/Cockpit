import { useState, useEffect, useCallback, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "../store";
import type { Config } from "../bindings/Config";
import type { SkillsGithub } from "../bindings/SkillsGithub";
import { MONACO_THEMES } from "@/lib/monaco-themes";
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
import {
  Save,
  FolderOpen,
  Settings,
  Undo2,
  Bot,
  GitBranch,
  Palette,
  FolderTree,
  ListTree,
  SlidersHorizontal,
  ChevronDown,
  Bell,
} from "lucide-react";
import { cn } from "@/lib/utils";

/** Transient banner shown after a save attempt. */
type Feedback =
  | { readonly kind: "success"; readonly message: string }
  | { readonly kind: "error"; readonly message: string };

/** App theme choices, mirroring the `app_theme` values understood by the store. */
const APP_THEME_OPTIONS = [
  { value: "dark", label: "Dark" },
  { value: "light", label: "Light" },
  { value: "system", label: "System" },
] as const;

/** Shared field styling for the tokened `<select>` control. */
const SELECT_CLASS =
  "h-9 w-full appearance-none rounded-md border border-border bg-card px-3 py-1 text-sm text-foreground shadow-sm transition-colors hover:border-ring/60 focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50";

/**
 * One labelled settings row: a description column on the left and the control
 * on the right. Keeps every field in a consistent grid so sections read as a
 * tidy form rather than a stack of ad-hoc inputs.
 */
function SettingsRow({
  label,
  htmlFor,
  description,
  children,
}: {
  readonly label: string;
  readonly htmlFor?: string;
  readonly description?: ReactNode;
  readonly children: ReactNode;
}) {
  return (
    <div className="grid grid-cols-1 gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)] sm:items-start sm:gap-6">
      <div className="flex flex-col gap-1">
        <Label htmlFor={htmlFor}>{label}</Label>
        {description !== undefined && (
          <p className="text-xs leading-relaxed text-muted-foreground">
            {description}
          </p>
        )}
      </div>
      <div className="flex flex-col gap-1.5">{children}</div>
    </div>
  );
}

/**
 * Application settings screen.
 *
 * Loads the persisted [`Config`] via `get_config`, lets the user edit a curated
 * subset of fields, and saves by spreading the loaded config and overriding
 * only the edited fields. It never constructs a partial `Config` literal, which
 * is what previously caused the type to drift when new fields were added.
 */
export function SettingsView() {
  const config = useAppStore((s) => s.config);
  const configLoading = useAppStore((s) => s.configLoading);
  const configError = useAppStore((s) => s.configError);
  const fetchConfig = useAppStore((s) => s.fetchConfig);
  const saveConfig = useAppStore((s) => s.saveConfig);

  // --- Repository ---
  const [repoPath, setRepoPath] = useState("");

  // --- Linear (optional) ---
  const [linearApiKey, setLinearApiKey] = useState("");
  const [linearProjectId, setLinearProjectId] = useState("");

  // --- Agent ---
  const [agentCommand, setAgentCommand] = useState("claude");
  const [maxParallelAgents, setMaxParallelAgents] = useState(3);

  // --- Skills sync (owner/repo/branch/path/auto_sync) ---
  const [skillsOwner, setSkillsOwner] = useState("");
  const [skillsRepo, setSkillsRepo] = useState("");
  const [skillsBranch, setSkillsBranch] = useState("main");
  const [skillsPath, setSkillsPath] = useState("");
  const [skillsAutoSync, setSkillsAutoSync] = useState(false);

  // --- Appearance ---
  const [appTheme, setAppTheme] = useState("dark");
  const [editorTheme, setEditorTheme] = useState("vs-dark");

  // --- Background refresh & notifications ---
  // String-backed so an empty field (disabled) round-trips cleanly to `null`.
  const [notifyPollSecs, setNotifyPollSecs] = useState("");

  // --- Advanced ---
  const [hookPort, setHookPort] = useState(19876);

  // --- UI state ---
  const [feedback, setFeedback] = useState<Feedback | null>(null);

  useEffect(() => {
    void fetchConfig();
  }, [fetchConfig]);

  // Populate the form from the loaded config. Declared as a callback so both the
  // initial load effect and the Discard button share one source of truth.
  const populateFromConfig = useCallback((c: Config) => {
    setRepoPath(c.repo_path ?? "");
    setLinearApiKey(c.linear_api_key ?? "");
    setLinearProjectId(c.linear_project_id ?? "");
    setAgentCommand(c.agent_command);
    setMaxParallelAgents(c.max_parallel_agents);
    setSkillsOwner(c.skills_github?.owner ?? "");
    setSkillsRepo(c.skills_github?.repo ?? "");
    setSkillsBranch(c.skills_github?.branch ?? "main");
    setSkillsPath(c.skills_github?.path ?? "");
    setSkillsAutoSync(c.skills_github?.auto_sync ?? false);
    setAppTheme(c.app_theme ?? "dark");
    setEditorTheme(c.editor_theme ?? "vs-dark");
    setNotifyPollSecs(
      c.notify_poll_secs === null ? "" : String(c.notify_poll_secs),
    );
    setHookPort(c.hook_port);
  }, []);

  useEffect(() => {
    if (config !== null) {
      populateFromConfig(config);
    }
  }, [config, populateFromConfig]);

  const handleBrowse = useCallback(async () => {
    const selected = await open({ directory: true });
    if (selected !== null) {
      setRepoPath(selected);
    }
  }, []);

  const handleDiscard = useCallback(() => {
    if (config !== null) {
      populateFromConfig(config);
    }
    setFeedback(null);
  }, [config, populateFromConfig]);

  const handleSave = useCallback(async () => {
    if (config === null) {
      return;
    }
    setFeedback(null);

    // A skills source is only persisted when an owner and repo are given;
    // otherwise the whole source is cleared to null (local-only skills).
    const skillsGithub: SkillsGithub | null =
      skillsOwner.trim() !== "" && skillsRepo.trim() !== ""
        ? {
            owner: skillsOwner.trim(),
            repo: skillsRepo.trim(),
            branch: skillsBranch.trim() !== "" ? skillsBranch.trim() : "main",
            path: skillsPath.trim(),
            auto_sync: skillsAutoSync,
          }
        : null;

    // Background polling: empty / 0 / non-positive disables it (null); any
    // positive value is kept (the backend floors it at 30s).
    const parsedPoll = Number.parseInt(notifyPollSecs.trim(), 10);
    const notifyPoll =
      Number.isNaN(parsedPoll) || parsedPoll <= 0 ? null : parsedPoll;

    // Spread the loaded config so kept-but-unedited fields (e.g. ide_command,
    // agent_prompts) survive round-trips, then override only edited fields.
    const next: Config = {
      ...config,
      repo_path: repoPath !== "" ? repoPath : null,
      linear_api_key: linearApiKey !== "" ? linearApiKey : null,
      linear_project_id: linearProjectId !== "" ? linearProjectId : null,
      agent_command: agentCommand !== "" ? agentCommand : "claude",
      max_parallel_agents: maxParallelAgents > 0 ? maxParallelAgents : 1,
      skills_github: skillsGithub,
      app_theme: appTheme !== "" ? appTheme : null,
      editor_theme: editorTheme !== "" ? editorTheme : null,
      notify_poll_secs: notifyPoll,
      hook_port: hookPort,
    };

    try {
      await saveConfig(next);
      setFeedback({ kind: "success", message: "Settings saved." });
    } catch (e: unknown) {
      setFeedback({ kind: "error", message: String(e) });
    }
  }, [
    config,
    repoPath,
    linearApiKey,
    linearProjectId,
    agentCommand,
    maxParallelAgents,
    skillsOwner,
    skillsRepo,
    skillsBranch,
    skillsPath,
    skillsAutoSync,
    appTheme,
    editorTheme,
    notifyPollSecs,
    hookPort,
    saveConfig,
  ]);

  useEffect(() => {
    if (configError !== null) {
      setFeedback({ kind: "error", message: configError });
    }
  }, [configError]);

  if (configLoading && config === null) {
    return (
      <div className="mx-auto max-w-3xl px-6 py-8">
        <p className="text-sm text-muted-foreground">
          Loading configuration...
        </p>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-3xl px-6 py-8">
      {/* Page header */}
      <div className="mb-6 flex items-center gap-3">
        <Settings className="h-6 w-6 text-muted-foreground" />
        <div>
          <h1 className="text-xl font-semibold text-foreground">Settings</h1>
          <p className="text-sm text-muted-foreground">
            Repository, agent, skills, and appearance.
          </p>
        </div>
      </div>

      {feedback !== null && (
        <p
          className={cn(
            "mb-6 text-sm",
            feedback.kind === "success" ? "text-success" : "text-destructive",
          )}
        >
          {feedback.message}
        </p>
      )}

      <form
        onSubmit={(e) => {
          e.preventDefault();
          void handleSave();
        }}
        className="flex flex-col gap-6"
      >
        {/* --------------------------------------------------------------- */}
        {/* Repository                                                      */}
        {/* --------------------------------------------------------------- */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <FolderTree className="h-4 w-4 text-muted-foreground" />
              <CardTitle>Repository</CardTitle>
            </div>
            <CardDescription>
              The git repository cockpit manages.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-6">
            <SettingsRow
              label="Repository path"
              htmlFor="repo-path"
              description="Absolute path to the checkout cockpit works against."
            >
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
            </SettingsRow>
          </CardContent>
        </Card>

        {/* --------------------------------------------------------------- */}
        {/* Worktrees & logs (read-only)                                    */}
        {/* --------------------------------------------------------------- */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <ListTree className="h-4 w-4 text-muted-foreground" />
              <CardTitle>Worktrees &amp; logs</CardTitle>
            </div>
            <CardDescription>
              Where cockpit keeps its working state.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <p className="text-sm text-muted-foreground">
              Agent worktrees, run logs, plans, and installed skills live under{" "}
              <code className="rounded bg-muted px-1 py-0.5 text-xs text-foreground">
                $HOME/.cockpit
              </code>{" "}
              (
              <code className="text-xs">worktrees/</code>,{" "}
              <code className="text-xs">logs/</code>,{" "}
              <code className="text-xs">plans/</code>,{" "}
              <code className="text-xs">skills/</code>). These paths are managed
              by cockpit and are not configurable.
            </p>
          </CardContent>
        </Card>

        {/* --------------------------------------------------------------- */}
        {/* Linear (optional)                                               */}
        {/* --------------------------------------------------------------- */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <GitBranch className="h-4 w-4 text-muted-foreground" />
              <CardTitle>Linear</CardTitle>
            </div>
            <CardDescription>
              Optional. Linear is one project source; cockpit works without it.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-6">
            <SettingsRow
              label="Linear API key"
              htmlFor="linear-api-key"
              description="Personal API key used to read your Linear projects and issues."
            >
              <Input
                id="linear-api-key"
                type="password"
                value={linearApiKey}
                onChange={(e) => {
                  setLinearApiKey(e.target.value);
                }}
                placeholder="lin_api_..."
              />
            </SettingsRow>

            <SettingsRow
              label="Linear project ID"
              htmlFor="linear-project-id"
              description="Default project cockpit opens when using Linear as a source."
            >
              <Input
                id="linear-project-id"
                type="text"
                value={linearProjectId}
                onChange={(e) => {
                  setLinearProjectId(e.target.value);
                }}
                placeholder="PRJ-123"
              />
            </SettingsRow>
          </CardContent>
        </Card>

        {/* --------------------------------------------------------------- */}
        {/* Agent                                                           */}
        {/* --------------------------------------------------------------- */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <Bot className="h-4 w-4 text-muted-foreground" />
              <CardTitle>Agent</CardTitle>
            </div>
            <CardDescription>
              Cockpit uses your Claude Code login (<code>claude</code> on your
              PATH). No API key needed.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-6">
            <SettingsRow
              label="Agent command"
              htmlFor="agent-command"
              description="Executable cockpit spawns to run the agent."
            >
              <Input
                id="agent-command"
                type="text"
                value={agentCommand}
                onChange={(e) => {
                  setAgentCommand(e.target.value);
                }}
                placeholder="claude"
              />
            </SettingsRow>

            <SettingsRow
              label="Max parallel agents"
              htmlFor="max-parallel-agents"
              description="Upper bound on implementer agents run at once during a plan fan-out."
            >
              <Input
                id="max-parallel-agents"
                type="number"
                min={1}
                max={64}
                value={maxParallelAgents}
                onChange={(e) => {
                  const parsed = parseInt(e.target.value, 10);
                  if (!Number.isNaN(parsed)) {
                    setMaxParallelAgents(parsed);
                  }
                }}
                placeholder="3"
              />
            </SettingsRow>
          </CardContent>
        </Card>

        {/* --------------------------------------------------------------- */}
        {/* Skills sync                                                     */}
        {/* --------------------------------------------------------------- */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <ListTree className="h-4 w-4 text-muted-foreground" />
              <CardTitle>Skills sync</CardTitle>
            </div>
            <CardDescription>
              The GitHub source cockpit syncs installable skills from. Leave
              owner or repo blank to keep skills local-only.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-6">
            <SettingsRow
              label="Owner"
              htmlFor="skills-owner"
              description="Repository owner (user or org)."
            >
              <Input
                id="skills-owner"
                type="text"
                value={skillsOwner}
                onChange={(e) => {
                  setSkillsOwner(e.target.value);
                }}
                placeholder="acme"
              />
            </SettingsRow>

            <SettingsRow label="Repository" htmlFor="skills-repo">
              <Input
                id="skills-repo"
                type="text"
                value={skillsRepo}
                onChange={(e) => {
                  setSkillsRepo(e.target.value);
                }}
                placeholder="conventions"
              />
            </SettingsRow>

            <SettingsRow
              label="Branch"
              htmlFor="skills-branch"
              description="Branch to sync from."
            >
              <Input
                id="skills-branch"
                type="text"
                value={skillsBranch}
                onChange={(e) => {
                  setSkillsBranch(e.target.value);
                }}
                placeholder="main"
              />
            </SettingsRow>

            <SettingsRow
              label="Path"
              htmlFor="skills-path"
              description="Directory in the repo holding one skill per subdirectory."
            >
              <Input
                id="skills-path"
                type="text"
                value={skillsPath}
                onChange={(e) => {
                  setSkillsPath(e.target.value);
                }}
                placeholder="skills"
              />
            </SettingsRow>

            <SettingsRow
              label="Auto-sync"
              description="Sync skills automatically on relevant triggers."
            >
              <div className="flex h-9 items-center">
                <Switch
                  checked={skillsAutoSync}
                  onCheckedChange={(checked) => {
                    setSkillsAutoSync(checked);
                  }}
                  aria-label="Toggle automatic skills sync"
                />
              </div>
            </SettingsRow>
          </CardContent>
        </Card>

        {/* --------------------------------------------------------------- */}
        {/* Appearance                                                      */}
        {/* --------------------------------------------------------------- */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <Palette className="h-4 w-4 text-muted-foreground" />
              <CardTitle>Appearance</CardTitle>
            </div>
            <CardDescription>
              Application and code editor theming.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-6">
            <SettingsRow label="App theme" description="Overall color scheme.">
              <div
                role="radiogroup"
                aria-label="App theme"
                className="inline-flex w-fit items-center gap-1 rounded-lg border border-border bg-muted p-[3px]"
              >
                {APP_THEME_OPTIONS.map((opt) => {
                  const selected = appTheme === opt.value;
                  return (
                    <button
                      key={opt.value}
                      type="button"
                      role="radio"
                      aria-checked={selected}
                      onClick={() => {
                        setAppTheme(opt.value);
                        if (opt.value === "dark") {
                          document.documentElement.classList.add("dark");
                        } else if (opt.value === "light") {
                          document.documentElement.classList.remove("dark");
                        }
                      }}
                      className={cn(
                        "cursor-pointer rounded-md border border-transparent px-3 py-1 text-sm font-medium transition-colors focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50",
                        selected
                          ? "bg-card text-foreground shadow-sm"
                          : "bg-transparent text-muted-foreground hover:text-foreground",
                      )}
                    >
                      {opt.label}
                    </button>
                  );
                })}
              </div>
            </SettingsRow>

            <SettingsRow
              label="Editor theme"
              htmlFor="editor-theme"
              description="Theme used by the Monaco diff editor."
            >
              <div className="relative">
                <select
                  id="editor-theme"
                  value={editorTheme}
                  onChange={(e) => {
                    setEditorTheme(e.target.value);
                  }}
                  className={cn(SELECT_CLASS, "pr-9")}
                >
                  {MONACO_THEMES.map((theme) => (
                    <option key={theme.id} value={theme.id}>
                      {theme.label}
                    </option>
                  ))}
                </select>
                <ChevronDown
                  className="pointer-events-none absolute right-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground"
                  aria-hidden="true"
                />
              </div>
            </SettingsRow>
          </CardContent>
        </Card>

        {/* --------------------------------------------------------------- */}
        {/* Background refresh & notifications                              */}
        {/* --------------------------------------------------------------- */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <Bell className="h-4 w-4 text-muted-foreground" />
              <CardTitle>Background refresh &amp; notifications</CardTitle>
            </div>
            <CardDescription>
              Poll GitHub in the background and notify you when a PR becomes
              reviewable.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-6">
            <SettingsRow
              label="Poll interval (seconds)"
              htmlFor="notify-poll-secs"
              description="Polls GitHub every N seconds (min 30) and notifies on new review requests, CI going green, and new commits. Leave empty or 0 to disable."
            >
              <Input
                id="notify-poll-secs"
                type="number"
                min={30}
                value={notifyPollSecs}
                onChange={(e) => {
                  setNotifyPollSecs(e.target.value);
                }}
                placeholder="90"
              />
            </SettingsRow>
          </CardContent>
        </Card>

        {/* --------------------------------------------------------------- */}
        {/* Advanced                                                        */}
        {/* --------------------------------------------------------------- */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <SlidersHorizontal className="h-4 w-4 text-muted-foreground" />
              <CardTitle>Advanced</CardTitle>
            </div>
            <CardDescription>Low-level runtime settings.</CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-6">
            <SettingsRow
              label="Hook port"
              htmlFor="hook-port"
              description="Local port the Stop-hook listener binds to."
            >
              <Input
                id="hook-port"
                type="number"
                min={1}
                max={65535}
                value={hookPort}
                onChange={(e) => {
                  const parsed = parseInt(e.target.value, 10);
                  if (!Number.isNaN(parsed)) {
                    setHookPort(parsed);
                  }
                }}
                placeholder="19876"
              />
            </SettingsRow>
          </CardContent>
        </Card>

        {/* --------------------------------------------------------------- */}
        {/* Actions                                                         */}
        {/* --------------------------------------------------------------- */}
        <div className="flex justify-end gap-3 pb-8">
          <Button
            type="button"
            variant="outline"
            onClick={handleDiscard}
            disabled={configLoading}
          >
            <Undo2 />
            Discard
          </Button>
          <Button type="submit" disabled={configLoading}>
            <Save />
            {configLoading ? "Saving..." : "Save"}
          </Button>
        </div>
      </form>
    </div>
  );
}
