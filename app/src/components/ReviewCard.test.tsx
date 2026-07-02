import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ReviewCard } from "./ReviewCard";
import { makeReview, makeAgentRun, ALL_GATE_STATES } from "../test/fixtures";

/** A diff adding `n` lines to `path`, for exercising the risk chips. */
function addLines(path: string, n: number): string {
  let s = `diff --git a/${path} b/${path}\n--- a/${path}\n+++ b/${path}\n@@ -0,0 +1,${String(n)} @@\n`;
  for (let i = 0; i < n; i++) {
    s += `+row ${String(i)}\n`;
  }
  return s;
}

describe("ReviewCard", () => {
  it("shows the Restack button only when the review is stale", () => {
    const fresh = makeReview({ stale: false });
    const { rerender } = render(
      <ReviewCard review={fresh} onAction={vi.fn()} onRestack={vi.fn()} />,
    );
    expect(
      screen.queryByRole("button", { name: /Restack/ }),
    ).not.toBeInTheDocument();

    const stale = makeReview({ stale: true });
    rerender(
      <ReviewCard review={stale} onAction={vi.fn()} onRestack={vi.fn()} />,
    );
    expect(
      screen.getByRole("button", { name: /Restack/ }),
    ).toBeInTheDocument();
  });

  it("disables Restack and shows 'Restacking…' while an agent is active", () => {
    const restacking = makeReview({
      stale: true,
      agent: makeAgentRun({ mode: "Restack" }),
    });
    render(
      <ReviewCard review={restacking} onAction={vi.fn()} onRestack={vi.fn()} />,
    );
    const btn = screen.getByRole("button", { name: /Restacking/ });
    expect(btn).toBeDisabled();
  });

  it("fires onRestack with the PR ref when clicked", async () => {
    const user = userEvent.setup();
    const onRestack = vi.fn<(pr: string) => void>();
    const stale = makeReview({ pr: "pr-xyz", stale: true, agent: null });
    render(
      <ReviewCard review={stale} onAction={vi.fn()} onRestack={onRestack} />,
    );

    await user.click(screen.getByRole("button", { name: /Restack/ }));
    expect(onRestack).toHaveBeenCalledWith("pr-xyz");
  });

  it("renders a context-aware primary action for every gate state", () => {
    // Exercising the valid GateState union proves the assertNever default is
    // unreachable without forcing an invalid value.
    const labels: Record<(typeof ALL_GATE_STATES)[number], string> = {
      Pending: "Review",
      InReview: "Review",
      Dispatched: "Watch",
      Reworked: "Re-review",
      Approved: "View",
      Merged: "View",
    };
    for (const state of ALL_GATE_STATES) {
      const { unmount } = render(
        <ReviewCard
          review={makeReview({ gate_state: state })}
          onAction={vi.fn()}
          onRestack={vi.fn()}
        />,
      );
      expect(
        screen.getByRole("button", { name: labels[state] }),
      ).toBeInTheDocument();
      unmount();
    }
  });

  it("leads with a state-derived reason line", () => {
    render(
      <ReviewCard
        review={makeReview({ gate_state: "Reworked" })}
        onAction={vi.fn()}
        onRestack={vi.fn()}
      />,
    );
    expect(screen.getByText(/agent reworked/i)).toBeInTheDocument();
  });

  it("shows a Restack reason for a stale review", () => {
    render(
      <ReviewCard
        review={makeReview({ stale: true })}
        onAction={vi.fn()}
        onRestack={vi.fn()}
      />,
    );
    expect(screen.getByText(/restack needed/i)).toBeInTheDocument();
  });

  it("surfaces the stack relationship when the review has children", () => {
    render(
      <ReviewCard
        review={makeReview({ children: ["child-1", "child-2"] })}
        onAction={vi.fn()}
        onRestack={vi.fn()}
      />,
    );
    expect(screen.getByText(/parent of 2/i)).toBeInTheDocument();
  });

  it("renders the compact density as a telemetry row with an action", () => {
    render(
      <ReviewCard
        review={makeReview({ gate_state: "Pending" })}
        onAction={vi.fn()}
        onRestack={vi.fn()}
        density="compact"
      />,
    );
    expect(
      screen.getByRole("button", { name: "Review" }),
    ).toBeInTheDocument();
  });

  it("renders a CI x/y chip from the review's ci_summary", () => {
    render(
      <ReviewCard
        review={makeReview({
          ci_summary: { passed: 2, total: 3, failed: 1, pending: 0 },
          diff: { raw: addLines("data.txt", 10) },
        })}
        onAction={vi.fn()}
        onRestack={vi.fn()}
      />,
    );
    expect(screen.getByText("2/3")).toBeInTheDocument();
  });

  it("adds the F6 splitting nudge to the size chip past 400 changed lines", () => {
    render(
      <ReviewCard
        review={makeReview({ diff: { raw: addLines("data.txt", 500) } })}
        onAction={vi.fn()}
        onRestack={vi.fn()}
      />,
    );
    expect(
      screen.getByTitle("Consider splitting (>400 changed lines)"),
    ).toBeInTheDocument();
  });

  it("surfaces a sensitive-path chip for a risky file", () => {
    render(
      <ReviewCard
        review={makeReview({
          diff: { raw: addLines("migrations/001_init.sql", 5) },
        })}
        onAction={vi.fn()}
        onRestack={vi.fn()}
      />,
    );
    expect(screen.getByText("migrations")).toBeInTheDocument();
  });

  it("surfaces a test-touch chip when the diff touches tests", () => {
    render(
      <ReviewCard
        review={makeReview({ diff: { raw: addLines("src/foo.test.ts", 5) } })}
        onAction={vi.fn()}
        onRestack={vi.fn()}
      />,
    );
    expect(screen.getByText("tests")).toBeInTheDocument();
  });

  it("layers a risk note under the gate reason", () => {
    render(
      <ReviewCard
        review={makeReview({
          gate_state: "Pending",
          ci_summary: { passed: 1, total: 2, failed: 1, pending: 0 },
          diff: { raw: addLines("data.txt", 10) },
        })}
        onAction={vi.fn()}
        onRestack={vi.fn()}
      />,
    );
    expect(screen.getByText(/needs your review/i)).toBeInTheDocument();
    expect(screen.getByText(/CI failing/)).toBeInTheDocument();
  });
});
