import { describe, expect, it } from "vitest";
import { sourceLabel } from "../src/labels";

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
