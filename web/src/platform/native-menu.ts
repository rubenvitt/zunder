// The system's own context menu in the desktop app (Tauri menu API, permission
// core:menu:default via core:default). Loaded only inside the app via dynamic import,
// like platform/tauri.ts, so the browser bundle never carries it.

import type { MenuEntry } from "../state/menu";

type NativeItem =
  | { text: string; enabled?: boolean; action?: () => void }
  | { text: string; enabled?: boolean; checked: boolean; action?: () => void }
  | { text: string; enabled?: boolean; items: NativeItem[] }
  | { item: "Separator" };

/** The Tauri item options for the shared item model (exported for tests). */
export function toNativeItems(entries: MenuEntry[]): NativeItem[] {
  return entries.map((entry): NativeItem => {
    if (entry.kind === "separator") return { item: "Separator" };
    if (entry.kind === "submenu") return { text: entry.label, enabled: !entry.disabled, items: toNativeItems(entry.items) };
    // A check item toggles its own mark on click; the action decides the real state.
    if (entry.checked !== undefined) return { text: entry.label, enabled: !entry.disabled, checked: entry.checked, action: () => entry.run() };
    return { text: entry.label, enabled: !entry.disabled, action: () => entry.run() };
  });
}

/** The last menu; its resources are freed when the next one opens. */
let previous: { close(): Promise<void> } | null = null;

export async function popupNativeMenu(entries: MenuEntry[], x: number, y: number): Promise<void> {
  const [{ Menu }, { LogicalPosition }] = await Promise.all([import("@tauri-apps/api/menu"), import("@tauri-apps/api/dpi")]);
  if (previous) {
    const old = previous;
    previous = null;
    void old.close().catch(() => undefined);
  }
  const menu = await Menu.new({ items: toNativeItems(entries) });
  previous = menu;
  await menu.popup(new LogicalPosition(x, y));
}
