// Context menus (right click): one item model, two renderers. In the desktop app the
// menu is the system's own (platform/native-menu.ts, loaded on demand); in the browser,
// on mobile or when the native menu fails, the web UI draws it (components/ContextMenu).

import { createSignal } from "solid-js";
import { isTauri } from "../platform/runtime";
import { appInfo } from "./desktop";

export type MenuEntry =
  | {
      kind: "item";
      label: string;
      run: () => void;
      disabled?: boolean;
      /** Shown with a check mark (current status, weight …). */
      checked?: boolean;
      /** Destructive (Löschen): drawn in the danger colour in the web menu. */
      danger?: boolean;
      /** Keyboard hint shown on the right in the web menu (e.g. "x"). */
      hint?: string;
    }
  | { kind: "submenu"; label: string; items: MenuEntry[]; disabled?: boolean }
  | { kind: "separator" };

export interface MenuRequest {
  items: MenuEntry[];
  /** Viewport position (CSS pixels) of the menu's top left corner. */
  x: number;
  y: number;
  /** Element that gets the focus back when the menu closes without an action. */
  returnFocus?: HTMLElement | null;
}

const [contextMenu, setContextMenu] = createSignal<MenuRequest | null>(null);
export { contextMenu };

export function closeContextMenu(): void {
  setContextMenu(null);
}

/** Separators only between groups: none leading, trailing or doubled. */
export function tidy(items: MenuEntry[]): MenuEntry[] {
  const out: MenuEntry[] = [];
  for (const item of items) {
    if (item.kind === "separator" && (out.length === 0 || out[out.length - 1]!.kind === "separator")) continue;
    out.push(item);
  }
  while (out.length > 0 && out[out.length - 1]!.kind === "separator") out.pop();
  return out;
}

/** Native menus exist in the desktop app only (not on iOS/Android). */
function nativeMenus(): boolean {
  if (!isTauri()) return false;
  const desktop = appInfo()?.desktop;
  if (desktop !== undefined) return desktop;
  return !/iPhone|iPad|Android/i.test(navigator.userAgent);
}

/** Shows a context menu at a viewport position: native in the desktop app, else drawn by the web UI. */
export function showContextMenu(request: MenuRequest): void {
  const items = tidy(request.items);
  if (items.length === 0) return;
  if (!nativeMenus()) {
    setContextMenu({ ...request, items });
    return;
  }
  void import("../platform/native-menu")
    .then((mod) => mod.popupNativeMenu(items, request.x, request.y))
    .catch((err: unknown) => {
      console.warn("Natives Kontextmenü nicht verfügbar, zeige das der Web-Oberfläche", err);
      setContextMenu({ ...request, items });
    });
}
