// Whether Jev (decision model for inbox proposals) is set up on the server.
// Loaded once per session on first use; the settings view updates it. Unknown
// (app, error, not loaded yet) counts as "not set up", so the panel stays away.

import { createSignal } from "solid-js";
import { isTauri } from "../platform/runtime";
import { api } from "./store";

const [configured, setConfigured] = createSignal(false);
let loading: Promise<unknown> | null = null;

/** True once the server reported a stored Jev key. */
export function jevConfigured(): boolean {
  if (!loading && !isTauri()) {
    loading = api()
      .jevSettings()
      .then((s) => setConfigured(s.configured))
      .catch(() => setConfigured(false));
  }
  return configured();
}

/** Called by the settings view after saving or resetting. */
export function setJevConfigured(value: boolean): void {
  loading ??= Promise.resolve();
  setConfigured(value);
}
