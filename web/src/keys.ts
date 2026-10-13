// Global keyboard map (contract section 5).

import { isTauri } from "./platform/runtime";
import { toggleDone } from "./state/actions";
import { navigate, route, type Route } from "./state/router";
import { data } from "./state/store";
import {
  activeBoard,
  activeList,
  anyOverlayOpen,
  copyTaskId,
  detailId,
  openSearch,
  palette,
  selectedId,
  setCaptureOpen,
  setDetailId,
  setHelpOpen,
  setPalette,
  setSelectedId,
  toggleSidebar,
} from "./state/ui";
import { inboxActions } from "./views/Lists";
import { toggleCurrentProjectView } from "./views/Structure";

const GO: Record<string, Route> = {
  h: { view: "heute" },
  i: { view: "inbox" },
  l: { view: "lose" },
  b: { view: "bereiche" },
  d: { view: "dev" },
  p: { view: "phoenix" },
};

function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || target.isContentEditable;
}

/**
 * A focused control that Enter activates natively (button, link …). The row's own
 * main button is not counted: Enter there opens the selected task either way.
 */
function isInteractive(target: EventTarget | null): target is HTMLElement {
  if (!(target instanceof HTMLElement)) return false;
  if (target.classList.contains("task-main")) return false;
  return !!target.closest("button, a[href], summary, [role=button], [role=link], [role=menuitem], [role=tab], [role=option]");
}

function moveSelection(delta: number): void {
  const list = activeList()();
  if (list.length === 0) return;
  const i = list.findIndex((t) => t.id === selectedId());
  const next = i < 0 ? (delta > 0 ? 0 : list.length - 1) : Math.max(0, Math.min(list.length - 1, i + delta));
  const id = list[next]!.id;
  setSelectedId(id);
  if (detailId()) setDetailId(id);
  // Navigating the list takes the focus off a control (e.g. a clicked nav link), so the
  // next Enter opens the selected task instead of activating that control again.
  const active = document.activeElement;
  if (isInteractive(active)) {
    const row = document.querySelector<HTMLElement>(`[data-task-id="${CSS.escape(id)}"] .task-main`);
    if (row) row.focus({ preventScroll: true });
    else active.blur();
  }
}

function selectedTask() {
  const id = selectedId();
  if (id && data.tasks[id] && activeList()().some((t) => t.id === id)) return data.tasks[id];
  return undefined;
}

export function installKeyboard(): () => void {
  let goPending = false;
  let goTimer: ReturnType<typeof setTimeout> | undefined;

  const handler = (e: KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      if (palette()) setPalette(null);
      else openSearch();
      return;
    }
    if (e.defaultPrevented || e.isComposing) return;
    // ⌘, / Ctrl+,: settings, as everywhere on macOS. Works from text fields too, not over a dialog.
    if ((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && e.key === ",") {
      if (anyOverlayOpen()) return;
      e.preventDefault();
      // The Tauri app has no session settings; its settings page is the connection page.
      navigate({ view: isTauri() ? "verbindung" : "einstellungen" });
      return;
    }
    // ⌘B / Ctrl+B: sidebar on/off. In the Markdown editor it stays "bold".
    if ((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "b") {
      const target = e.target instanceof HTMLElement ? e.target : null;
      if (target?.isContentEditable || anyOverlayOpen()) return;
      e.preventDefault();
      toggleSidebar();
      return;
    }
    if (e.key === "Escape") {
      if (anyOverlayOpen()) return;
      if (isTyping(e.target)) {
        (e.target as HTMLElement).blur();
        return;
      }
      if (detailId()) setDetailId(null);
      else setSelectedId(null);
      return;
    }
    if (isTyping(e.target) || anyOverlayOpen() || e.metaKey || e.ctrlKey || e.altKey) return;

    if (goPending) {
      goPending = false;
      clearTimeout(goTimer);
      const target = GO[e.key];
      if (target) {
        e.preventDefault();
        navigate(target);
      }
      return;
    }

    const board = activeBoard();
    const key = e.key;
    let handled = true;
    switch (key) {
      case "g":
        goPending = true;
        goTimer = setTimeout(() => (goPending = false), 1200);
        break;
      case "n":
        setCaptureOpen(true);
        break;
      case "/":
        openSearch();
        break;
      case "?":
        setHelpOpen(true);
        break;
      case "j":
      case "ArrowDown":
        moveSelection(1);
        break;
      case "k":
      case "ArrowUp":
        moveSelection(-1);
        break;
      case "J":
        if (board) board.reorder(1);
        else handled = false;
        break;
      case "K":
        if (board) board.reorder(-1);
        else handled = false;
        break;
      case "h":
      case "ArrowLeft":
        if (board) board.moveSelection(-1);
        else handled = false;
        break;
      case "ArrowRight":
        if (board) board.moveSelection(1);
        else handled = false;
        break;
      case "H":
        if (board) board.moveTask(-1);
        else handled = false;
        break;
      case "L":
        if (board) board.moveTask(1);
        else handled = false;
        break;
      case "Enter": {
        // Let a focused button or link do its own thing (contract: fully keyboard operable).
        if (isInteractive(e.target)) return;
        const task = selectedTask();
        if (task) setDetailId(task.id);
        else handled = false;
        break;
      }
      case "x": {
        const task = selectedTask();
        if (task) toggleDone(task.id);
        else handled = false;
        break;
      }
      case "y": {
        // The open task first, then the selected row: "yank" the id for an agent.
        const id = detailId() ?? selectedTask()?.id;
        const task = id ? data.tasks[id] : undefined;
        if (task) copyTaskId(task);
        else handled = false;
        break;
      }
      case "v":
        handled = toggleCurrentProjectView();
        break;
      default:
        handled = false;
    }

    // Keys that depend on the view.
    if (!handled && route().view === "inbox" && (key === "a" || key === "l" || key === "d")) {
      const task = selectedTask() ?? activeList()()[0];
      if (task) {
        handled = true;
        if (key === "a") inboxActions.assign(task);
        else if (key === "l") inboxActions.loose(task);
        else inboxActions.drop(task);
      }
    } else if (!handled && board && key === "l") {
      board.moveSelection(1);
      handled = true;
    }
    if (handled) e.preventDefault();
  };

  window.addEventListener("keydown", handler);
  return () => window.removeEventListener("keydown", handler);
}
