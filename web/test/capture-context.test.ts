import { describe, expect, it } from "vitest";
import { captureContext } from "../src/capture/context";

const lookup = {
  hasArea: (id: string) => id === "a1",
  hasProject: (id: string) => id === "p1",
};

describe("captureContext", () => {
  it("preselects the list of the current view", () => {
    expect(captureContext({ view: "lose" }, lookup)).toEqual({ kind: "loose" });
    expect(captureContext({ view: "bereiche", kind: "area", id: "a1" }, lookup)).toEqual({ kind: "area", id: "a1" });
    expect(captureContext({ view: "bereiche", kind: "project", id: "p1" }, lookup)).toEqual({ kind: "project", id: "p1" });
  });

  it("keeps the inbox for views that are not a location", () => {
    for (const view of ["heute", "inbox", "dev", "tokens", "einstellungen", "verbindung"] as const) {
      expect(captureContext({ view }, lookup)).toBeNull();
    }
    expect(captureContext({ view: "phoenix", fire: "2026-W41" }, lookup)).toBeNull();
    expect(captureContext({ view: "aufgabe", id: "t1" }, lookup)).toBeNull();
    expect(captureContext({ view: "bereiche" }, lookup)).toBeNull();
  });

  it("ignores unknown or deleted areas and projects", () => {
    expect(captureContext({ view: "bereiche", kind: "area", id: "gone" }, lookup)).toBeNull();
    expect(captureContext({ view: "bereiche", kind: "project", id: "gone" }, lookup)).toBeNull();
    // An area ID in a project route is not a project.
    expect(captureContext({ view: "bereiche", kind: "project", id: "a1" }, lookup)).toBeNull();
  });
});
