// Sharing from other apps (iOS, macOS): the app's share extension leaves each
// shared link or text in a folder the app reads (share_take); here they become
// inbox tasks, and the app removes them afterwards (share_done). Only loaded in
// the Tauri app (state/desktop.ts).

import type { SharedItem } from "../platform/tauri";

/** Longer titles are cut; the full first line then goes into the description. */
export const SHARED_TITLE_MAX = 200;

const URL_ONLY = /^https?:\/\/\S+$/i;

export interface SharedTask {
  title: string;
  /** Markdown: the link and the remaining lines of the text. */
  description: string | null;
}

/** "example.org/artikel/42" for a link without its own title. */
export function linkLabel(url: string): string {
  try {
    const u = new URL(url);
    const path = u.pathname.replace(/\/+$/, "");
    const label = `${u.host.replace(/^www\./, "")}${path}`;
    return label || url;
  } catch {
    return url;
  }
}

function shorten(text: string): string {
  if (text.length <= SHARED_TITLE_MAX) return text;
  return `${text.slice(0, SHARED_TITLE_MAX - 1).trimEnd()}…`;
}

/** Title and description of the task for a shared item; null when it holds nothing. */
export function sharedTask(item: SharedItem): SharedTask | null {
  const lines = (item.text ?? "")
    .replace(/\r\n?/g, "\n")
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
  let url = item.url?.trim() || null;
  // Some apps share a link as plain text.
  if (!url && lines.length > 0 && URL_ONLY.test(lines[0]!)) url = lines.shift()!;
  if (url && lines[0] === url) lines.shift();

  const first = lines.shift();
  const title = first ? shorten(first) : url ? shorten(linkLabel(url)) : "";
  if (!title) return null;

  const body: string[] = [];
  if (first && first !== title) body.push(first);
  if (url) body.push(url);
  if (lines.length > 0) body.push(lines.join("\n"));
  return { title, description: body.length > 0 ? body.join("\n\n") : null };
}

export interface ShareImportDeps {
  take(): Promise<SharedItem[]>;
  done(ids: string[]): Promise<unknown>;
  /** Creates the inbox task; the id or null when refused. */
  create(task: SharedTask): string | null;
  /** Called with the titles of the new tasks (at least one). */
  created?(titles: string[]): void;
}

/**
 * Turns waiting shared items into inbox tasks, one round at a time: calls while a
 * round runs start another one afterwards (an event during the round may announce
 * new items). Items whose task was created are not created again, even if removing
 * them failed; refused ones stay for the next start.
 */
export function shareImporter(deps: ShareImportDeps): () => Promise<void> {
  const handled = new Set<string>();
  const refused = new Set<string>();
  let running: Promise<void> | null = null;
  let again = false;

  async function round(): Promise<void> {
    const items = await deps.take();
    const done: string[] = [];
    const titles: string[] = [];
    for (const item of items) {
      if (handled.has(item.id)) {
        done.push(item.id);
        continue;
      }
      if (refused.has(item.id)) continue;
      const task = sharedTask(item);
      if (!task) {
        done.push(item.id);
        continue;
      }
      if (deps.create(task)) {
        handled.add(item.id);
        done.push(item.id);
        titles.push(task.title);
      } else {
        refused.add(item.id);
      }
    }
    if (titles.length > 0) deps.created?.(titles);
    if (done.length > 0) await deps.done(done);
  }

  return function run(): Promise<void> {
    if (running) {
      again = true;
      return running;
    }
    running = (async () => {
      try {
        do {
          again = false;
          try {
            await round();
          } catch (err) {
            console.warn("Geteilte Einträge nicht übernommen", err);
            again = false;
          }
        } while (again);
      } finally {
        running = null;
      }
    })();
    return running;
  };
}

/** Toast after an import. */
export function sharedMessage(titles: string[]): string {
  if (titles.length === 1) {
    const title = titles[0]!;
    const short = title.length > 60 ? `${title.slice(0, 59).trimEnd()}…` : title;
    return `„${short}“ liegt jetzt in der Inbox.`;
  }
  return `${titles.length} geteilte Einträge liegen jetzt in der Inbox.`;
}
