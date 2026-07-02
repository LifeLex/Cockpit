import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { GateState } from "../bindings/GateState";
import { StateFilter } from "./StateFilter";
import { makeReview } from "../test/fixtures";

describe("StateFilter", () => {
  const reviews = [
    makeReview({ pr: "a", gate_state: "InReview" }),
    makeReview({ pr: "b", gate_state: "InReview" }),
    makeReview({ pr: "c", gate_state: "Approved" }),
    makeReview({ pr: "d", gate_state: "Dispatched", stale: true }),
  ];

  it("renders per-state counts and the total", () => {
    render(
      <StateFilter
        reviews={reviews}
        activeFilter={null}
        showStale={false}
        onFilterChange={vi.fn()}
        onToggleStale={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: "All (4)" })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "In Review (2)" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Approved (1)" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Pending (0)" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Stale (1)" }),
    ).toBeInTheDocument();
  });

  it("invokes onFilterChange with the chosen gate state", async () => {
    const user = userEvent.setup();
    const onFilterChange = vi.fn<(s: GateState | null) => void>();
    render(
      <StateFilter
        reviews={reviews}
        activeFilter={null}
        showStale={false}
        onFilterChange={onFilterChange}
        onToggleStale={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("button", { name: "In Review (2)" }));
    expect(onFilterChange).toHaveBeenCalledWith("InReview");

    await user.click(screen.getByRole("button", { name: "All (4)" }));
    expect(onFilterChange).toHaveBeenCalledWith(null);
  });

  it("invokes onToggleStale when the stale chip is clicked", async () => {
    const user = userEvent.setup();
    const onToggleStale = vi.fn();
    render(
      <StateFilter
        reviews={reviews}
        activeFilter={null}
        showStale={false}
        onFilterChange={vi.fn()}
        onToggleStale={onToggleStale}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Stale (1)" }));
    expect(onToggleStale).toHaveBeenCalledOnce();
  });

  it("exposes Merged as a filterable chip so it is never hidden silently", async () => {
    const user = userEvent.setup();
    const onFilterChange = vi.fn<(s: GateState | null) => void>();
    const withMerged = [...reviews, makeReview({ pr: "e", gate_state: "Merged" })];
    render(
      <StateFilter
        reviews={withMerged}
        activeFilter={null}
        showStale={false}
        onFilterChange={onFilterChange}
        onToggleStale={vi.fn()}
      />,
    );

    const chip = screen.getByRole("button", { name: "Merged (1)" });
    expect(chip).toBeInTheDocument();
    await user.click(chip);
    expect(onFilterChange).toHaveBeenCalledWith("Merged");
  });

  it("renders zero counts for an empty review list", () => {
    render(
      <StateFilter
        reviews={[]}
        activeFilter={null}
        showStale={false}
        onFilterChange={vi.fn()}
        onToggleStale={vi.fn()}
      />,
    );
    expect(screen.getByRole("button", { name: "All (0)" })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Stale (0)" }),
    ).toBeInTheDocument();
  });
});
