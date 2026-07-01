/**
 * Typed test doubles for the Tauri IPC surface used by the store and
 * components: `@tauri-apps/api/core`'s `invoke` and `@tauri-apps/api/event`'s
 * `listen`.
 *
 * Tests register per-command handlers with {@link mockInvoke} instead of
 * asserting against a bare `vi.fn()`, so each test states exactly what the
 * backend would return (or reject with) for the commands it exercises. No
 * `any`: handlers are typed `(args) => unknown`, and callers cast the awaited
 * result at the call site where the command's return type is known.
 *
 * The `vi.mock` factories below live in this module so a single
 * `vi.mock("@tauri-apps/api/core", …)` in each test file can delegate here.
 */
import { vi } from "vitest";

/** A single command handler: receives the invoke args, returns/throws a value. */
export type InvokeHandler = (args: Record<string, unknown>) => unknown;

/** A registered event listener callback (mirrors Tauri's `EventCallback`). */
type EventCallback = (event: { readonly payload: unknown }) => void;

/** Command name -> handler registry, consulted by the mocked `invoke`. */
const handlers = new Map<string, InvokeHandler>();

/** Event name -> registered listener callbacks, driven by {@link emitEvent}. */
const listeners = new Map<string, Set<EventCallback>>();

/** Ordered record of every `invoke` call, for asserting call args. */
export interface InvokeCall {
  readonly cmd: string;
  readonly args: Record<string, unknown>;
}
const calls: InvokeCall[] = [];

/** Register (or replace) the handler for a single command. */
export function mockInvoke(cmd: string, handler: InvokeHandler): void {
  handlers.set(cmd, handler);
}

/**
 * Register a command whose handler rejects, to exercise error paths (the store
 * treats a rejected `invoke` as non-fatal and sets `error`).
 */
export function mockInvokeReject(cmd: string, reason: unknown): void {
  handlers.set(cmd, () => {
    throw reason instanceof Error ? reason : new Error(String(reason));
  });
}

/** Every recorded `invoke` call in order. */
export function invokeCalls(): readonly InvokeCall[] {
  return calls;
}

/** The recorded calls for a single command, in order. */
export function callsFor(cmd: string): readonly InvokeCall[] {
  return calls.filter((c) => c.cmd === cmd);
}

/** Fire a Tauri event to every listener registered for `name`. */
export function emitEvent(name: string, payload: unknown): void {
  const set = listeners.get(name);
  if (set === undefined) return;
  for (const cb of set) cb({ payload });
}

/** Number of live listeners for an event (asserts unlisten cleanup). */
export function listenerCount(name: string): number {
  return listeners.get(name)?.size ?? 0;
}

/** Clear all handlers, listeners, and recorded calls. Runs in `afterEach`. */
export function resetTauriMocks(): void {
  handlers.clear();
  listeners.clear();
  calls.length = 0;
}

/**
 * The mocked `invoke`. Looks up the registered handler; an unregistered command
 * rejects loudly so tests never silently pass against a missing stub.
 */
export const invoke = vi.fn(
  (cmd: string, args?: Record<string, unknown>): Promise<unknown> => {
    const resolvedArgs = args ?? {};
    calls.push({ cmd, args: resolvedArgs });
    const handler = handlers.get(cmd);
    if (handler === undefined) {
      return Promise.reject(
        new Error(`no mock registered for invoke("${cmd}")`),
      );
    }
    try {
      return Promise.resolve(handler(resolvedArgs));
    } catch (e: unknown) {
      return Promise.reject(e instanceof Error ? e : new Error(String(e)));
    }
  },
);

/**
 * The mocked `listen`. Records the callback and returns a Promise of an
 * unlisten function that removes it, mirroring Tauri's contract so components
 * can be asserted to clean up on unmount.
 */
export const listen = vi.fn(
  (name: string, cb: EventCallback): Promise<() => void> => {
    const set = listeners.get(name) ?? new Set<EventCallback>();
    set.add(cb);
    listeners.set(name, set);
    return Promise.resolve(() => {
      set.delete(cb);
    });
  },
);
