import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ConversationBand } from "./ConversationBand";
import { makeConversationItem } from "../test/fixtures";

// openExternal routes through the opener plugin; stub it so no real IPC fires.
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(() => Promise.resolve()),
}));

describe("ConversationBand", () => {
  it("renders a row for each conversation kind with its label", () => {
    const items = [
      makeConversationItem({
        id: "r-approve",
        kind: "Review",
        author: "reviewer1",
        state: "APPROVED",
        body: "Ship it.",
      }),
      makeConversationItem({
        id: "r-changes",
        kind: "Review",
        author: "reviewer2",
        state: "CHANGES_REQUESTED",
        body: "Needs work.",
      }),
      makeConversationItem({
        id: "rc-1",
        kind: "ReviewComment",
        author: "reviewer3",
        path: "src/lib.rs",
        line: 42,
        body: "Nit here.",
      }),
      makeConversationItem({
        id: "ic-1",
        kind: "IssueComment",
        author: "octocat",
        body: "Top-level thought.",
      }),
    ];

    render(
      <ConversationBand items={items} loading={false} onRefresh={vi.fn()} />,
    );

    // Each author renders as a mono @handle.
    expect(screen.getByText("@reviewer1")).toBeInTheDocument();
    expect(screen.getByText("@reviewer2")).toBeInTheDocument();
    expect(screen.getByText("@reviewer3")).toBeInTheDocument();
    expect(screen.getByText("@octocat")).toBeInTheDocument();

    // Kind/state labels.
    expect(screen.getByText("approved")).toBeInTheDocument();
    expect(screen.getByText("requested changes")).toBeInTheDocument();
    expect(screen.getByText("src/lib.rs:42")).toBeInTheDocument();
    // ReviewComment and IssueComment both surface a "comment" label; the review
    // comment shows its anchor instead, so exactly one plain "comment" remains.
    expect(screen.getAllByText("comment")).toHaveLength(1);

    // Bodies are rendered as plain text.
    expect(screen.getByText("Ship it.")).toBeInTheDocument();
    expect(screen.getByText("Top-level thought.")).toBeInTheDocument();

    // Header count reflects the number of items and is expanded by default.
    expect(
      screen.getByText(/CONVERSATION ON GITHUB · READ-ONLY \(4\)/),
    ).toBeInTheDocument();
    expect(
      screen
        .getByRole("button", { name: /conversation on github/i })
        .getAttribute("aria-expanded"),
    ).toBe("true");
  });

  it("collapses by default when empty and hides item rows", () => {
    render(<ConversationBand items={[]} loading={false} onRefresh={vi.fn()} />);

    const toggle = screen.getByRole("button", {
      name: /conversation on github/i,
    });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(
      screen.getByText(/CONVERSATION ON GITHUB · READ-ONLY \(0\)/),
    ).toBeInTheDocument();
    // Collapsed: the empty-state row is not rendered.
    expect(
      screen.queryByText("No GitHub conversation yet."),
    ).not.toBeInTheDocument();
  });

  it("fires onRefresh when the refresh control is clicked", () => {
    const onRefresh = vi.fn();
    render(
      <ConversationBand
        items={[makeConversationItem()]}
        loading={false}
        onRefresh={onRefresh}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: /refresh conversation/i }),
    );
    expect(onRefresh).toHaveBeenCalledOnce();
  });
});
