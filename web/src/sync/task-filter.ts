// Search and filter inside one list or board (project, area, Dev, Lose): a text query plus
// type and weight chips. Pure, so the rule is testable without the store.

import type { WeightKey } from "./balance";
import type { Task, TaskType } from "./types";

export interface TaskFilter {
  /** Free text; every word must occur (order does not matter). */
  query: string;
  /** Empty = every type. */
  types: readonly TaskType[];
  /** Empty = every weight; "none" = not weighed. */
  weights: readonly WeightKey[];
}

export const EMPTY_FILTER: TaskFilter = { query: "", types: [], weights: [] };

/** Lower case without umlauts and accents: "Prüfung" finds "pruefung" and "prufung" alike. */
export function normalizeText(text: string): string {
  return text
    .toLowerCase()
    .replace(/ä/g, "ae")
    .replace(/ö/g, "oe")
    .replace(/ü/g, "ue")
    .replace(/ß/g, "ss")
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "");
}

/** The query split into normalized words. */
export function queryTerms(query: string): string[] {
  return normalizeText(query).split(/\s+/).filter(Boolean);
}

export function isFilterActive(filter: TaskFilter): boolean {
  return filter.query.trim() !== "" || filter.types.length > 0 || filter.weights.length > 0;
}

/** What the text query searches in one task: reference, title, description, plan, branch, PR, whom it waits on. */
function haystackOf(task: Task): string {
  return [task.ref, task.title, task.description, task.plan, task.branch, task.pr_url, task.waiting_on].filter(Boolean).join(" ");
}

/** Haystack for umlaut-tolerant matching: "ae" also stands as "a", so "prufung" finds "Prüfung". */
function searchable(text: string): string {
  const n = normalizeText(text);
  const plain = normalizeText(text.replace(/[äöü]/gi, (c) => ({ ä: "a", ö: "o", ü: "u", Ä: "a", Ö: "o", Ü: "u" })[c] ?? c));
  return n === plain ? n : `${n} ${plain}`;
}

/**
 * Whether a root task passes the filter. Type and weight look at the task itself; the text
 * also searches its live subtasks, so a hit deep in an epic still shows the epic.
 */
export function matchesFilter(task: Task, filter: TaskFilter, subtasks: () => readonly Task[] = () => []): boolean {
  if (filter.types.length > 0 && !filter.types.includes(task.type)) return false;
  if (filter.weights.length > 0 && !filter.weights.includes(task.weight ?? "none")) return false;
  const terms = queryTerms(filter.query);
  if (terms.length === 0) return true;
  const own = searchable(haystackOf(task));
  if (terms.every((t) => own.includes(t))) return true;
  const all = `${own} ${subtasks()
    .filter((s) => !s.deleted)
    .map((s) => searchable(haystackOf(s)))
    .join(" ")}`;
  return terms.every((t) => all.includes(t));
}
