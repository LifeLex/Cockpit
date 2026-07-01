import { useState, useEffect, useCallback } from "react";
import { useAppStore } from "../store";
import type { AgentMode } from "../bindings/AgentMode";
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
} from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { Bot, Save, Undo2, Loader2 } from "lucide-react";
import { cn } from "@/lib/utils";

function assertNever(x: never): never {
  throw new Error(`unreachable: ${String(x)}`);
}

/** The agent modes, in display order, as an exhaustive literal table. */
const AGENT_MODES = ["Implement", "Plan", "Fix", "Restack"] as const;

/** Human-facing label for an agent mode. */
function modeLabel(mode: AgentMode): string {
  switch (mode) {
    case "Implement":
      return "Implement";
    case "Plan":
      return "Plan";
    case "Fix":
      return "Fix";
    case "Restack":
      return "Restack";
    default:
      return assertNever(mode);
  }
}

/** One-line description of what each agent mode does. */
function modeDescription(mode: AgentMode): string {
  switch (mode) {
    case "Implement":
      return "Runs when a plan is approved to build each PR in the batch.";
    case "Plan":
      return "Revises the project plan during the plan gate's rework loop.";
    case "Fix":
      return "Applies your diff-gate comments and pushes the rework.";
    case "Restack":
      return "Rebases a descendant PR after its base branch changed.";
    default:
      return assertNever(mode);
  }
}

/** Narrow an unknown tab value to an `AgentMode`, or null if unrecognized. */
function toAgentMode(value: unknown): AgentMode | null {
  switch (value) {
    case "Implement":
    case "Plan":
    case "Fix":
    case "Restack":
      return value;
    default:
      return null;
  }
}

/**
 * Per-`AgentMode` prompt-fragment editor.
 *
 * For the selected mode it loads the stored override and the builtin default.
 * The custom fragment (a `<textarea>`) is injected verbatim into that agent's
 * prompt; the builtin is shown as the placeholder and can be restored by
 * clearing the override ("Reset to default").
 */
export function AgentEditor() {
  const getAgentPrompt = useAppStore((s) => s.getAgentPrompt);
  const getBuiltinAgentPrompt = useAppStore((s) => s.getBuiltinAgentPrompt);
  const saveAgentPrompt = useAppStore((s) => s.saveAgentPrompt);

  const [mode, setMode] = useState<AgentMode>("Implement");
  const [builtin, setBuiltin] = useState("");
  const [saved, setSaved] = useState("");
  const [draft, setDraft] = useState("");
  const [loading, setLoading] = useState(false);
  const [savedFlash, setSavedFlash] = useState(false);

  // Load the override + builtin whenever the selected mode changes.
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setSavedFlash(false);
    void (async () => {
      const [override, defaultText] = await Promise.all([
        getAgentPrompt(mode),
        getBuiltinAgentPrompt(mode),
      ]);
      if (cancelled) return;
      const overrideText = override ?? "";
      setBuiltin(defaultText ?? "");
      setSaved(overrideText);
      setDraft(overrideText);
      setLoading(false);
    })();
    return () => {
      cancelled = true;
    };
  }, [mode, getAgentPrompt, getBuiltinAgentPrompt]);

  const dirty = draft !== saved;

  const handleTabChange = useCallback((value: unknown) => {
    const next = toAgentMode(value);
    if (next !== null) {
      setMode(next);
    }
  }, []);

  const handleSave = useCallback(async () => {
    await saveAgentPrompt(mode, draft);
    setSaved(draft);
    setSavedFlash(true);
  }, [mode, draft, saveAgentPrompt]);

  const handleReset = useCallback(async () => {
    // Clearing the override (empty text) restores the builtin default.
    await saveAgentPrompt(mode, "");
    setSaved("");
    setDraft("");
    setSavedFlash(true);
  }, [mode, saveAgentPrompt]);

  return (
    <div className="mx-auto max-w-3xl px-6 py-8">
      <div className="mb-6 flex items-center gap-3">
        <Bot className="h-6 w-6 text-muted-foreground" />
        <div>
          <h1 className="text-xl font-semibold text-foreground">Agents</h1>
          <p className="text-sm text-muted-foreground">
            Customize the prompt fragment for each agent mode.
          </p>
        </div>
      </div>

      <Tabs value={mode} onValueChange={handleTabChange}>
        <TabsList className="mb-6">
          {AGENT_MODES.map((m) => (
            <TabsTrigger key={m} value={m}>
              {modeLabel(m)}
            </TabsTrigger>
          ))}
        </TabsList>

        {AGENT_MODES.map((m) => (
          <TabsContent key={m} value={m}>
            <Card>
              <CardHeader>
                <div className="flex items-center justify-between gap-3">
                  <div>
                    <CardTitle>{modeLabel(m)} prompt</CardTitle>
                    <CardDescription>{modeDescription(m)}</CardDescription>
                  </div>
                  {dirty && (
                    <span className="shrink-0 text-xs font-medium text-warning">
                      Unsaved changes
                    </span>
                  )}
                </div>
              </CardHeader>
              <CardContent className="flex flex-col gap-4">
                <p className="text-xs leading-relaxed text-muted-foreground">
                  This text is injected <strong>verbatim</strong> into the{" "}
                  {modeLabel(m).toLowerCase()} agent's prompt. Leave it empty to
                  use the builtin default shown as the placeholder.
                </p>

                {loading ? (
                  <p className="text-sm text-muted-foreground">Loading...</p>
                ) : (
                  <Textarea
                    value={draft}
                    onChange={(e) => {
                      setDraft(e.target.value);
                      setSavedFlash(false);
                    }}
                    placeholder={
                      builtin !== ""
                        ? builtin
                        : "Add a custom prompt fragment..."
                    }
                    spellCheck={false}
                    className="min-h-[20rem] font-mono text-xs leading-relaxed"
                  />
                )}

                <div className="flex items-center justify-between gap-3">
                  <span
                    className={cn(
                      "text-sm text-success transition-opacity",
                      savedFlash ? "opacity-100" : "opacity-0",
                    )}
                  >
                    Saved.
                  </span>
                  <div className="flex items-center gap-3">
                    <Button
                      variant="outline"
                      onClick={() => {
                        void handleReset();
                      }}
                      disabled={loading || saved === ""}
                    >
                      <Undo2 />
                      Reset to default
                    </Button>
                    <Button
                      onClick={() => {
                        void handleSave();
                      }}
                      disabled={loading || !dirty}
                    >
                      {loading ? (
                        <Loader2 className="animate-spin" />
                      ) : (
                        <Save />
                      )}
                      Save
                    </Button>
                  </div>
                </div>
              </CardContent>
            </Card>
          </TabsContent>
        ))}
      </Tabs>
    </div>
  );
}
