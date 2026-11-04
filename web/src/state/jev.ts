// Whether Jev (decision model for task proposals) is set up on the server.
// Loaded once per session on first use through `GET /api/jev`, which the
// browser and the app may read; the settings view updates it. Unknown (offline,
// error, not loaded yet) counts as "not set up", so the panel stays away.

import { createSignal } from "solid-js";
import { api } from "./store";

const [configured, setConfigured] = createSignal(false);
let loading: Promise<unknown> | null = null;
// A request whose generation is no longer current (the settings view set a value
// meanwhile) must not overwrite the newer value.
let generation = 0;

/** True once the server reported a stored Jev key. */
export function jevConfigured(): boolean {
  if (!loading) {
    const mine = ++generation;
    loading = api()
      .jevStatus()
      .then((s) => {
        if (mine === generation) setConfigured(s.configured);
      })
      .catch(() => {
        if (mine !== generation) return;
        // Try again on the next read (no loop: an unchanged false notifies nobody).
        loading = null;
        setConfigured(false);
      });
  }
  return configured();
}

/** Called by the settings view after saving or resetting. */
export function setJevConfigured(value: boolean): void {
  generation += 1;
  loading ??= Promise.resolve();
  setConfigured(value);
}
