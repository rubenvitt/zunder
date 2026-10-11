// Runtime detection without any Tauri code: the browser bundle only carries
// this check, the Tauri bridge (platform/tauri.ts) is loaded on demand.

/** True inside the Tauri app's webview (Tauri 2 injects __TAURI_INTERNALS__). */
export function isTauri(): boolean {
  return typeof window !== "undefined" && !!(window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
}
