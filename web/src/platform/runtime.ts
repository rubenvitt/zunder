// Runtime detection without any Tauri code: the browser bundle only carries
// this check, the Tauri bridge (platform/tauri.ts) is loaded on demand.

/** True inside the Tauri app's webview (Tauri 2 injects __TAURI_INTERNALS__). */
export function isTauri(): boolean {
  return typeof window !== "undefined" && !!(window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
}

/**
 * Opens an external link in a new tab, or in the app in the system browser
 * (via tauri-plugin-opener; window.open does nothing in the webview).
 */
export function openExternal(url: string): void {
  const internals = (window as { __TAURI_INTERNALS__?: { invoke(cmd: string, args: Record<string, unknown>): Promise<unknown> } }).__TAURI_INTERNALS__;
  if (internals) void internals.invoke("plugin:opener|open_url", { url }).catch(() => undefined);
  else window.open(url, "_blank", "noopener,noreferrer");
}
