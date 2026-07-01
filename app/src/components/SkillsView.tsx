import { useState, useEffect, useCallback } from "react";
import { useAppStore } from "../store";
import type { Skill } from "../bindings/Skill";
import type { SkillSource } from "../bindings/SkillSource";
import type { SyncReport } from "../bindings/SyncReport";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Textarea } from "@/components/ui/textarea";
import { EmptyState } from "./EmptyState";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import {
  Blocks,
  Plus,
  Save,
  Trash2,
  RefreshCw,
  Loader2,
} from "lucide-react";
import { cn } from "@/lib/utils";

/** Editor selection: an existing skill by name, a brand-new draft, or none. */
type Selection =
  | { readonly kind: "none" }
  | { readonly kind: "existing"; readonly name: string }
  | { readonly kind: "new" };

/** Transient inline feedback shown after a sync/save action. */
type Feedback =
  | { readonly kind: "success"; readonly message: string }
  | { readonly kind: "error"; readonly message: string };

/** Compose the sync report counts into a single human-readable line. */
function syncReportMessage(report: SyncReport): string {
  return `Synced: ${String(report.installed)} installed, ${String(report.updated)} updated, ${String(report.skipped)} skipped.`;
}

/** Short human label for a skill's provenance badge. */
function sourceLabel(source: SkillSource): string {
  switch (source.kind) {
    case "Local":
      return "Local";
    case "GitHub":
      return `${source.owner}/${source.repo}`;
    default:
      return assertNever(source);
  }
}

/** Exhaustiveness guard: fails to compile if a `SkillSource` variant is added. */
function assertNever(x: never): never {
  throw new Error(`unreachable SkillSource: ${JSON.stringify(x)}`);
}

/**
 * Skills management view.
 *
 * A master/detail layout: the left column lists installed skills; selecting one
 * loads its raw `SKILL.md` into a monospace textarea for editing. Supports
 * creating a new skill, saving, deleting (guarded by a confirm dialog), and
 * syncing from the configured GitHub source.
 */
export function SkillsView() {
  const skills = useAppStore((s) => s.skills);
  const skillsLoading = useAppStore((s) => s.skillsLoading);
  const error = useAppStore((s) => s.error);
  const listSkills = useAppStore((s) => s.listSkills);
  const saveSkill = useAppStore((s) => s.saveSkill);
  const deleteSkill = useAppStore((s) => s.deleteSkill);
  const syncSkills = useAppStore((s) => s.syncSkills);

  const [selection, setSelection] = useState<Selection>({ kind: "none" });
  const [newName, setNewName] = useState("");
  const [contents, setContents] = useState("");
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  useEffect(() => {
    void listSkills();
  }, [listSkills]);

  // Reconstruct the raw SKILL.md from the parsed skill: frontmatter block plus
  // body. The list command returns parsed skills, so this reassembles a
  // faithful editable source rather than persisting the parsed halves.
  const rebuildSource = useCallback((skill: Skill): string => {
    const tags =
      skill.tags.length > 0 ? `tags: [${skill.tags.join(", ")}]\n` : "";
    return `---\nname: ${skill.name}\ndescription: ${skill.description}\n${tags}---\n\n${skill.body}`;
  }, []);

  const handleSelect = useCallback(
    (skill: Skill) => {
      setSelection({ kind: "existing", name: skill.name });
      setContents(rebuildSource(skill));
      setFeedback(null);
    },
    [rebuildSource],
  );

  const handleNew = useCallback(() => {
    setSelection({ kind: "new" });
    setNewName("");
    setContents(
      "---\nname: \ndescription: \ntags: []\n---\n\n# Skill\n\nDescribe the skill here.\n",
    );
    setFeedback(null);
  }, []);

  const handleSave = useCallback(async () => {
    setFeedback(null);
    let name: string;
    if (selection.kind === "existing") {
      name = selection.name;
    } else if (selection.kind === "new") {
      name = newName.trim();
      if (name === "") {
        setFeedback({ kind: "error", message: "A skill name is required." });
        return;
      }
    } else {
      return;
    }

    await saveSkill(name, contents);
    setSelection({ kind: "existing", name });
    setFeedback({ kind: "success", message: `Saved “${name}”.` });
  }, [selection, newName, contents, saveSkill]);

  const handleConfirmDelete = useCallback(async () => {
    if (confirmDelete === null) return;
    const name = confirmDelete;
    setConfirmDelete(null);
    await deleteSkill(name);
    setSelection({ kind: "none" });
    setContents("");
    setFeedback({ kind: "success", message: `Deleted “${name}”.` });
  }, [confirmDelete, deleteSkill]);

  const handleSync = useCallback(async () => {
    setFeedback(null);
    const report = await syncSkills();
    if (report !== null) {
      setFeedback({ kind: "success", message: syncReportMessage(report) });
    }
  }, [syncSkills]);

  const editing = selection.kind !== "none";
  const canSave =
    editing &&
    (selection.kind === "existing" ||
      (selection.kind === "new" && newName.trim() !== ""));

  return (
    <div className="mx-auto max-w-5xl px-6 py-8">
      {/* Header */}
      <div className="mb-6 flex items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <Blocks className="h-6 w-6 text-muted-foreground" />
          <div>
            <h1 className="text-xl font-semibold text-foreground">Skills</h1>
            <p className="text-sm text-muted-foreground">
              Reusable review skills injected into rework prompts.
            </p>
          </div>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            onClick={() => {
              void handleSync();
            }}
            disabled={skillsLoading}
          >
            {skillsLoading ? (
              <Loader2 className="animate-spin" />
            ) : (
              <RefreshCw />
            )}
            Sync from GitHub
          </Button>
          <Button onClick={handleNew}>
            <Plus />
            New skill
          </Button>
        </div>
      </div>

      {error !== null && (
        <p className="mb-4 text-sm text-destructive">{error}</p>
      )}
      {feedback !== null && (
        <p
          className={cn(
            "mb-4 text-sm",
            feedback.kind === "success" ? "text-success" : "text-destructive",
          )}
        >
          {feedback.message}
        </p>
      )}

      <div className="grid grid-cols-1 gap-6 md:grid-cols-[minmax(0,18rem)_minmax(0,1fr)]">
        {/* ----------------------------------------------------------------- */}
        {/* Skill list                                                        */}
        {/* ----------------------------------------------------------------- */}
        <div className="flex flex-col gap-2">
          {skillsLoading && skills.length === 0 ? (
            <p className="text-sm text-muted-foreground">Loading skills...</p>
          ) : skills.length === 0 ? (
            <EmptyState
              icon="🧩"
              title="No skills yet"
              description="Create a skill or sync from your configured GitHub source."
              actionLabel="New skill"
              onAction={handleNew}
            />
          ) : (
            <ul className="flex flex-col gap-2">
              {skills.map((skill) => {
                const active =
                  selection.kind === "existing" &&
                  selection.name === skill.name;
                return (
                  <li key={skill.path}>
                    <button
                      type="button"
                      onClick={() => {
                        handleSelect(skill);
                      }}
                      className={cn(
                        "w-full rounded-lg border px-3 py-2.5 text-left transition-colors",
                        active
                          ? "border-primary bg-muted"
                          : "border-border bg-card hover:bg-muted/50",
                      )}
                    >
                      <div className="flex items-center justify-between gap-2">
                        <span className="truncate text-sm font-medium text-foreground">
                          {skill.name}
                        </span>
                        <div className="flex shrink-0 items-center gap-1">
                          <Badge
                            variant={
                              skill.source.kind === "GitHub"
                                ? "secondary"
                                : "outline"
                            }
                            className="shrink-0"
                          >
                            {sourceLabel(skill.source)}
                          </Badge>
                          {skill.tags.length > 0 && (
                            <Badge variant="outline" className="shrink-0">
                              {skill.tags[0]}
                            </Badge>
                          )}
                        </div>
                      </div>
                      {skill.description !== "" && (
                        <p className="mt-1 truncate text-xs text-muted-foreground">
                          {skill.description}
                        </p>
                      )}
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </div>

        {/* ----------------------------------------------------------------- */}
        {/* Editor                                                            */}
        {/* ----------------------------------------------------------------- */}
        <div className="flex flex-col gap-4">
          {!editing ? (
            <div className="flex h-full min-h-64 items-center justify-center rounded-xl border border-border/50 bg-card px-8 py-12 text-center text-sm text-muted-foreground">
              Select a skill to edit, or create a new one.
            </div>
          ) : (
            <>
              {selection.kind === "new" && (
                <div className="flex flex-col gap-2">
                  <Label htmlFor="skill-name">Skill name</Label>
                  <Input
                    id="skill-name"
                    type="text"
                    value={newName}
                    onChange={(e) => {
                      setNewName(e.target.value);
                    }}
                    placeholder="rust-testing"
                    autoFocus
                  />
                </div>
              )}

              <div className="flex flex-col gap-2">
                <Label htmlFor="skill-contents">SKILL.md</Label>
                <Textarea
                  id="skill-contents"
                  value={contents}
                  onChange={(e) => {
                    setContents(e.target.value);
                  }}
                  spellCheck={false}
                  className="min-h-[26rem] font-mono text-xs leading-relaxed"
                />
              </div>

              <div className="flex items-center justify-between gap-3">
                {selection.kind === "existing" ? (
                  <Button
                    variant="destructive"
                    onClick={() => {
                      setConfirmDelete(selection.name);
                    }}
                    disabled={skillsLoading}
                  >
                    <Trash2 />
                    Delete
                  </Button>
                ) : (
                  <span />
                )}
                <Button
                  onClick={() => {
                    void handleSave();
                  }}
                  disabled={!canSave || skillsLoading}
                >
                  {skillsLoading ? (
                    <Loader2 className="animate-spin" />
                  ) : (
                    <Save />
                  )}
                  Save
                </Button>
              </div>
            </>
          )}
        </div>
      </div>

      {/* Delete confirmation (guarded destructive action). */}
      <Dialog
        open={confirmDelete !== null}
        onOpenChange={(open) => {
          if (!open) setConfirmDelete(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Delete skill</DialogTitle>
            <DialogDescription>
              This permanently removes “{confirmDelete}” from your local skills.
              This cannot be undone.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => {
                setConfirmDelete(null);
              }}
            >
              Cancel
            </Button>
            <Button
              variant="destructive"
              onClick={() => {
                void handleConfirmDelete();
              }}
            >
              <Trash2 />
              Delete
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
