/**
 * Embedded terminal backed by a PTY on the Rust side and xterm.js
 * in the browser.
 *
 * On mount the component spawns a shell session (via `spawn_shell`),
 * listens for output events (`"shell-output"`), and forwards keystrokes
 * to the backend (`shell_write`). On unmount it kills the session.
 */

import { useEffect, useRef, useCallback } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "@xterm/xterm/css/xterm.css";

interface TerminalProps {
  /** Unique session identifier (typically `crypto.randomUUID()`). */
  readonly id: string;
  /** Working directory for the shell process. */
  readonly cwd: string;
  /** Called when the user explicitly closes the terminal. */
  readonly onClose?: (() => void) | undefined;
}

/** Payload shape emitted by the Rust `"shell-output"` event. */
interface ShellOutputPayload {
  readonly id: string;
  readonly data: string;
}

/**
 * Decode a base64 string into a Uint8Array.
 *
 * Uses the browser's built-in `atob` which is always available in
 * a Tauri webview context.
 */
function decodeBase64(encoded: string): Uint8Array {
  const raw = atob(encoded);
  const bytes = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) {
    // INVARIANT: charCodeAt always returns a number for valid indices
    // within the string length.
    bytes[i] = raw.charCodeAt(i);
  }
  return bytes;
}

/**
 * Embedded terminal component.
 *
 * Renders an xterm.js instance that communicates with a PTY-backed
 * shell session on the Rust side via Tauri commands and events.
 */
function Terminal({ id, cwd, onClose }: TerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<XTerm | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  // Track whether the effect has already cleaned up to avoid
  // double-killing sessions in React strict mode.
  const cleanedUpRef = useRef(false);

  const handleClose = useCallback(() => {
    if (onClose !== undefined) {
      onClose();
    }
  }, [onClose]);

  useEffect(() => {
    const container = containerRef.current;
    if (container === null) return;

    cleanedUpRef.current = false;

    const term = new XTerm({
      cursorBlink: true,
      fontFamily: "'SF Mono', 'Fira Code', 'Cascadia Code', monospace",
      fontSize: 13,
      lineHeight: 1.2,
      theme: {
        background: "#0a0a0a",
        foreground: "#e5e5e5",
        cursor: "#e5e5e5",
        selectionBackground: "#3a3a3a",
        black: "#0a0a0a",
        red: "#e06c75",
        green: "#98c379",
        yellow: "#e5c07b",
        blue: "#61afef",
        magenta: "#c678dd",
        cyan: "#56b6c2",
        white: "#e5e5e5",
        brightBlack: "#5c6370",
        brightRed: "#e06c75",
        brightGreen: "#98c379",
        brightYellow: "#e5c07b",
        brightBlue: "#61afef",
        brightMagenta: "#c678dd",
        brightCyan: "#56b6c2",
        brightWhite: "#ffffff",
      },
    });

    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(container);
    fitAddon.fit();

    termRef.current = term;
    fitAddonRef.current = fitAddon;

    // Forward keystrokes to the PTY.
    const dataDisposable = term.onData((data: string) => {
      void invoke("shell_write", { id, data });
    });

    // Listen for output from the PTY.
    let unlistenFn: (() => void) | null = null;

    const setupListener = listen<ShellOutputPayload>(
      "shell-output",
      (event) => {
        if (event.payload.id === id) {
          const bytes = decodeBase64(event.payload.data);
          term.write(bytes);
        }
      },
    );

    void setupListener.then((fn) => {
      unlistenFn = fn;
    });

    // Spawn the shell session.
    void invoke("spawn_shell", { id, cwd });

    // Handle resize events.
    const handleResize = () => {
      if (fitAddonRef.current !== null) {
        fitAddonRef.current.fit();
        const dims = fitAddonRef.current.proposeDimensions();
        if (dims !== undefined) {
          void invoke("shell_resize", {
            id,
            cols: dims.cols,
            rows: dims.rows,
          });
        }
      }
    };

    const resizeObserver = new ResizeObserver(handleResize);
    resizeObserver.observe(container);

    return () => {
      if (!cleanedUpRef.current) {
        cleanedUpRef.current = true;
        void invoke("shell_kill", { id });
      }
      resizeObserver.disconnect();
      dataDisposable.dispose();
      if (unlistenFn !== null) {
        unlistenFn();
      }
      term.dispose();
      termRef.current = null;
      fitAddonRef.current = null;
    };
    // The id and cwd are stable for the lifetime of this component
    // instance. handleClose is memoized via useCallback.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, cwd]);

  return (
    <div className="flex h-full flex-col">
      {onClose !== undefined && (
        <div className="flex items-center justify-between border-b border-border bg-card px-3 py-1.5">
          <span className="text-xs text-muted-foreground font-mono truncate">
            {cwd}
          </span>
          <button
            onClick={handleClose}
            className="rounded px-2 py-0.5 text-xs text-muted-foreground hover:bg-accent hover:text-accent-foreground"
          >
            Close
          </button>
        </div>
      )}
      <div
        ref={containerRef}
        className="flex-1 min-h-0"
        style={{ backgroundColor: "#0a0a0a" }}
      />
    </div>
  );
}

export { Terminal };
export type { TerminalProps };
