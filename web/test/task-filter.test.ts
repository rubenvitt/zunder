// Search and filter inside a list or board: text over the task and its subtasks, type and weight chips.

import { describe, expect, it } from "vitest";
import { EMPTY_FILTER, isFilterActive, matchesFilter, normalizeText, queryTerms, type TaskFilter } from "../src/sync/task-filter";
import type { Task } from "../src/sync/types";

function task(id: string, extra: Partial<Task> = {}): Task {
  return {
    id,
    title: id,
    description: null,
    type: "aufgabe",
    status: "open",
    plan: null,
    branch: null,
    pr_url: null,
    waiting_on: null,
    weight: null,
    ref: null,
    deleted: false,
    ...extra,
  } as Task;
}

const f = (extra: Partial<TaskFilter>): TaskFilter => ({ ...EMPTY_FILTER, ...extra });

describe("normalizeText and queryTerms", () => {
  it("folds case, umlauts and accents", () => {
    expect(normalizeText("Prüfung Straße Café")).toBe("pruefung strasse cafe");
    expect(queryTerms("  Login   BUG ")).toEqual(["login", "bug"]);
    expect(queryTerms("   ")).toEqual([]);
  });
});

describe("isFilterActive", () => {
  it("is off for the empty filter and whitespace", () => {
    expect(isFilterActive(EMPTY_FILTER)).toBe(false);
    expect(isFilterActive(f({ query: "  " }))).toBe(false);
    expect(isFilterActive(f({ query: "x" }))).toBe(true);
    expect(isFilterActive(f({ types: ["bug"] }))).toBe(true);
    expect(isFilterActive(f({ weights: ["none"] }))).toBe(true);
  });
});

describe("matchesFilter", () => {
  it("needs every word, in any order and field", () => {
    const t = task("a", { title: "Login reparieren", description: "Fehler nach dem **Update**", ref: "ZUN-42" });
    expect(matchesFilter(t, f({ query: "update login" }))).toBe(true);
    expect(matchesFilter(t, f({ query: "zun-42" }))).toBe(true);
    expect(matchesFilter(t, f({ query: "login logout" }))).toBe(false);
  });

  it("finds umlauts written out or plain", () => {
    const t = task("a", { title: "Prüfung vorbereiten" });
    expect(matchesFilter(t, f({ query: "prüfung" }))).toBe(true);
    expect(matchesFilter(t, f({ query: "pruefung" }))).toBe(true);
    expect(matchesFilter(t, f({ query: "prufung" }))).toBe(true);
  });

  it("searches plan, branch, PR and whom it waits on", () => {
    expect(matchesFilter(task("a", { branch: "feature/suche" }), f({ query: "feature/suche" }))).toBe(true);
    expect(matchesFilter(task("a", { plan: "Index bauen" }), f({ query: "index" }))).toBe(true);
    expect(matchesFilter(task("a", { pr_url: "https://github.com/o/r/pull/7" }), f({ query: "pull/7" }))).toBe(true);
    expect(matchesFilter(task("a", { waiting_on: "Anna" }), f({ query: "anna" }))).toBe(true);
  });

  it("shows the epic when a live subtask matches", () => {
    const epic = task("epic", { title: "Umzug" });
    const subs = [task("s1", { title: "Kartons besorgen" }), task("s2", { title: "Klavier", deleted: true })];
    expect(matchesFilter(epic, f({ query: "kartons" }), () => subs)).toBe(true);
    // Words may be split between the epic and its subtasks.
    expect(matchesFilter(epic, f({ query: "umzug kartons" }), () => subs)).toBe(true);
    expect(matchesFilter(epic, f({ query: "klavier" }), () => subs)).toBe(false);
  });

  it("filters by type and weight, several chips mean either", () => {
    const bug = task("b", { type: "bug", weight: "brand" });
    const feature = task("f", { type: "feature" });
    expect(matchesFilter(bug, f({ types: ["bug"] }))).toBe(true);
    expect(matchesFilter(feature, f({ types: ["bug"] }))).toBe(false);
    expect(matchesFilter(feature, f({ types: ["bug", "feature"] }))).toBe(true);
    expect(matchesFilter(bug, f({ weights: ["brand", "glut"] }))).toBe(true);
    expect(matchesFilter(feature, f({ weights: ["none"] }))).toBe(true);
    expect(matchesFilter(bug, f({ weights: ["none"] }))).toBe(false);
  });

  it("combines text and chips", () => {
    const bug = task("b", { type: "bug", title: "Absturz beim Start" });
    expect(matchesFilter(bug, f({ query: "absturz", types: ["bug"] }))).toBe(true);
    expect(matchesFilter(bug, f({ query: "absturz", types: ["feature"] }))).toBe(false);
  });
});
