import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ReviewCard } from "./ReviewCard";
import { makeReview, makeAgentRun, ALL_GATE_STATES } from "../test/fixtures";

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
});
