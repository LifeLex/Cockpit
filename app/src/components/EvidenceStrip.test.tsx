import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { EvidenceStrip } from "./EvidenceStrip";
import type { EvidenceSummary } from "../bindings/EvidenceSummary";
import type { CiCheck } from "../bindings/CiCheck";
import { makeCheck } from "../test/fixtures";

/** An evidence bundle exercising every chip kind. */
function makeEvidence(overrides: Partial<EvidenceSummary> = {}): EvidenceSummary {
  return {
    signals: {
      additions: 120,
      deletions: 30,
      files_changed: 4,
      size_class: "M",
      test_delta: {
        test_files_changed: 1,
        assertions_added: 2,
        assertions_removed: 1,
      },
      risk_paths: [{ flag: "Migration", path: "db/migrations/001_init.sql" }],
      weakening: [
        {
          kind: "DeletedAssertion",
          path: "tests/foo.rs",
          line: 3,
          side: "Old",
          excerpt: "assert_eq!(x, 42);",
        },
      ],
    },
    ci: { passed: 3, total: 4, failed: 1, pending: 0 },
    agent_ran: [{ command: "cargo test", ok: true }],
    ...overrides,
  };
}

describe("EvidenceStrip", () => {
  it("renders nothing when evidence is null", () => {
    const { container } = render(<EvidenceStrip evidence={null} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("renders a chip for each evidence kind", () => {
    const checks: CiCheck[] = [
      makeCheck({ name: "build", workflow: "Nightly", bucket: "fail" }),
    ];
    render(<EvidenceStrip evidence={makeEvidence()} ciChecks={checks} />);

    // CI rollup + the failing workflow name.
    expect(screen.getByText("3/4")).toBeInTheDocument();
    expect(screen.getByText("CI")).toBeInTheDocument();
    expect(screen.getByText("Nightly")).toBeInTheDocument();
    // Size chip.
    expect(screen.getByText("M")).toBeInTheDocument();
    expect(screen.getByText("4 files")).toBeInTheDocument();
    // Test delta.
    expect(screen.getByText("Tests")).toBeInTheDocument();
    expect(screen.getByText("+2")).toBeInTheDocument();
    // Risk path.
    expect(screen.getByText("Migration")).toBeInTheDocument();
    // Weakening flag.
    expect(screen.getByText("Deleted assertion")).toBeInTheDocument();
    // Agent command.
    expect(screen.getByText("cargo test")).toBeInTheDocument();
  });

  it("omits the CI chip when the bundle has no CI", () => {
    render(<EvidenceStrip evidence={makeEvidence({ ci: null })} />);
    expect(screen.queryByText("CI")).not.toBeInTheDocument();
    // The size chip is still present.
    expect(screen.getByText("M")).toBeInTheDocument();
  });

  it("jumps to the offending hunk when a weakening chip is clicked", () => {
    const onJumpTo = vi.fn();
    render(<EvidenceStrip evidence={makeEvidence()} onJumpTo={onJumpTo} />);

    fireEvent.click(
      screen.getByRole("button", { name: /Deleted assertion/ }),
    );
    expect(onJumpTo).toHaveBeenCalledWith("tests/foo.rs", "Old", 3);
  });
});
