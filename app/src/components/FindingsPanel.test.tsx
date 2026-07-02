import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import {
  FindingsPanel,
  findingsAutoExpand,
  findingsBreakdown,
} from "./FindingsPanel";
import { makeFinding } from "../test/fixtures";

/** A no-op toggle for tests that don't exercise the collapse control. */
const noop = () => {
  /* no-op */
};

describe("findingsBreakdown", () => {
  it("counts non-dismissed findings per severity", () => {
    const findings = [
      makeFinding({ id: "a", severity: "Critical" }),
      makeFinding({ id: "b", severity: "Warning" }),
      makeFinding({ id: "c", severity: "Warning" }),
      makeFinding({ id: "d", severity: "Info" }),
    ];
    expect(findingsBreakdown(findings, new Set())).toEqual({
      critical: 1,
      warning: 2,
      info: 1,
    });
  });

  it("excludes dismissed findings from the counts", () => {
    const findings = [
      makeFinding({ id: "a", severity: "Critical" }),
      makeFinding({ id: "b", severity: "Warning" }),
    ];
    expect(findingsBreakdown(findings, new Set(["a"]))).toEqual({
      critical: 0,
      warning: 1,
      info: 0,
    });
  });
});

describe("findingsAutoExpand", () => {
  it("expands only when a non-dismissed Critical finding exists", () => {
    const withCritical = [makeFinding({ id: "a", severity: "Critical" })];
    const withoutCritical = [makeFinding({ id: "b", severity: "Warning" })];
    expect(findingsAutoExpand(withCritical, new Set())).toBe(true);
    expect(findingsAutoExpand(withoutCritical, new Set())).toBe(false);
    // Dismissing the only Critical drops it below the threshold.
    expect(findingsAutoExpand(withCritical, new Set(["a"]))).toBe(false);
  });
});

describe("FindingsPanel", () => {
  it("renders nothing when there are no findings", () => {
    const { container } = render(
      <FindingsPanel
        findings={[]}
        dismissed={new Set()}
        open
        onToggle={noop}
        onDismiss={noop}
        onJumpTo={noop}
      />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("renders nothing once every finding is dismissed", () => {
    const { container } = render(
      <FindingsPanel
        findings={[makeFinding({ id: "a" })]}
        dismissed={new Set(["a"])}
        open
        onToggle={noop}
        onDismiss={noop}
        onJumpTo={noop}
      />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the count and a severity breakdown of only non-zero classes", () => {
    render(
      <FindingsPanel
        findings={[
          makeFinding({ id: "a", severity: "Critical" }),
          makeFinding({ id: "b", severity: "Warning" }),
          makeFinding({ id: "c", severity: "Warning" }),
        ]}
        dismissed={new Set()}
        open
        onToggle={noop}
        onDismiss={noop}
        onJumpTo={noop}
      />,
    );
    expect(screen.getByText("PRE-REVIEW FINDINGS (3)")).toBeInTheDocument();
    expect(screen.getByText("1 critical · 2 warning")).toBeInTheDocument();
    // No Info findings present, so "info" is omitted entirely.
    expect(screen.queryByText(/info/)).not.toBeInTheDocument();
  });

  it("keeps the header visible but hides rows when collapsed", () => {
    render(
      <FindingsPanel
        findings={[makeFinding({ id: "a", title: "Hidden while collapsed" })]}
        dismissed={new Set()}
        open={false}
        onToggle={noop}
        onDismiss={noop}
        onJumpTo={noop}
      />,
    );
    expect(screen.getByText("PRE-REVIEW FINDINGS (1)")).toBeInTheDocument();
    expect(
      screen.queryByText("Hidden while collapsed"),
    ).not.toBeInTheDocument();
  });

  it("shows a file-level tag (no jump) for a line-0 finding", () => {
    const onJumpTo = vi.fn();
    render(
      <FindingsPanel
        findings={[makeFinding({ id: "a", range: [0, 0] })]}
        dismissed={new Set()}
        open
        onToggle={noop}
        onDismiss={noop}
        onJumpTo={onJumpTo}
      />,
    );
    expect(screen.getByText("file-level")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Jump" }),
    ).not.toBeInTheDocument();
  });

  it("jumps to the finding's start line, side-aware", () => {
    const onJumpTo = vi.fn();
    render(
      <FindingsPanel
        findings={[
          makeFinding({
            id: "a",
            path: "src/foo.rs",
            range: [12, 14],
            side: "Old",
          }),
        ]}
        dismissed={new Set()}
        open
        onToggle={noop}
        onDismiss={noop}
        onJumpTo={onJumpTo}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Jump" }));
    expect(onJumpTo).toHaveBeenCalledWith("src/foo.rs", "Old", 12);
  });

  it("calls onDismiss with the finding id", () => {
    const onDismiss = vi.fn();
    render(
      <FindingsPanel
        findings={[makeFinding({ id: "finding-42" })]}
        dismissed={new Set()}
        open
        onToggle={noop}
        onDismiss={onDismiss}
        onJumpTo={noop}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Dismiss finding" }));
    expect(onDismiss).toHaveBeenCalledWith("finding-42");
  });

  it("toggles a long rationale with a show-more control", () => {
    const long = "x".repeat(200);
    render(
      <FindingsPanel
        findings={[makeFinding({ id: "a", rationale: long })]}
        dismissed={new Set()}
        open
        onToggle={noop}
        onDismiss={noop}
        onJumpTo={noop}
      />,
    );
    const toggle = screen.getByRole("button", { name: "Show more" });
    fireEvent.click(toggle);
    expect(
      screen.getByRole("button", { name: "Show less" }),
    ).toBeInTheDocument();
  });
});
