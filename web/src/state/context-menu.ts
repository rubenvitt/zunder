// Context menus on the document: right click, the context menu key (Shift+F10) and,
// on touch screens, a long press open the menu of the nearest element that carries
// `data-task-id`, `data-area-id`, `data-project-id` or `data-comment-id`.
// Text fields, the editor and selected text keep the system's own menu (copy, paste).

import { localDate } from "../sync/dates";
import { showContextMenu, type MenuEntry } from "./menu";
import { areaMenuItems, commentMenuItems, projectMenuItems } from "./structure-menu";
import { taskMenuItems } from "./task-menu";
import { data, now, taskIndex } from "./store";
import { activeList, setSelectedId } from "./ui";

const TARGETS = "[data-task-id], [data-area-id], [data-project-id], [data-comment-id]";

/** How long a finger rests before the menu opens, and how far it may wander meanwhile. */
export const LONG_PRESS_MS = 500;
const LONG_PRESS_SLOP = 10;

function isEditable(el: Element): boolean {
  if (el.closest("input, textarea, select")) return true;
  return el instanceof HTMLElement && el.isContentEditable;
}

/** The menu items for an element, or null if it names nothing live. Side effects (selection) happen here. */
export function menuFor(el: HTMLElement): MenuEntry[] | null {
  const { taskId, areaId, projectId, commentId } = el.dataset;
  if (taskId !== undefined) {
    const index = taskIndex();
    const task = index.byId.get(taskId);
    if (!task || index.isDeleted(task)) return null;
    // Like Finder: the row the menu belongs to is the selected one (if it is in the current list).
    if (activeList()().some((t) => t.id === task.id)) setSelectedId(task.id);
    return taskMenuItems(task, localDate(new Date(now())));
  }
  if (projectId !== undefined) {
    const project = data.projects[projectId];
    return project && !project.deleted ? projectMenuItems(project) : null;
  }
  if (areaId !== undefined) {
    const area = data.areas[areaId];
    return area && !area.deleted ? areaMenuItems(area) : null;
  }
  if (commentId !== undefined) {
    const comment = data.comments[commentId];
    return comment && !comment.deleted ? commentMenuItems(comment) : null;
  }
  return null;
}

/** The element whose menu a press on `target` opens, or null if the system menu stays. */
function targetOf(target: EventTarget | null): HTMLElement | null {
  if (!(target instanceof Element) || isEditable(target)) return null;
  const el = target.closest<HTMLElement>(TARGETS);
  if (!el) return null;
  // Selected text keeps the system menu (copy, look up …).
  const selection = window.getSelection();
  if (selection && !selection.isCollapsed && selection.anchorNode && el.contains(selection.anchorNode)) return null;
  return el;
}

function open(el: HTMLElement, x: number, y: number): boolean {
  const items = menuFor(el);
  if (!items) return false;
  const focused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  showContextMenu({ items, x, y, returnFocus: focused });
  return true;
}

/** Installs the context menus; returns the uninstaller. */
export function installContextMenu(): () => void {
  // ---- Long press (touch): iOS never sends `contextmenu`, Android does after its own delay ----
  let press: { id: number; x: number; y: number; el: HTMLElement; timer: ReturnType<typeof setTimeout> } | null = null;
  /** Set once a long press opened a menu: the click and `contextmenu` that follow belong to it. */
  let pressed = false;

  const cancel = () => {
    if (press) clearTimeout(press.timer);
    press = null;
  };
  const onPointerDown = (e: PointerEvent) => {
    cancel();
    pressed = false;
    if (e.pointerType !== "touch" || !e.isPrimary || e.defaultPrevented) return;
    const el = targetOf(e.target);
    if (!el) return;
    const at = { id: e.pointerId, x: e.clientX, y: e.clientY, el };
    press = {
      ...at,
      timer: setTimeout(() => {
        press = null;
        if (!at.el.isConnected || !open(at.el, at.x, at.y)) return;
        pressed = true;
        navigator.vibrate?.(10);
      }, LONG_PRESS_MS),
    };
  };
  const onPointerMove = (e: PointerEvent) => {
    if (press && e.pointerId === press.id && Math.hypot(e.clientX - press.x, e.clientY - press.y) > LONG_PRESS_SLOP) cancel();
  };
  const onPointerEnd = (e: PointerEvent) => {
    if (press && e.pointerId === press.id) cancel();
  };
  const onClick = (e: MouseEvent) => {
    if (!pressed) return;
    // The finger lifting after a long press is no tap on the row.
    pressed = false;
    if (e.target instanceof Element && e.target.closest(".context-menu")) return;
    e.preventDefault();
    e.stopPropagation();
  };

  // ---- Right click and the context menu key ----
  const onContextMenu = (e: MouseEvent) => {
    if (pressed) {
      // Android's own long press after ours: the menu is already open.
      e.preventDefault();
      return;
    }
    if (e.defaultPrevented) return;
    const el = targetOf(e.target);
    if (!el) return;
    cancel();
    // From the keyboard there is no pointer position: below the element.
    let x = e.clientX;
    let y = e.clientY;
    if (x === 0 && y === 0) {
      const rect = el.getBoundingClientRect();
      x = rect.left + 16;
      y = rect.top + Math.min(rect.height, 32);
    }
    if (open(el, x, y)) e.preventDefault();
  };

  document.addEventListener("contextmenu", onContextMenu);
  document.addEventListener("pointerdown", onPointerDown, true);
  document.addEventListener("pointermove", onPointerMove, true);
  document.addEventListener("pointerup", onPointerEnd, true);
  document.addEventListener("pointercancel", onPointerEnd, true);
  document.addEventListener("dragstart", cancel, true);
  document.addEventListener("scroll", cancel, true);
  document.addEventListener("click", onClick, true);
  return () => {
    cancel();
    document.removeEventListener("contextmenu", onContextMenu);
    document.removeEventListener("pointerdown", onPointerDown, true);
    document.removeEventListener("pointermove", onPointerMove, true);
    document.removeEventListener("pointerup", onPointerEnd, true);
    document.removeEventListener("pointercancel", onPointerEnd, true);
    document.removeEventListener("dragstart", cancel, true);
    document.removeEventListener("scroll", cancel, true);
    document.removeEventListener("click", onClick, true);
  };
}
