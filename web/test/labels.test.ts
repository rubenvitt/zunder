import { describe, expect, it } from "vitest";
import { emptyBalance } from "../src/sync/balance";
import { parseCapture } from "../src/capture/parse";
import { CAPTURE_SHORTHAND, sourceLabel, weekForWhomLine, weekWeightLine } from "../src/labels";

describe("sourceLabel", () => {
  it("names the origin of a history entry", () => {
    expect(sourceLabel("web")).toBe("Web");
    expect(sourceLabel("server")).toBe("Server");
    expect(sourceLabel("api:import")).toBe("Import");
    expect(sourceLabel("api:funke")).toBe("API · funke");
    expect(sourceLabel("mcp:claude-code")).toBe("Agent · claude-code");
    expect(sourceLabel("mcp:oauth:Claude")).toBe("Agent · Claude (OAuth)");
    expect(sourceLabel("app:RPhone-16 (iOS)")).toBe("App · RPhone-16 (iOS)");
    expect(sourceLabel("intake:iuk-suite")).toBe("Eingang · iuk-suite");
  });
});

describe("Wochenspiegel lines", () => {
  it("name only the groups that have something, in a fixed order", () => {
    const b = emptyBalance("2026-W42");
    expect(weekWeightLine(b)).toBe("Diese Woche ist noch nichts erledigt.");
    Object.assign(b, { total: 16 });
    Object.assign(b.by_weight, { rauch: 11, glut: 3, none: 2 });
    Object.assign(b.by_for_whom, { andere: 9, mich: 5, none: 2 });
    expect(weekWeightLine(b)).toBe("Diese Woche: 3 Glut, 11 Rauch, 2 ohne Gewicht.");
    expect(weekForWhomLine(b)).toBe("Für wen: 5 für mich, 9 für andere, 2 ohne Angabe.");
  });
});

describe("quick capture shorthand examples", () => {
  const structure = { areas: [{ id: "a", name: "Bereich" }], projects: [{ id: "p", name: "Projekt" }] };
  it.each(CAPTURE_SHORTHAND.flatMap((item) => item.examples.map((example) => [example, item.meaning])))("%s is read by the parser (%s)", (example) => {
    const r = parseCapture(`Aufgabe ${example}`, new Date(2026, 9, 18, 8, 0), structure);
    expect(r.title).toBe("Aufgabe");
    expect(r.unknownTags).toEqual([]);
    expect(r.tokens.length).toBeGreaterThan(0);
  });
});
