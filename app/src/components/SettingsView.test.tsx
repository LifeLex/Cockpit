import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Config } from "../bindings/Config";
import { mockInvoke } from "../test/tauri-mock";

// SettingsView loads/saves config through the store (`invoke`) and browses via
// the dialog plugin; stub both so no real IPC fires.
vi.mock("@tauri-apps/api/core", async () => {
  const mock = await import("../test/tauri-mock");
  return { invoke: mock.invoke };
});
vi.mock("@tauri-apps/api/event", async () => {
  const mock = await import("../test/tauri-mock");
  return { listen: mock.listen };
});
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(() => Promise.resolve(null)),
}));

const { SettingsView } = await import("./SettingsView");
const { useAppStore } = await import("../store");

const pristine = useAppStore.getState();

beforeEach(() => {
  useAppStore.setState(pristine, true);
});

/** A full, valid Config so the form renders (never a partial literal). */
function makeConfig(overrides: Partial<Config> = {}): Config {
  return {
    linear_api_key: null,
    linear_project_id: null,
    repo_path: null,
    agent_command: "claude",
    hook_port: 19876,
    max_parallel_agents: 3,
    ide_command: null,
    app_theme: "dark",
    editor_theme: "vs-dark",
    agent_prompts: {
      implement: null,
      plan: null,
      fix: null,
      restack: null,
      review: null,
    },
    skills_github: null,
    lsp_servers: {
      enabled: true,
      pyright_command: null,
      typescript_command: null,
    },
    notify_poll_secs: null,
    agent_permission_mode: "Approve",
    ...overrides,
  };
}

describe("SettingsView agent permission mode", () => {
  it("does not switch to Bypass when the confirmation is cancelled", async () => {
    const user = userEvent.setup();
    mockInvoke("get_config", () => makeConfig());
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);

    render(<SettingsView />);

    const approve = await screen.findByRole("radio", {
      name: /Approve in cockpit/,
    });
    expect(approve).toHaveAttribute("aria-checked", "true");

    const bypass = screen.getByRole("radio", { name: /Bypass/ });
    await user.click(bypass);

    // The confirm ran, but the cancel kept Approve selected.
    expect(confirmSpy).toHaveBeenCalledTimes(1);
    expect(bypass).toHaveAttribute("aria-checked", "false");
    expect(approve).toHaveAttribute("aria-checked", "true");

    confirmSpy.mockRestore();
  });

  it("switches to Bypass when the confirmation is accepted", async () => {
    const user = userEvent.setup();
    mockInvoke("get_config", () => makeConfig());
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);

    render(<SettingsView />);

    const approve = await screen.findByRole("radio", {
      name: /Approve in cockpit/,
    });
    const bypass = screen.getByRole("radio", { name: /Bypass/ });
    await user.click(bypass);

    expect(bypass).toHaveAttribute("aria-checked", "true");
    expect(approve).toHaveAttribute("aria-checked", "false");

    confirmSpy.mockRestore();
  });

  it("selecting Auto-accept edits needs no confirmation", async () => {
    const user = userEvent.setup();
    mockInvoke("get_config", () => makeConfig());
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);

    render(<SettingsView />);

    const acceptEdits = await screen.findByRole("radio", {
      name: /Auto-accept edits/,
    });
    await user.click(acceptEdits);

    expect(confirmSpy).not.toHaveBeenCalled();
    expect(acceptEdits).toHaveAttribute("aria-checked", "true");

    confirmSpy.mockRestore();
  });
});
