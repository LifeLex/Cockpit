import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { PendingPermission } from "../store";
import { mockInvoke, callsFor } from "../test/tauri-mock";
import { makeReview, makeAgentRun } from "../test/fixtures";

// The panel talks to Tauri via the store (`invoke`) and directly via `listen`.
vi.mock("@tauri-apps/api/core", async () => {
  const mock = await import("../test/tauri-mock");
  return { invoke: mock.invoke };
});
vi.mock("@tauri-apps/api/event", async () => {
  const mock = await import("../test/tauri-mock");
  return { listen: mock.listen };
});

const { AgentPanel } = await import("./AgentPanel");
const { useAppStore } = await import("../store");

const pristine = useAppStore.getState();

// jsdom does not implement Element.prototype.scrollTo; the panel auto-scrolls to
// the newest entry on mount, so provide a no-op (defineProperty avoids typing the
// DOM's overloaded method signature).
Object.defineProperty(Element.prototype, "scrollTo", {
  value: () => {},
  writable: true,
  configurable: true,
});

beforeEach(() => {
  useAppStore.setState(pristine, true);
});

const PR = "https://github.com/o/r/pull/1";

/** A pending permission targeting the panel's object. */
function makePermission(
  overrides: Partial<PendingPermission> = {},
): PendingPermission {
  return {
    id: "perm-1",
    object_id: PR,
    tool_name: "Bash",
    summary: "cargo test",
    requested_at_epoch_ms: 1_700_000_000_000,
    ...overrides,
  };
}

/**
 * Seed an active review with an attached agent so the panel has a live run
 * (avoids the empty-state trajectory fetch) and enqueue a pending permission.
 */
function seed(): void {
  useAppStore.setState({
    activeReview: makeReview({ pr: PR, agent: makeAgentRun() }),
    pendingPermissions: [makePermission()],
  });
}

describe("AgentPanel permission entry", () => {
  it("renders the pending permission as a distinct entry", () => {
    seed();
    render(<AgentPanel visible objectId={PR} onClose={vi.fn()} />);

    expect(screen.getByText("Permission — Bash")).toBeInTheDocument();
    expect(screen.getByText("cargo test")).toBeInTheDocument();
    // The header status line reflects the blocked agent.
    expect(screen.getByText(/waiting for permission/)).toBeInTheDocument();
  });

  it("Allow resolves the request through the store with allow=true", async () => {
    const user = userEvent.setup();
    seed();
    mockInvoke("resolve_permission", () => true);

    render(<AgentPanel visible objectId={PR} onClose={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: "Allow" }));

    expect(callsFor("resolve_permission")[0]?.args).toEqual({
      id: "perm-1",
      allow: true,
    });
  });

  it("Deny resolves the request through the store with allow=false", async () => {
    const user = userEvent.setup();
    seed();
    mockInvoke("resolve_permission", () => true);

    render(<AgentPanel visible objectId={PR} onClose={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: "Deny" }));

    expect(callsFor("resolve_permission")[0]?.args).toEqual({
      id: "perm-1",
      allow: false,
    });
  });

  it("ignores a permission for a different object", () => {
    useAppStore.setState({
      activeReview: makeReview({ pr: PR, agent: makeAgentRun() }),
      pendingPermissions: [makePermission({ id: "other", object_id: "other-pr" })],
    });
    render(<AgentPanel visible objectId={PR} onClose={vi.fn()} />);

    expect(screen.queryByText("Permission — Bash")).not.toBeInTheDocument();
  });
});
