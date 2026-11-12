// Context menus: one item model for the web menu and the desktop app's native one.

import { describe, expect, it, vi } from "vitest";
import { toNativeItems } from "../src/platform/native-menu";
import { tidy, type MenuEntry } from "../src/state/menu";

const item = (label: string, extra: Partial<Extract<MenuEntry, { kind: "item" }>> = {}): MenuEntry => ({ kind: "item", label, run: () => undefined, ...extra });
const sep: MenuEntry = { kind: "separator" };

describe("tidy", () => {
  it("drops leading, trailing and doubled separators", () => {
    expect(tidy([sep, item("A"), sep, sep, item("B"), sep]).map((e) => e.kind)).toEqual(["item", "separator", "item"]);
    expect(tidy([sep, sep])).toEqual([]);
  });
});

describe("toNativeItems", () => {
  it("maps items, check items, submenus and separators to Tauri options", () => {
    const run = vi.fn();
    const native = toNativeItems([
      item("Öffnen", { run }),
      item("Gesperrt", { disabled: true }),
      sep,
      { kind: "submenu", label: "Status", items: [item("Offen", { checked: true }), item("Erledigt", { checked: false })] },
    ]);
    expect(native).toEqual([
      { text: "Öffnen", enabled: true, action: expect.any(Function) },
      { text: "Gesperrt", enabled: false, action: expect.any(Function) },
      { item: "Separator" },
      {
        text: "Status",
        enabled: true,
        items: [
          { text: "Offen", enabled: true, checked: true, action: expect.any(Function) },
          { text: "Erledigt", enabled: true, checked: false, action: expect.any(Function) },
        ],
      },
    ]);
    (native[0] as { action: () => void }).action();
    expect(run).toHaveBeenCalledOnce();
  });
});
