// Status of linked GitHub PRs (`pr_url`), read through `POST /api/pr-status`.
//
// Components call `prStatus(url)` while rendering. Unknown links are collected
// and asked in one batch shortly after; the answer lands in a store, so every
// card and the detail view update together. Links that were shown recently are
// asked again every few minutes and when the window comes back into view (the
// server caches GitHub's answers, so this stays cheap).
//
// Without a GitHub token on the server (`configured: false`), offline or on an
// error, `prStatus` stays `undefined` and the UI shows the plain link.

import { createStore, reconcile } from "solid-js/store";
import type { PrStatus, PrStatuses } from "../sync/types";
import { api } from "./store";

type Fetch = (urls: string[]) => Promise<PrStatuses>;

/** Delay that collects the links of one render pass. */
const BATCH_DELAY_MS = 30;
/** How often shown links are asked again. */
export const REFRESH_MS = 3 * 60 * 1000;
/** Links not shown for this long are no longer refreshed. */
const FORGET_MS = 15 * 60 * 1000;
/** Without a token, ask again after this long (the server may have got one). */
const UNCONFIGURED_RETRY_MS = 30 * 60 * 1000;
/** Upper bound of the server per request. */
const MAX_PER_REQUEST = 200;

export interface PrTracker {
  /** `undefined`: not known (yet); `null`: GitHub does not know the PR or may not show it. */
  status(url: string): PrStatus | null | undefined;
  /** Asks again for all links shown recently. */
  refresh(): void;
  dispose(): void;
}

export function createPrTracker(fetch: () => Fetch, now: () => number = Date.now): PrTracker {
  const [statuses, setStatuses] = createStore<Record<string, PrStatus | null>>({});
  /** Link → last time it was shown. */
  const seen = new Map<string, number>();
  /** Link → time of the last answer. */
  const fetched = new Map<string, number>();
  const queue = new Set<string>();
  const inFlight = new Set<string>();
  let timer: ReturnType<typeof setTimeout> | null = null;
  let unconfiguredSince: number | null = null;

  function schedule(): void {
    if (timer !== null) return;
    timer = setTimeout(() => {
      timer = null;
      void flush();
    }, BATCH_DELAY_MS);
  }

  async function flush(): Promise<void> {
    if (unconfiguredSince !== null && now() - unconfiguredSince < UNCONFIGURED_RETRY_MS) {
      queue.clear();
      return;
    }
    const urls = [...queue].filter((u) => !inFlight.has(u)).slice(0, MAX_PER_REQUEST);
    for (const u of urls) {
      queue.delete(u);
      inFlight.add(u);
    }
    if (queue.size > 0) schedule();
    if (urls.length === 0) return;
    try {
      const answer = await fetch()(urls);
      if (!answer.configured) {
        unconfiguredSince = now();
        return;
      }
      unconfiguredSince = null;
      const at = now();
      const update: Record<string, PrStatus | null> = {};
      for (const u of urls) {
        if (u in answer.statuses) {
          update[u] = answer.statuses[u] ?? null;
          fetched.set(u, at);
        }
      }
      setStatuses(update);
    } catch {
      // Offline or GitHub unreachable: keep what is shown, try again on the next refresh.
    } finally {
      for (const u of urls) inFlight.delete(u);
    }
  }

  function want(url: string): void {
    if (inFlight.has(url) || queue.has(url)) return;
    queue.add(url);
    schedule();
  }

  function refresh(): void {
    const t = now();
    for (const [url, at] of seen) {
      if (t - at > FORGET_MS) seen.delete(url);
      else if (t - (fetched.get(url) ?? 0) >= REFRESH_MS / 2) want(url);
    }
  }

  const interval = setInterval(refresh, REFRESH_MS);
  const onVisible = () => {
    if (typeof document === "undefined" || document.visibilityState === "visible") refresh();
  };
  if (typeof document !== "undefined") document.addEventListener("visibilitychange", onVisible);

  return {
    status(url) {
      const key = url.trim();
      if (!isGithubPr(key)) return undefined;
      const value = statuses[key];
      const first = !seen.has(key);
      seen.set(key, now());
      if (first && !fetched.has(key)) want(key);
      return value;
    },
    refresh,
    dispose() {
      clearInterval(interval);
      if (timer !== null) clearTimeout(timer);
      if (typeof document !== "undefined") document.removeEventListener("visibilitychange", onVisible);
      setStatuses(reconcile({}));
    },
  };
}

/** Same rule as the server: `https://github.com/{owner}/{repo}/pull/{n}…`. */
export function isGithubPr(url: string): boolean {
  return /^https?:\/\/(www\.)?github\.com\/[\w.-]+\/[\w.-]+\/pull\/[1-9]\d*(?:[/?#]|$)/i.test(url.trim());
}

let shared: PrTracker | null = null;

/** Status of the PR behind `url` for the UI (see {@link PrTracker.status}). */
export function prStatus(url: string): PrStatus | null | undefined {
  shared ??= createPrTracker(() => (urls) => api().prStatus(urls));
  return shared.status(url);
}
