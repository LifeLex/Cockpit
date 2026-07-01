/**
 * Open external URLs in the user's default system browser.
 *
 * A Tauri webview does not honor `<a target="_blank">`, so every external link
 * must be routed through the opener plugin. Failures are non-fatal (Invariant
 * 1): a link that cannot be opened is logged, never thrown, so the UI never
 * blocks on it.
 */

import { openUrl } from "@tauri-apps/plugin-opener";
import { useAppStore } from "../store";

/**
 * Open `url` in the system browser via the Tauri opener plugin.
 *
 * Never throws — opening a link is best-effort (Invariant 1). On failure it
 * surfaces a visible message via the app store (so a broken link is not a
 * silent no-op) and logs the underlying error. The most common cause in dev is
 * a stale `src-tauri` build: the opener plugin is compiled into the Rust side,
 * so `cargo tauri dev` must be fully restarted after it was added.
 */
export async function openExternal(url: string): Promise<void> {
  try {
    await openUrl(url);
  } catch (e: unknown) {
    console.error("openExternal failed", url, e);
    useAppStore.setState({
      error: `Couldn't open ${url} in your browser (${String(e)}). If you're running dev, restart \`cargo tauri dev\` so the opener plugin loads.`,
    });
  }
}
