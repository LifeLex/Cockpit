/**
 * Vitest global setup. Registered via `test.setupFiles` in `vite.config.ts`.
 *
 * Adds jest-dom matchers (`toBeInTheDocument`, `toHaveTextContent`, …) and
 * resets the shared Tauri IPC mocks between tests so state never leaks across
 * cases.
 */
import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";
import { resetTauriMocks } from "./tauri-mock";

afterEach(() => {
  cleanup();
  resetTauriMocks();
});
