import { describe, expect, it, vi } from "vitest";
import type { SharedItem } from "../src/platform/tauri";
import { linkLabel, SHARED_TITLE_MAX, sharedMessage, sharedTask, shareImporter, type SharedTask } from "../src/state/share";

describe("sharedTask", () => {
  it("takes the first line as title and keeps link and rest in the description", () => {
    expect(sharedTask({ id: "a", text: "Artikel über Zunder\n\nNoch lesen\nbis Freitag", url: "https://example.org/a" })).toEqual({
      title: "Artikel über Zunder",
      description: "https://example.org/a\n\nNoch lesen\nbis Freitag",
    });
  });

  it("names a link without title after host and path", () => {
    expect(sharedTask({ id: "a", text: "", url: "https://www.example.org/artikel/42/" })).toEqual({
      title: "example.org/artikel/42",
      description: "https://www.example.org/artikel/42/",
    });
    expect(sharedTask({ id: "a", text: null, url: "https://example.org" })?.title).toBe("example.org");
  });

  it("reads a link shared as plain text", () => {
    expect(sharedTask({ id: "a", text: "https://example.org/x\nAnsehen" })).toEqual({
      title: "Ansehen",
      description: "https://example.org/x",
    });
    expect(sharedTask({ id: "a", text: "https://example.org/x", url: "https://example.org/x" })).toEqual({
      title: "example.org/x",
      description: "https://example.org/x",
    });
  });

  it("keeps plain text without a link", () => {
    expect(sharedTask({ id: "a", text: "  Milch kaufen  \r\n" })).toEqual({ title: "Milch kaufen", description: null });
  });

  it("cuts long titles and keeps the whole line in the description", () => {
    const line = "x".repeat(SHARED_TITLE_MAX + 50);
    const task = sharedTask({ id: "a", text: line })!;
    expect(task.title).toHaveLength(SHARED_TITLE_MAX);
    expect(task.title.endsWith("…")).toBe(true);
    expect(task.description).toBe(line);
  });

  it("ignores empty items", () => {
    expect(sharedTask({ id: "a", text: " \n ", url: " " })).toBeNull();
    expect(sharedTask({ id: "a" })).toBeNull();
  });

  it("labels broken links with the link itself", () => {
    expect(linkLabel("kein link")).toBe("kein link");
  });
});

describe("shareImporter", () => {
  function setup(items: SharedItem[], create: (task: SharedTask) => string | null = () => "task") {
    const waiting = [...items];
    const done: string[][] = [];
    const created: string[][] = [];
    const deps = {
      take: vi.fn(async () => [...waiting]),
      done: vi.fn(async (ids: string[]) => {
        done.push(ids);
        for (const id of ids) waiting.splice(waiting.findIndex((i) => i.id === id), 1);
      }),
      create: vi.fn(create),
      created: (titles: string[]) => created.push(titles),
    };
    return { run: shareImporter(deps), deps, waiting, done, created };
  }

  it("creates one task per item and removes them", async () => {
    const s = setup([
      { id: "a", text: "Eins" },
      { id: "b", url: "https://example.org" },
      { id: "c", text: "  " },
    ]);
    await s.run();
    expect(s.deps.create.mock.calls.map(([t]) => t.title)).toEqual(["Eins", "example.org"]);
    expect(s.done).toEqual([["c", "a", "b"]]);
    expect(s.created).toEqual([["Eins", "example.org"]]);
    expect(s.waiting).toEqual([]);
  });

  it("does nothing when nothing waits", async () => {
    const s = setup([]);
    await s.run();
    expect(s.deps.done).not.toHaveBeenCalled();
    expect(s.created).toEqual([]);
  });

  it("keeps refused items and does not retry them in the same session", async () => {
    const s = setup([{ id: "a", text: "Eins" }], () => null);
    await s.run();
    await s.run();
    expect(s.deps.create).toHaveBeenCalledTimes(1);
    expect(s.done).toEqual([]);
    expect(s.waiting).toHaveLength(1);
  });

  it("never creates a task twice when removing failed", async () => {
    const s = setup([{ id: "a", text: "Eins" }]);
    s.deps.done.mockRejectedValueOnce(new Error("io"));
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await s.run();
    await s.run();
    warn.mockRestore();
    expect(s.deps.create).toHaveBeenCalledTimes(1);
    expect(s.done).toEqual([["a"]]);
  });

  it("removes an item only once its task is stored", async () => {
    const s = setup([{ id: "a", text: "Eins" }], () => "t1");
    let stored = false;
    const run = shareImporter({ ...s.deps, saved: async (ids) => new Set(stored ? ids : []) });
    await run();
    expect(s.done).toEqual([]);
    expect(s.waiting).toHaveLength(1);
    stored = true;
    await run();
    expect(s.deps.create).toHaveBeenCalledTimes(1);
    expect(s.done).toEqual([["a"]]);
  });

  it("runs once more when called during a round", async () => {
    const s = setup([{ id: "a", text: "Eins" }]);
    const first = s.run();
    s.waiting.push({ id: "b", text: "Zwei" });
    const second = s.run();
    await Promise.all([first, second]);
    expect(s.deps.take).toHaveBeenCalledTimes(2);
    expect(s.deps.create.mock.calls.map(([t]) => t.title)).toEqual(["Eins", "Zwei"]);
  });

  it("survives a failing take", async () => {
    const s = setup([{ id: "a", text: "Eins" }]);
    s.deps.take.mockRejectedValueOnce(new Error("unknown command"));
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await s.run();
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
    await s.run();
    expect(s.created).toEqual([["Eins"]]);
  });
});

describe("sharedMessage", () => {
  it("names one task and counts several", () => {
    expect(sharedMessage(["Milch kaufen"])).toBe("„Milch kaufen“ liegt jetzt in der Inbox.");
    expect(sharedMessage(["a", "b", "c"])).toBe("3 geteilte Einträge liegen jetzt in der Inbox.");
    expect(sharedMessage(["y".repeat(80)])).toBe(`„${"y".repeat(59)}…“ liegt jetzt in der Inbox.`);
  });
});
