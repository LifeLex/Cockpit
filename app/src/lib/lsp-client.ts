/**
 * Minimal LSP-over-WebSocket client for the Monaco diff editor.
 *
 * The Rust bridge (`cockpit_core::adapters::lsp`) exposes a localhost
 * WebSocket that speaks LSP JSON-RPC (the browser side sends/receives bare
 * JSON payloads; the bridge adds/strips `Content-Length` stdio framing). This
 * module connects to that URL and wires a *language server* to a Monaco text
 * model: it drives `initialize`/`didOpen`/`didChange`/`didClose`, renders
 * diagnostics as Monaco markers, and registers completion + hover providers
 * that proxy to the server.
 *
 * # Why not `monaco-languageclient`
 *
 * `monaco-languageclient` hard-couples to specific Monaco/Vite versions and
 * recent releases require the heavy `@codingame/monaco-vscode-api` shim, which
 * conflicts with this app's `@monaco-editor/react` (vanilla `monaco-editor`).
 * A small hand-written client keeps the dependency tree green and avoids that
 * migration. It covers the high-value features (diagnostics, completion,
 * hover) without pulling in the VS Code services layer.
 *
 * No `any`, no `as`-to-silence: LSP payloads are narrowed with type guards
 * (CLAUDE.md §3).
 */

import type { Monaco } from "@monaco-editor/react";
import type { editor, IDisposable, languages, Position } from "monaco-editor";
import { invoke } from "@tauri-apps/api/core";

// ---------------------------------------------------------------------------
// JSON-RPC wire types (narrowed, never `any`)
// ---------------------------------------------------------------------------

/** A JSON value, used where the LSP schema is not modeled in full. */
type Json =
  | null
  | boolean
  | number
  | string
  | readonly Json[]
  | { readonly [key: string]: Json };

interface RpcRequest {
  readonly jsonrpc: "2.0";
  readonly id: number;
  readonly method: string;
  readonly params: Json;
}

interface RpcNotification {
  readonly jsonrpc: "2.0";
  readonly method: string;
  readonly params: Json;
}

/** A message received from the server: either a response or a notification. */
interface RpcResponse {
  readonly jsonrpc: "2.0";
  readonly id: number;
  readonly result?: Json;
  readonly error?: { readonly code: number; readonly message: string };
}

function isRecord(value: unknown): value is { readonly [key: string]: unknown } {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isRpcResponse(value: unknown): value is RpcResponse {
  return isRecord(value) && typeof value["id"] === "number" && !("method" in value);
}

function isRpcNotification(value: unknown): value is RpcNotification {
  return isRecord(value) && typeof value["method"] === "string" && !("id" in value);
}

// ---------------------------------------------------------------------------
// LSP diagnostic shapes (narrowed)
// ---------------------------------------------------------------------------

interface LspPosition {
  readonly line: number;
  readonly character: number;
}

interface LspRange {
  readonly start: LspPosition;
  readonly end: LspPosition;
}

interface LspDiagnostic {
  readonly range: LspRange;
  readonly message: string;
  readonly severity?: number;
}

function isLspPosition(value: unknown): value is LspPosition {
  return (
    isRecord(value) &&
    typeof value["line"] === "number" &&
    typeof value["character"] === "number"
  );
}

function isLspRange(value: unknown): value is LspRange {
  return isRecord(value) && isLspPosition(value["start"]) && isLspPosition(value["end"]);
}

function isLspDiagnostic(value: unknown): value is LspDiagnostic {
  return isRecord(value) && isLspRange(value["range"]) && typeof value["message"] === "string";
}

function parseDiagnostics(params: unknown): {
  readonly uri: string;
  readonly diagnostics: readonly LspDiagnostic[];
} | null {
  if (!isRecord(params) || typeof params["uri"] !== "string") return null;
  const rawList = params["diagnostics"];
  const diagnostics: LspDiagnostic[] = [];
  if (Array.isArray(rawList)) {
    for (const d of rawList) {
      if (isLspDiagnostic(d)) diagnostics.push(d);
    }
  }
  return { uri: params["uri"], diagnostics };
}

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

/** Options for attaching a language client to a Monaco model. */
export interface LspAttachOptions {
  /** The Monaco namespace from `@monaco-editor/react`'s onMount. */
  readonly monaco: Monaco;
  /** The text model to provide language features for (the modified/right side). */
  readonly model: editor.ITextModel;
  /** The Monaco `languageId` (e.g. `"typescript"`, `"python"`). */
  readonly languageId: string;
  /** Absolute filesystem path of the workspace root for `rootUri`. */
  readonly rootPath: string;
}

/**
 * A live language-client attachment. Call [`dispose`](LspAttachment.dispose)
 * to tear down providers, close the document, and drop the socket.
 */
export interface LspAttachment {
  /** Tear down the attachment: didClose, dispose providers, close the socket. */
  readonly dispose: () => void;
}

/** Convert a filesystem path to a `file://` URI. */
function pathToUri(path: string): string {
  // Absolute paths only; encode each segment but keep separators.
  const encoded = path.split("/").map(encodeURIComponent).join("/");
  return `file://${encoded.startsWith("/") ? "" : "/"}${encoded}`;
}

/** Map an LSP severity (1..4) to a Monaco `MarkerSeverity`. */
function markerSeverity(
  monaco: Monaco,
  severity: number | undefined,
): editor.IMarkerData["severity"] {
  switch (severity) {
    case 1:
      return monaco.MarkerSeverity.Error;
    case 2:
      return monaco.MarkerSeverity.Warning;
    case 3:
      return monaco.MarkerSeverity.Info;
    case 4:
      return monaco.MarkerSeverity.Hint;
    default:
      return monaco.MarkerSeverity.Error;
  }
}

/**
 * Resolve the bridge WebSocket URL for `languageId` via the Tauri command,
 * then open a language-client attachment against `model`.
 *
 * Returns `null` when the backend reports no bridge (LSP disabled or the
 * language has no configured server), or when the WebSocket cannot open. A
 * `null` result is non-fatal: the editor keeps plain syntax highlighting.
 */
export async function attachLspClient(
  options: LspAttachOptions,
): Promise<LspAttachment | null> {
  const url = await invoke<string | null>("start_lsp_bridge", {
    languageId: options.languageId,
  }).catch((e: unknown) => {
    console.error("start_lsp_bridge failed", e);
    return null;
  });

  if (url === null || url === undefined) return null;

  const socket = await openSocket(url);
  if (socket === null) return null;

  return new LanguageClient(options, socket).start();
}

/** Open a WebSocket, resolving to it once open, or `null` on failure. */
function openSocket(url: string): Promise<WebSocket | null> {
  return new Promise((resolve) => {
    let settled = false;
    const socket = new WebSocket(url);
    socket.addEventListener("open", () => {
      if (!settled) {
        settled = true;
        resolve(socket);
      }
    });
    socket.addEventListener("error", () => {
      if (!settled) {
        settled = true;
        resolve(null);
      }
    });
  });
}

/**
 * Drives one language server for one Monaco model over the given socket.
 *
 * Instances are created internally by [`attachLspClient`]; the returned
 * [`LspAttachment`] is the only public surface.
 */
class LanguageClient {
  private readonly monaco: Monaco;
  private readonly model: editor.ITextModel;
  private readonly languageId: string;
  private readonly uri: string;
  private readonly rootUri: string;
  private readonly socket: WebSocket;

  private nextId = 1;
  private version = 1;
  private readonly pending = new Map<
    number,
    { resolve: (value: Json) => void; reject: (reason: unknown) => void }
  >();
  private readonly disposables: IDisposable[] = [];
  private disposed = false;

  constructor(options: LspAttachOptions, socket: WebSocket) {
    this.monaco = options.monaco;
    this.model = options.model;
    this.languageId = options.languageId;
    this.uri = pathToUri(`${options.rootPath}/__cockpit_review__.${extForLanguage(options.languageId)}`);
    this.rootUri = pathToUri(options.rootPath);
    this.socket = socket;
  }

  /** Begin the LSP session and register providers; returns the attachment. */
  start(): LspAttachment {
    this.socket.addEventListener("message", this.onMessage);
    this.socket.addEventListener("close", this.onClose);

    void this.initialize();

    return { dispose: () => { this.dispose(); } };
  }

  private readonly onMessage = (event: MessageEvent<unknown>): void => {
    if (typeof event.data !== "string") return;
    let parsed: unknown;
    try {
      parsed = JSON.parse(event.data);
    } catch {
      return;
    }

    if (isRpcResponse(parsed)) {
      const entry = this.pending.get(parsed.id);
      if (entry !== undefined) {
        this.pending.delete(parsed.id);
        if (parsed.error !== undefined) {
          entry.reject(new Error(parsed.error.message));
        } else {
          entry.resolve(parsed.result ?? null);
        }
      }
      return;
    }

    if (isRpcNotification(parsed)) {
      this.handleNotification(parsed);
    }
  };

  private readonly onClose = (): void => {
    this.dispose();
  };

  private handleNotification(notification: RpcNotification): void {
    if (notification.method === "textDocument/publishDiagnostics") {
      const parsed = parseDiagnostics(notification.params);
      if (parsed === null || parsed.uri !== this.uri) return;
      this.applyDiagnostics(parsed.diagnostics);
    }
  }

  private applyDiagnostics(diagnostics: readonly LspDiagnostic[]): void {
    if (this.disposed) return;
    const markers: editor.IMarkerData[] = diagnostics.map((d) => ({
      severity: markerSeverity(this.monaco, d.severity),
      message: d.message,
      startLineNumber: d.range.start.line + 1,
      startColumn: d.range.start.character + 1,
      endLineNumber: d.range.end.line + 1,
      endColumn: d.range.end.character + 1,
    }));
    this.monaco.editor.setModelMarkers(this.model, "cockpit-lsp", markers);
  }

  private send(message: RpcRequest | RpcNotification): void {
    if (this.socket.readyState !== WebSocket.OPEN) return;
    this.socket.send(JSON.stringify(message));
  }

  private request(method: string, params: Json): Promise<Json> {
    const id = this.nextId++;
    const message: RpcRequest = { jsonrpc: "2.0", id, method, params };
    return new Promise<Json>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.send(message);
    });
  }

  private notify(method: string, params: Json): void {
    this.send({ jsonrpc: "2.0", method, params });
  }

  private async initialize(): Promise<void> {
    try {
      await this.request("initialize", {
        processId: null,
        rootUri: this.rootUri,
        capabilities: {
          textDocument: {
            synchronization: { dynamicRegistration: false },
            completion: { completionItem: { snippetSupport: false } },
            hover: { contentFormat: ["plaintext", "markdown"] },
            publishDiagnostics: {},
          },
        },
        workspaceFolders: [{ uri: this.rootUri, name: "workspace" }],
      });
    } catch {
      // Initialize failed (server crashed / not installed). Non-fatal: the
      // editor keeps plain highlighting.
      this.dispose();
      return;
    }
    if (this.disposed) return;

    this.notify("initialized", {});
    this.didOpen();
    this.registerProviders();
  }

  private didOpen(): void {
    this.notify("textDocument/didOpen", {
      textDocument: {
        uri: this.uri,
        languageId: this.languageId,
        version: this.version,
        text: this.model.getValue(),
      },
    });
    // Sync subsequent edits (full-document; simplest correct strategy).
    this.disposables.push(
      this.model.onDidChangeContent(() => {
        this.version += 1;
        this.notify("textDocument/didChange", {
          textDocument: { uri: this.uri, version: this.version },
          contentChanges: [{ text: this.model.getValue() }],
        });
      }),
    );
  }

  private registerProviders(): void {
    const completion = this.monaco.languages.registerCompletionItemProvider(
      this.languageId,
      {
        provideCompletionItems: (model: editor.ITextModel, position: Position) =>
          this.provideCompletions(model, position),
      },
    );
    const hover = this.monaco.languages.registerHoverProvider(this.languageId, {
      provideHover: (model: editor.ITextModel, position: Position) =>
        this.provideHover(model, position),
    });
    this.disposables.push(completion, hover);
  }

  private async provideCompletions(
    model: editor.ITextModel,
    position: Position,
  ): Promise<languages.CompletionList | null> {
    if (model !== this.model) return null;
    const result = await this.request("textDocument/completion", {
      textDocument: { uri: this.uri },
      position: { line: position.lineNumber - 1, character: position.column - 1 },
    }).catch(() => null);

    const items = extractCompletionItems(result);
    if (items === null) return null;

    const word = model.getWordUntilPosition(position);
    const range: languages.CompletionItem["range"] = {
      startLineNumber: position.lineNumber,
      endLineNumber: position.lineNumber,
      startColumn: word.startColumn,
      endColumn: word.endColumn,
    };

    const kind = this.monaco.languages.CompletionItemKind.Text;
    return {
      suggestions: items.map((item): languages.CompletionItem => {
        const base = {
          label: item.label,
          kind,
          insertText: item.insertText,
          range,
        };
        // Under exactOptionalPropertyTypes, only include `detail` when present.
        return item.detail === undefined ? base : { ...base, detail: item.detail };
      }),
    };
  }

  private async provideHover(
    model: editor.ITextModel,
    position: Position,
  ): Promise<languages.Hover | null> {
    if (model !== this.model) return null;
    const result = await this.request("textDocument/hover", {
      textDocument: { uri: this.uri },
      position: { line: position.lineNumber - 1, character: position.column - 1 },
    }).catch(() => null);

    const contents = extractHoverContents(result);
    if (contents === null) return null;
    return { contents: [{ value: contents }] };
  }

  private dispose(): void {
    if (this.disposed) return;
    this.disposed = true;

    this.socket.removeEventListener("message", this.onMessage);
    this.socket.removeEventListener("close", this.onClose);

    for (const d of this.disposables) d.dispose();
    this.disposables.length = 0;

    for (const [, entry] of this.pending) {
      entry.reject(new Error("language client disposed"));
    }
    this.pending.clear();

    // Best-effort clear markers so stale diagnostics don't linger.
    if (!this.model.isDisposed()) {
      this.monaco.editor.setModelMarkers(this.model, "cockpit-lsp", []);
    }

    if (
      this.socket.readyState === WebSocket.OPEN ||
      this.socket.readyState === WebSocket.CONNECTING
    ) {
      this.notify("shutdown", null);
      this.socket.close();
    }
  }
}

// ---------------------------------------------------------------------------
// Result extraction (narrowed)
// ---------------------------------------------------------------------------

interface CompletionEntry {
  readonly label: string;
  readonly insertText: string;
  readonly detail: string | undefined;
}

function extractCompletionItems(result: unknown): readonly CompletionEntry[] | null {
  const rawItems = Array.isArray(result)
    ? result
    : isRecord(result) && Array.isArray(result["items"])
      ? result["items"]
      : null;
  if (rawItems === null) return null;

  const entries: CompletionEntry[] = [];
  for (const item of rawItems) {
    if (!isRecord(item) || typeof item["label"] !== "string") continue;
    const label = item["label"];
    const insertText =
      typeof item["insertText"] === "string" ? item["insertText"] : label;
    const detail = typeof item["detail"] === "string" ? item["detail"] : undefined;
    entries.push({ label, insertText, detail });
  }
  return entries;
}

function extractHoverContents(result: unknown): string | null {
  if (!isRecord(result)) return null;
  const contents = result["contents"];
  if (typeof contents === "string") return contents === "" ? null : contents;
  if (isRecord(contents) && typeof contents["value"] === "string") {
    return contents["value"] === "" ? null : contents["value"];
  }
  if (Array.isArray(contents)) {
    const parts: string[] = [];
    for (const part of contents) {
      if (typeof part === "string") parts.push(part);
      else if (isRecord(part) && typeof part["value"] === "string") {
        parts.push(part["value"]);
      }
    }
    const joined = parts.join("\n\n");
    return joined === "" ? null : joined;
  }
  return null;
}

/** A representative file extension for a Monaco language id, for the doc URI. */
function extForLanguage(languageId: string): string {
  switch (languageId) {
    case "typescript":
      return "ts";
    case "javascript":
      return "js";
    case "python":
      return "py";
    default:
      return "txt";
  }
}
