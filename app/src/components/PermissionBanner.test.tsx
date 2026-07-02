import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import type { PendingPermission } from "../store";

// The banner reads the store, which routes Tauri IPC through the typed mock.
vi.mock("@tauri-apps/api/core", async () => {
  const mock = await import("../test/tauri-mock");
  return { invoke: mock.invoke };
});
vi.mock("@tauri-apps/api/event", async () => {
  const mock = await import("../test/tauri-mock");
  return { listen: mock.listen };
});

const { PermissionBanner } = await import("./PermissionBanner");
const { useAppStore } = await import("../store");

const pristine = useAppStore.getState();

beforeEach(() => {
  useAppStore.setState(pristine, true);
});

/** A minimal pending tool-permission request for the render tests. */
function makePermission(
  overrides: Partial<PendingPermission> = {},
): PendingPermission {
  return {
    id: "perm-1",
    object_id: "https://github.com/o/r/pull/1",
    tool_name: "Write",
    summary: "Write src/x.ts",
    requested_at_epoch_ms: 1_700_000_000_000,
    ...overrides,
  };
}

describe("PermissionBanner", () => {
  it("renders nothing when there are no pending permissions", () => {
    const { container } = render(<PermissionBanner />);
    expect(container).toBeEmptyDOMElement();
  });

  it("renders the count and the first request's summary", () => {
    useAppStore.setState({
      pendingPermissions: [
        makePermission({ id: "p-1", summary: "Run cargo test" }),
        makePermission({ id: "p-2", summary: "Write file" }),
      ],
    });

    render(<PermissionBanner />);

    expect(
      screen.getByText("2 agents waiting for permission"),
    ).toBeInTheDocument();
    // The summary shown is the first request's.
    expect(screen.getByText("Run cargo test")).toBeInTheDocument();
  });

  it("uses the singular noun for a single pending request", () => {
    useAppStore.setState({
      pendingPermissions: [makePermission({ id: "p-1" })],
    });

    render(<PermissionBanner />);

    expect(
      screen.getByText("1 agent waiting for permission"),
    ).toBeInTheDocument();
  });
});
