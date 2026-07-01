import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { GatePill, gateStateLabel } from "./GatePill";
import { ALL_GATE_STATES } from "../test/fixtures";

describe("GatePill", () => {
  it("renders the human-readable label for every gate state", () => {
    for (const state of ALL_GATE_STATES) {
      const { unmount } = render(<GatePill state={state} />);
      expect(screen.getByText(gateStateLabel(state))).toBeInTheDocument();
      unmount();
    }
  });

  it("maps InReview to the spaced 'In Review' label", () => {
    expect(gateStateLabel("InReview")).toBe("In Review");
  });

  it("applies the state-keyed token color class", () => {
    render(<GatePill state="Approved" />);
    const pill = screen.getByText("Approved");
    expect(pill.className).toContain("text-state-approved");
  });

  it("appends caller-provided classes", () => {
    render(<GatePill state="Pending" className="ml-2" />);
    expect(screen.getByText("Pending").className).toContain("ml-2");
  });
});
