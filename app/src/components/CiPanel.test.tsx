import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor, act } from "@testing-library/react";
import {
  mockInvoke,
  emitEvent,
  listenerCount,
} from "../test/tauri-mock";
import { makeCheck } from "../test/fixtures";

// The panel talks to Tauri via the store (`invoke`) and directly via `listen`.
vi.mock("@tauri-apps/api/core", async () => {
  const mock = await import("../test/tauri-mock");
  return { invoke: mock.invoke };
});
vi.mock("@tauri-apps/api/event", async () => {
  const mock = await import("../test/tauri-mock");
  return { listen: mock.listen };
});
// openExternal routes through the opener plugin; stub it so no real IPC fires.
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(() => Promise.resolve()),
}));

const { CiPanel } = await import("./CiPanel");
const { useAppStore } = await import("../store");

const pristine = useAppStore.getState();

beforeEach(() => {
  useAppStore.setState(pristine, true);
});

describe("CiPanel", () => {
  it("renders the empty state when the PR has no checks", async () => {
    mockInvoke("list_ci_checks", () => []);

    render(<CiPanel pr="https://gh/pr/1" active={true} />);

    expect(await screen.findByText("No CI checks")).toBeInTheDocument();
  });

  it("groups checks into workflow pipelines after loading", async () => {
    mockInvoke("list_ci_checks", () => [
      makeCheck({ name: "build", workflow: "CI", bucket: "pass" }),
      makeCheck({ name: "e2e", workflow: "Nightly", bucket: "pass" }),
    ]);

    render(<CiPanel pr="https://gh/pr/1" active={true} />);

    // Two distinct workflows -> two pipeline section headers.
    expect(await screen.findByText("CI")).toBeInTheDocument();
    expect(screen.getByText("Nightly")).toBeInTheDocument();
  });

  it("updates live from a matching ci-updated event", async () => {
    mockInvoke("list_ci_checks", () => []);

    render(<CiPanel pr="https://gh/pr/1" active={true} />);
    expect(await screen.findByText("No CI checks")).toBeInTheDocument();

    // Backend pushes a fresh checks list for this PR.
    act(() => {
      emitEvent("ci-updated", [
        "https://gh/pr/1",
        [makeCheck({ name: "lint", workflow: "Lint", bucket: "pass" })],
      ]);
    });

    expect(await screen.findByText("Lint")).toBeInTheDocument();
    expect(screen.queryByText("No CI checks")).not.toBeInTheDocument();
  });

  it("ignores ci-updated events for a different PR", async () => {
    mockInvoke("list_ci_checks", () => []);

    render(<CiPanel pr="https://gh/pr/1" active={true} />);
    expect(await screen.findByText("No CI checks")).toBeInTheDocument();

    act(() => {
      emitEvent("ci-updated", [
        "https://gh/pr/OTHER",
        [makeCheck({ name: "lint", workflow: "Lint" })],
      ]);
    });

    // Still empty: the event was for another PR.
    expect(screen.getByText("No CI checks")).toBeInTheDocument();
    expect(screen.queryByText("Lint")).not.toBeInTheDocument();
  });

  it("unsubscribes from ci-updated on unmount", async () => {
    mockInvoke("list_ci_checks", () => []);

    const { unmount } = render(
      <CiPanel pr="https://gh/pr/1" active={true} />,
    );
    await screen.findByText("No CI checks");
    expect(listenerCount("ci-updated")).toBe(1);

    unmount();
    await waitFor(() => {
      expect(listenerCount("ci-updated")).toBe(0);
    });
  });
});
