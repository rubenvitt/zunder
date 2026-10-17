// "Erst für dich" (contract 5.1): the day it was clicked away and the task picked for a day live
// on the server (`/api/settings/focus`), so every browser sees them. This device keeps the last
// known value and its own changes in localStorage: offline they hold at once and go to the
// server at the next chance (start, switching to Heute, the browser going online). The Tauri app
// has no browser session for server settings and keeps both on the device.

import { createSignal } from "solid-js";
import { localDate } from "../sync/dates";
import { changeLocally, confirmed, fromServer, parseFocusLocal, type FocusLocal } from "../sync/focus";
import type { FocusSettings, FocusSettingsUpdate } from "../sync/types";
import { isUnauthorized } from "../sync/transport";
import { api, now } from "./store";

const STORAGE_KEY = "zunder.focus";

function load(): FocusLocal {
  try {
    return parseFocusLocal(localStorage.getItem(STORAGE_KEY));
  } catch {
    return parseFocusLocal(null);
  }
}

const [local, setLocalSignal] = createSignal<FocusLocal>(load());

function setLocal(next: FocusLocal): void {
  setLocalSignal(next);
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    // Private mode or blocked storage: the value lasts for this session only.
  }
}

/** The focus settings as this device knows them (server value plus own changes). */
export function focusSettings(): FocusSettings {
  return local().known;
}

/** Today's calendar day of this device: the day a dismissal or a pick holds for. */
export function focusToday(): string {
  return localDate(new Date(now()));
}

/** Errors that will not go away by trying again: the app has no such server settings. */
function isLocalOnly(err: unknown): boolean {
  return !!err && typeof err === "object" && (err as { code?: unknown }).code === "unsupported";
}

let inFlight: Promise<void> | null = null;

/**
 * Sends pending changes, else reads the server's value. One call at a time; failures keep the
 * local state (offline: retried at the next chance).
 */
export function refreshFocus(): Promise<void> {
  inFlight ??= sync().finally(() => (inFlight = null));
  return inFlight;
}

async function sync(): Promise<void> {
  try {
    const pending = local().pending;
    if (pending) setLocal(confirmed(local(), pending, await api().updateFocusSettings(pending)));
    else setLocal(fromServer(local(), await api().focusSettings()));
    // Changes made meanwhile go with the next round (after this one has finished).
    if (local().pending) setTimeout(() => void refreshFocus(), 0);
  } catch (err) {
    // The app keeps the value on the device; nothing will ever be sent.
    if (isLocalOnly(err)) setLocal({ known: local().known, pending: null });
    else if (!isUnauthorized(err)) console.debug("focus settings not synced", err);
  }
}

function change(update: FocusSettingsUpdate): void {
  setLocal(changeLocally(local(), update));
  void refreshFocus();
}

/** Clicks "Erst für dich" away for today. */
export function dismissFocus(): void {
  change({ dismissed_on: focusToday() });
}

/** Brings it back today (undo of the dismissal). */
export function undismissFocus(): void {
  change({ dismissed_on: null });
}

/** Picks a Glut task for today. */
export function chooseFocus(taskId: string): void {
  change({ chosen: { on: focusToday(), task_id: taskId } });
}

if (typeof window !== "undefined") {
  window.addEventListener("online", () => void refreshFocus());
}
