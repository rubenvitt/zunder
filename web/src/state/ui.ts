// UI state shared across views: selection, overlays, toasts, picker requests.

import { createSignal, getOwner, onCleanup, type Accessor } from "solid-js";
import type { Location, Task } from "../sync/types";

export type ToastKind = "info" | "warning" | "error";

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
}

const [toasts, setToasts] = createSignal<Toast[]>([]);
let toastSeq = 0;

export { toasts };

export function toast(message: string, kind: ToastKind = "info", durationMs = kind === "error" ? 7000 : 4000): void {
  const id = ++toastSeq;
  setToasts((list) => [...list.slice(-3), { id, kind, message }]);
  setTimeout(() => dismissToast(id), durationMs);
}

export function dismissToast(id: number): void {
  setToasts((list) => list.filter((t) => t.id !== id));
}

// ---- Selection and detail panel ---------------------------------------

export const [selectedId, setSelectedId] = createSignal<string | null>(null);
export const [detailId, setDetailId] = createSignal<string | null>(null);

/** The keyboard-navigable task list of the current view (registered by the view). */
const NO_LIST: Accessor<Task[]> = () => [];
const [activeList, setActiveListSignal] = createSignal<Accessor<Task[]>>(NO_LIST);
export { activeList };

/**
 * Registers the list j/k/Enter/x work on. It is unregistered when the registering
 * component goes away, so a view without a list (empty Bereiche, tokens, Dev
 * without projects) never acts on the invisible tasks of the previous view.
 */
export function registerList(list: Accessor<Task[]>): void {
  setActiveListSignal(() => list);
  if (getOwner()) {
    onCleanup(() => {
      if (activeList() === list) setActiveListSignal(() => NO_LIST);
    });
  }
}

/** Board views register column navigation so h/l and Shift+H/L work. */
export interface BoardController {
  moveSelection(dx: -1 | 1): void;
  moveTask(dx: -1 | 1): void;
  reorder(dy: -1 | 1): void;
}
const [activeBoard, setActiveBoard] = createSignal<BoardController | null>(null);
export { activeBoard, setActiveBoard };

// ---- Overlays ----------------------------------------------------------

export const [captureOpen, setCaptureOpen] = createSignal(false);
export const [helpOpen, setHelpOpen] = createSignal(false);

export type PaletteRequest =
  | { mode: "search" }
  | { mode: "location"; title: string; onPick: (location: Location) => void }
  | { mode: "task"; title: string; excludeIds: ReadonlySet<string>; onPick: (taskId: string) => void };

export const [palette, setPalette] = createSignal<PaletteRequest | null>(null);

export function openSearch(): void {
  setPalette({ mode: "search" });
}

export function pickLocation(title: string, onPick: (location: Location) => void): void {
  setPalette({ mode: "location", title, onPick });
}

export function pickTask(title: string, excludeIds: ReadonlySet<string>, onPick: (taskId: string) => void): void {
  setPalette({ mode: "task", title, excludeIds, onPick });
}

export type StructureEdit =
  | { kind: "area"; id?: string }
  | { kind: "project"; id?: string; areaId?: string };

export const [structureEdit, setStructureEdit] = createSignal<StructureEdit | null>(null);

export function anyOverlayOpen(): boolean {
  return captureOpen() || helpOpen() || palette() !== null || structureEdit() !== null;
}
