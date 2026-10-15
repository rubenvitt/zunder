// Recurring tasks in the UI: after completing the current instance of a series, the
// server creates the next one (contract 3.1) and the client gets it with the next pull.
// The UI waits for it a while and then says when it is due ("Nächste Instanz am …").

import { createEffect, createRoot, createSignal } from "solid-js";
import { isRecurring } from "../sync/derived";
import type { Task } from "../sync/types";
import { nextInstanceMessage } from "../labels";
import { now, taskIndex } from "./store";
import { toast } from "./ui";

/** How long the UI waits for the server's next instance (offline it comes later, unannounced). */
export const NEXT_INSTANCE_WAIT_MS = 60_000;

const watched = new Map<string, ReturnType<typeof setTimeout>>();
const [watchCount, setWatchCount] = createSignal(0);

function unwatch(id: string): void {
  clearTimeout(watched.get(id));
  if (watched.delete(id)) setWatchCount(watched.size);
}

/**
 * Called when the UI completes a task (done or dropped). For the current instance of a
 * series without a live next instance yet, announces the next instance once it arrives.
 */
export function expectNextInstance(task: Task): void {
  if (!isRecurring(task) || taskIndex().nextInstance(task.id)) return;
  unwatch(task.id);
  watched.set(
    task.id,
    setTimeout(() => unwatch(task.id), NEXT_INSTANCE_WAIT_MS),
  );
  setWatchCount(watched.size);
}

createRoot(() => {
  createEffect(() => {
    // Only while waiting: then every change of the tasks is checked.
    if (watchCount() === 0) return;
    const index = taskIndex();
    for (const id of [...watched.keys()]) {
      const next = index.nextInstance(id);
      if (!next) continue;
      unwatch(id);
      toast(nextInstanceMessage(next.due, new Date(now())), "info", 6000);
    }
  });
});
