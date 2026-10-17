// UI state shared across views: selection, overlays, toasts, picker requests.

import { createSignal, getOwner, onCleanup, type Accessor } from "solid-js";
import type { DeferTarget } from "../sync/defer";
import type { Location, ProjectKind, Task } from "../sync/types";

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

/** Copies text to the clipboard and confirms with a toast (or warns if that is not possible). */
export function copyText(text: string, done: string): void {
  if (!navigator.clipboard) {
    toast("Kopieren nicht möglich.", "warning");
    return;
  }
  void navigator.clipboard.writeText(text).then(
    () => toast(done),
    () => toast("Kopieren nicht möglich.", "warning"),
  );
}

/**
 * What agents need to address a task (claim_task, get_task …): its reference
 * (`ZUN-42`) if it has one, else its ID. One key or click away.
 */
export function copyTaskId(task: { id: string; ref: string | null }): void {
  if (task.ref) copyText(task.ref, `Kennung kopiert: ${task.ref}`);
  else copyText(task.id, `ID kopiert: ${task.id}`);
}

// ---- Selection and detail panel ---------------------------------------

export const [selectedId, setSelectedId] = createSignal<string | null>(null);
export const [detailId, setDetailId] = createSignal<string | null>(null);

/** Opens a task in the detail panel and selects it (links between tasks, mentions). */
export function openTask(id: string): void {
  setSelectedId(id);
  setDetailId(id);
}

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
  | { kind: "project"; id?: string; areaId?: string; projectKind?: ProjectKind };

export const [structureEdit, setStructureEdit] = createSignal<StructureEdit | null>(null);

/**
 * Dropping a recurring task asks first (contract 5.1): only this instance (the next one
 * follows) or the whole series. `before` runs right before the status is written.
 */
export interface DropRequest {
  taskId: string;
  before?: () => void;
}

export const [dropRequest, setDropRequest] = createSignal<DropRequest | null>(null);

/**
 * "Heute nicht" once more after two deferrals: a decision instead of a click (contract 5.1).
 * `before` runs right before a change that takes the task off the current list.
 */
export interface DeferRequest {
  taskId: string;
  target: DeferTarget;
  before?: () => void;
}

export const [deferRequest, setDeferRequest] = createSignal<DeferRequest | null>(null);

/** "Zurückgeben" of a Rauch task (contract 5.1): the dialog with sentence, name and reminder. */
export interface HandBackRequest {
  taskId: string;
}

export const [handBackRequest, setHandBackRequest] = createSignal<HandBackRequest | null>(null);

export function anyOverlayOpen(): boolean {
  return captureOpen() || helpOpen() || palette() !== null || structureEdit() !== null || dropRequest() !== null || deferRequest() !== null || handBackRequest() !== null;
}

// ---- Detail panel layout (per device, remembered) -----------------------

function stored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
function store(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Private mode or blocked storage: the setting lasts for this session only.
  }
}

// Sidebar (desktop): ⌘B / Ctrl+B hides it for more room, remembered per device.
const [sidebarHidden, setSidebarHiddenSignal] = createSignal(stored("zunder.sidebarHidden") === "1");
export { sidebarHidden };

export function toggleSidebar(): void {
  const hidden = !sidebarHidden();
  setSidebarHiddenSignal(hidden);
  store("zunder.sidebarHidden", hidden ? "1" : "0");
}

// Bereiche view (desktop): the tree of areas and projects can be hidden as well, remembered per device.
const [treeHidden, setTreeHiddenSignal] = createSignal(stored("zunder.treeHidden") === "1");
export { treeHidden };

export function toggleTree(): void {
  const hidden = !treeHidden();
  setTreeHiddenSignal(hidden);
  store("zunder.treeHidden", hidden ? "1" : "0");
}

export const DETAIL_MIN_WIDTH = 380;
const [detailWide, setDetailWideSignal] = createSignal(stored("zunder.detailWide") === "1");
const [detailWidth, setDetailWidthSignal] = createSignal(Number(stored("zunder.detailWidth")) || 560);
export { detailWide, detailWidth };

/** Wide mode: the open task takes the place of the list. */
export function setDetailWide(wide: boolean): void {
  setDetailWideSignal(wide);
  store("zunder.detailWide", wide ? "1" : "0");
}

export function setDetailWidth(width: number): void {
  const clamped = Math.round(Math.max(DETAIL_MIN_WIDTH, Math.min(width, window.innerWidth - 420)));
  setDetailWidthSignal(clamped);
  store("zunder.detailWidth", String(clamped));
}

// Dev view: project filter chips, remembered per device. Empty means all projects.
const [devProjectFilter, setDevProjectFilterSignal] = createSignal<string[]>(
  (stored("zunder.devProjectFilter") ?? "").split(",").filter(Boolean),
);
export { devProjectFilter };

export function setDevProjectFilter(ids: string[]): void {
  setDevProjectFilterSignal(ids);
  store("zunder.devProjectFilter", ids.join(","));
}
