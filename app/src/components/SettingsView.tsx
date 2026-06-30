import { useState, useEffect, useCallback } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "../store";
import type { Config } from "../bindings/Config";
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
import { Separator } from "@/components/ui/separator";
import {
  Save,
  FolderOpen,
  Eye,
  EyeOff,
  Settings,
  Undo2,
  Bot,
  Plug,
  Wrench,
  Palette,
} from "lucide-react";
import { cn } from "@/lib/utils";

type Feedback =
  | { readonly kind: "success"; readonly message: string }
  | { readonly kind: "error"; readonly message: string };

/** Model choices for the AI model selector. */
const MODEL_OPTIONS = [
  { value: "claude-sonnet-4-6", label: "Claude Sonnet 4.6" },
  { value: "claude-opus-4-6", label: "Claude Opus 4.6" },
  { value: "claude-haiku-4-5", label: "Claude Haiku 4.5" },
] as const;

/** IDE choices for the Development section. */
const IDE_OPTIONS = [
  { value: "cursor", label: "Cursor" },
  { value: "code", label: "VS Code" },
  { value: "zed", label: "Zed" },
  { value: "intellij", label: "IntelliJ" },
] as const;

/** App theme choices. */
const APP_THEME_OPTIONS = [
  { value: "dark", label: "Dark" },
  { value: "light", label: "Light" },
  { value: "system", label: "System" },
] as const;

export function SettingsView() {
  const config = useAppStore((s) => s.config);
  const configLoading = useAppStore((s) => s.configLoading);
  const configError = useAppStore((s) => s.configError);
  const fetchConfig = useAppStore((s) => s.fetchConfig);
  const saveConfig = useAppStore((s) => s.saveConfig);

  // --- AI fields ---
  const [anthropicApiKey, setAnthropicApiKey] = useState("");
  const [model, setModel] = useState("claude-sonnet-4-6");
  const [dailyBudgetUsd, setDailyBudgetUsd] = useState("");
  const [agentCommand, setAgentCommand] = useState("claude");

  // --- Integration fields ---
  const [linearApiKey, setLinearApiKey] = useState("");
  const [linearProjectId, setLinearProjectId] = useState("");
  const [githubToken, setGithubToken] = useState("");

  // --- Development fields ---
  const [repoPath, setRepoPath] = useState("");
  const [ideCommand, setIdeCommand] = useState("");
  const [hookPort, setHookPort] = useState(19876);

  // --- Appearance fields ---
  const [appTheme, setAppTheme] = useState("dark");
  const [editorTheme, setEditorTheme] = useState("vs-dark");
  const [terminalFont, setTerminalFont] = useState("");
  const [terminalFontSize, setTerminalFontSize] = useState(13);

  // --- UI state ---
  const [showAnthropicKey, setShowAnthropicKey] = useState(false);
  const [showLinearKey, setShowLinearKey] = useState(false);
  const [showGithubToken, setShowGithubToken] = useState(false);
  const [feedback, setFeedback] = useState<Feedback | null>(null);

  useEffect(() => {
    void fetchConfig();
  }, [fetchConfig]);

  // Populate form from loaded config.
  useEffect(() => {
    if (config !== null) {
      setAnthropicApiKey(config.anthropic_api_key ?? "");
      setModel(config.model ?? "claude-sonnet-4-6");
      setDailyBudgetUsd(
        config.daily_budget_usd !== null ? String(config.daily_budget_usd) : "",
      );
      setAgentCommand(config.agent_command);
      setLinearApiKey(config.linear_api_key ?? "");
      setLinearProjectId(config.linear_project_id ?? "");
      setGithubToken(config.github_token ?? "");
      setRepoPath(config.repo_path ?? "");
      setIdeCommand(config.ide_command ?? "");
      setHookPort(config.hook_port);
      setAppTheme(config.app_theme ?? "dark");
      setEditorTheme(config.editor_theme ?? "vs-dark");
      setTerminalFont(config.terminal_font ?? "");
      setTerminalFontSize(config.terminal_font_size ?? 13);
    }
  }, [config]);

  const handleBrowse = useCallback(async () => {
    const selected = await open({ directory: true });
    if (selected !== null) {
      setRepoPath(selected);
    }
  }, []);

  const handleDiscard = useCallback(() => {
    if (config !== null) {
      setAnthropicApiKey(config.anthropic_api_key ?? "");
      setModel(config.model ?? "claude-sonnet-4-6");
      setDailyBudgetUsd(
        config.daily_budget_usd !== null ? String(config.daily_budget_usd) : "",
      );
      setAgentCommand(config.agent_command);
      setLinearApiKey(config.linear_api_key ?? "");
      setLinearProjectId(config.linear_project_id ?? "");
      setGithubToken(config.github_token ?? "");
      setRepoPath(config.repo_path ?? "");
      setIdeCommand(config.ide_command ?? "");
      setHookPort(config.hook_port);
      setAppTheme(config.app_theme ?? "dark");
      setEditorTheme(config.editor_theme ?? "vs-dark");
      setTerminalFont(config.terminal_font ?? "");
      setTerminalFontSize(config.terminal_font_size ?? 13);
    }
    setFeedback(null);
  }, [config]);

  const handleSave = useCallback(async () => {
    setFeedback(null);
    const budgetParsed = parseFloat(dailyBudgetUsd);
    const next: Config = {
      // AI
      anthropic_api_key: anthropicApiKey || null,
      model: model || null,
      daily_budget_usd: !Number.isNaN(budgetParsed) ? budgetParsed : null,
      agent_command: agentCommand,
      // Integrations
      linear_api_key: linearApiKey || null,
      linear_project_id: linearProjectId || null,
      github_token: githubToken || null,
      // Development
      repo_path: repoPath || null,
      ide_command: ideCommand || null,
      hook_port: hookPort,
      // Appearance
      app_theme: appTheme || null,
      editor_theme: editorTheme || null,
      terminal_font: terminalFont || null,
      terminal_font_size: terminalFontSize > 0 ? terminalFontSize : null,
    };
    try {
      await saveConfig(next);
      setFeedback({ kind: "success", message: "Settings saved." });
    } catch (e: unknown) {
      setFeedback({ kind: "error", message: String(e) });
    }
  }, [
    anthropicApiKey,
    model,
    dailyBudgetUsd,
    agentCommand,
    linearApiKey,
    linearProjectId,
    githubToken,
    repoPath,
    ideCommand,
    hookPort,
    appTheme,
    editorTheme,
    terminalFont,
    terminalFontSize,
    saveConfig,
  ]);

  useEffect(() => {
    if (configError !== null) {
      setFeedback({ kind: "error", message: configError });
    }
  }, [configError]);

  if (configLoading && config === null) {
    return (
      <div className="mx-auto max-w-2xl px-6 py-8">
        <p className="text-sm text-muted-foreground">
          Loading configuration...
        </p>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-2xl px-6 py-8">
      {/* Page header */}
      <div className="mb-6 flex items-center gap-3">
        <Settings className="h-6 w-6 text-muted-foreground" />
        <div>
          <h1 className="text-xl font-semibold text-foreground">Settings</h1>
          <p className="text-sm text-muted-foreground">
            Configure AI, integrations, development, and appearance.
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

      <form
        onSubmit={(e) => {
          e.preventDefault();
          void handleSave();
        }}
        className="flex flex-col gap-6"
      >
        {/* ----------------------------------------------------------------- */}
        {/* AI Section                                                        */}
        {/* ----------------------------------------------------------------- */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <Bot className="h-4 w-4 text-muted-foreground" />
              <CardTitle>AI</CardTitle>
            </div>
            <CardDescription>
              Anthropic credentials, model selection, and agent settings.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-5">
            {/* Anthropic API Key */}
            <div className="flex flex-col gap-2">
              <Label htmlFor="anthropic-api-key">Anthropic API Key</Label>
              <div className="flex gap-2">
                <Input
                  id="anthropic-api-key"
                  type={showAnthropicKey ? "text" : "password"}
                  value={anthropicApiKey}
                  onChange={(e) => {
                    setAnthropicApiKey(e.target.value);
                  }}
                  placeholder="sk-ant-..."
                  className="flex-1"
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  onClick={() => {
                    setShowAnthropicKey((prev) => !prev);
                  }}
                  aria-label={showAnthropicKey ? "Hide API key" : "Show API key"}
                >
                  {showAnthropicKey ? <EyeOff /> : <Eye />}
                </Button>
              </div>
            </div>

            {/* Model */}
            <div className="flex flex-col gap-2">
              <Label htmlFor="model">Model</Label>
              <select
                id="model"
                value={model}
                onChange={(e) => {
                  setModel(e.target.value);
                }}
                className="h-9 rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {MODEL_OPTIONS.map((opt) => (
                  <option key={opt.value} value={opt.value}>
                    {opt.label}
                  </option>
                ))}
              </select>
            </div>

            {/* Daily budget */}
            <div className="flex flex-col gap-2">
              <Label htmlFor="daily-budget">Daily Budget Cap (USD)</Label>
              <Input
                id="daily-budget"
                type="number"
                step="0.01"
                min="0"
                value={dailyBudgetUsd}
                onChange={(e) => {
                  setDailyBudgetUsd(e.target.value);
                }}
                placeholder="No limit"
              />
              <p className="text-xs text-muted-foreground">
                Leave empty for unlimited.
              </p>
            </div>

            {/* Agent command */}
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
          </CardContent>
        </Card>

        {/* ----------------------------------------------------------------- */}
        {/* Integrations Section                                              */}
        {/* ----------------------------------------------------------------- */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <Plug className="h-4 w-4 text-muted-foreground" />
              <CardTitle>Integrations</CardTitle>
            </div>
            <CardDescription>
              Linear and GitHub credentials.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-5">
            {/* Linear API Key */}
            <div className="flex flex-col gap-2">
              <Label htmlFor="linear-api-key">Linear API Key</Label>
              <div className="flex gap-2">
                <Input
                  id="linear-api-key"
                  type={showLinearKey ? "text" : "password"}
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
                    setShowLinearKey((prev) => !prev);
                  }}
                  aria-label={showLinearKey ? "Hide API key" : "Show API key"}
                >
                  {showLinearKey ? <EyeOff /> : <Eye />}
                </Button>
              </div>
            </div>

            {/* Linear Project ID */}
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

            <Separator />

            {/* GitHub Token */}
            <div className="flex flex-col gap-2">
              <Label htmlFor="github-token">GitHub Token</Label>
              <div className="flex gap-2">
                <Input
                  id="github-token"
                  type={showGithubToken ? "text" : "password"}
                  value={githubToken}
                  onChange={(e) => {
                    setGithubToken(e.target.value);
                  }}
                  placeholder="ghp_..."
                  className="flex-1"
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  onClick={() => {
                    setShowGithubToken((prev) => !prev);
                  }}
                  aria-label={showGithubToken ? "Hide token" : "Show token"}
                >
                  {showGithubToken ? <EyeOff /> : <Eye />}
                </Button>
              </div>
            </div>
          </CardContent>
        </Card>

        {/* ----------------------------------------------------------------- */}
        {/* Development Section                                               */}
        {/* ----------------------------------------------------------------- */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <Wrench className="h-4 w-4 text-muted-foreground" />
              <CardTitle>Development</CardTitle>
            </div>
            <CardDescription>
              Repository path, IDE, and hook server settings.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-5">
            {/* Repository path */}
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

            {/* IDE */}
            <div className="flex flex-col gap-2">
              <Label htmlFor="ide-command">IDE</Label>
              <select
                id="ide-command"
                value={
                  IDE_OPTIONS.some((o) => o.value === ideCommand)
                    ? ideCommand
                    : "__custom__"
                }
                onChange={(e) => {
                  if (e.target.value !== "__custom__") {
                    setIdeCommand(e.target.value);
                  }
                }}
                className="h-9 rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {IDE_OPTIONS.map((opt) => (
                  <option key={opt.value} value={opt.value}>
                    {opt.label}
                  </option>
                ))}
                {!IDE_OPTIONS.some((o) => o.value === ideCommand) && ideCommand !== "" && (
                  <option value="__custom__">
                    Custom: {ideCommand}
                  </option>
                )}
              </select>
              {/* Allow typing a custom IDE command */}
              {!IDE_OPTIONS.some((o) => o.value === ideCommand) && (
                <Input
                  type="text"
                  value={ideCommand}
                  onChange={(e) => {
                    setIdeCommand(e.target.value);
                  }}
                  placeholder="Custom IDE command..."
                  className="mt-1"
                />
              )}
            </div>

            {/* Hook port */}
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
        </Card>

        {/* ----------------------------------------------------------------- */}
        {/* Appearance Section                                                */}
        {/* ----------------------------------------------------------------- */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <Palette className="h-4 w-4 text-muted-foreground" />
              <CardTitle>Appearance</CardTitle>
            </div>
            <CardDescription>
              Theme, font, and editor settings.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-5">
            {/* App theme (radio group) */}
            <div className="flex flex-col gap-2">
              <Label>App Theme</Label>
              <div className="flex gap-4">
                {APP_THEME_OPTIONS.map((opt) => (
                  <label
                    key={opt.value}
                    className="flex items-center gap-2 cursor-pointer text-sm"
                  >
                    <input
                      type="radio"
                      name="app-theme"
                      value={opt.value}
                      checked={appTheme === opt.value}
                      onChange={() => {
                        setAppTheme(opt.value);
                        if (opt.value === "dark") {
                          document.documentElement.classList.add("dark");
                        } else {
                          document.documentElement.classList.remove("dark");
                        }
                      }}
                      className="accent-primary"
                    />
                    {opt.label}
                  </label>
                ))}
              </div>
            </div>

            {/* Editor theme (select) */}
            <div className="flex flex-col gap-2">
              <Label htmlFor="editor-theme">Editor Theme</Label>
              <select
                id="editor-theme"
                value={editorTheme}
                onChange={(e) => {
                  setEditorTheme(e.target.value);
                }}
                className="h-9 rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {MONACO_THEMES.map((theme) => (
                  <option key={theme.id} value={theme.id}>
                    {theme.label}
                  </option>
                ))}
              </select>
            </div>

            {/* Terminal font */}
            <div className="flex flex-col gap-2">
              <Label htmlFor="terminal-font">Terminal Font</Label>
              <Input
                id="terminal-font"
                type="text"
                value={terminalFont}
                onChange={(e) => {
                  setTerminalFont(e.target.value);
                }}
                placeholder="SF Mono"
              />
            </div>

            {/* Terminal font size */}
            <div className="flex flex-col gap-2">
              <Label htmlFor="terminal-font-size">Terminal Font Size</Label>
              <Input
                id="terminal-font-size"
                type="number"
                min={8}
                max={72}
                value={terminalFontSize}
                onChange={(e) => {
                  const parsed = parseInt(e.target.value, 10);
                  if (!Number.isNaN(parsed)) {
                    setTerminalFontSize(parsed);
                  }
                }}
                placeholder="13"
              />
            </div>
          </CardContent>
        </Card>

        {/* ----------------------------------------------------------------- */}
        {/* Actions                                                           */}
        {/* ----------------------------------------------------------------- */}
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
