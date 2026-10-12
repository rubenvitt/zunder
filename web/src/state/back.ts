// Back on phones. On the narrow layout the task detail covers the view like a
// pushed page, so each task opened there gets a history entry (same URL, marked
// in history.state). Back (browser, Android, Safari's edge swipe or the edge
// swipe in the detail) then leaves a subtask for its parent and closes the
// detail from the task that was opened first. Closing it any other way (button,
// Esc, delete) removes its entries again, so Back never has to be pressed twice.
// On the wide layout the panel sits next to the list and stays out of history.

import { createEffect, on, onCleanup } from "solid-js";
import { detailId, setDetailId, setSelectedId } from "./ui";

/** Phone layout: the detail covers everything (same breakpoint as styles.css). */
export const NARROW_LAYOUT = "(max-width: 900px)";

interface DetailEntry {
  zunderDetail: string;
  /** Number of detail entries up to and including this one. */
  zunderDepth: number;
}

function detailEntry(state: unknown): DetailEntry | null {
  const s = state as Partial<DetailEntry> | null;
  return s && typeof s.zunderDetail === "string" && typeof s.zunderDepth === "number" ? (s as DetailEntry) : null;
}

/** Back from the open detail: to the previous task in history, otherwise closed. */
export function detailBack(): void {
  if (detailEntry(history.state)) history.back();
  else setDetailId(null);
}

/** Keeps history and the open detail in step (once, from the app shell). */
export function installDetailHistory(): void {
  // A reload while a task was open starts with the detail closed: the entry becomes a plain one.
  if (detailEntry(history.state)) history.replaceState(null, "");
  // Whether the current entry is one of ours; a plain hash navigation must not close the desktop panel.
  let onDetailEntry = false;

  createEffect(
    on(
      detailId,
      (id) => {
        const current = detailEntry(history.state);
        if ((current?.zunderDetail ?? null) === id) return;
        if (id) {
          if (!window.matchMedia(NARROW_LAYOUT).matches) return;
          history.pushState({ zunderDetail: id, zunderDepth: (current?.zunderDepth ?? 0) + 1 } satisfies DetailEntry, "");
          onDetailEntry = true;
        } else if (current) {
          history.go(-current.zunderDepth);
        }
      },
      { defer: true },
    ),
  );

  const onPopState = (e: PopStateEvent) => {
    const target = detailEntry(e.state)?.zunderDetail ?? null;
    const left = onDetailEntry;
    onDetailEntry = target !== null;
    if (target === detailId()) return;
    if (target) {
      setSelectedId(target);
      setDetailId(target);
    } else if (left) {
      setDetailId(null);
    }
  };
  window.addEventListener("popstate", onPopState);
  onCleanup(() => window.removeEventListener("popstate", onPopState));
}
