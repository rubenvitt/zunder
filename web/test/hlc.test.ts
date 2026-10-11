import { describe, expect, it } from "vitest";
import { compareHlc, formatHlc, HlcClock, isValidHlc, parseHlc } from "../src/sync/hlc";

describe("HLC format", () => {
  it("pads wall_ms to 13 and counter to 5 digits", () => {
    expect(formatHlc({ wallMs: 1728650000000, counter: 0, node: "web-3f9a" })).toBe("1728650000000:00000:web-3f9a");
    expect(formatHlc({ wallMs: 42, counter: 7, node: "server" })).toBe("0000000000042:00007:server");
  });

  it("round-trips through parse", () => {
    expect(parseHlc("1728650000000:00012:import")).toEqual({ wallMs: 1728650000000, counter: 12, node: "import" });
    expect(parseHlc("1728650000000:12:import")).toBeNull();
    expect(parseHlc("1728650000000:00012:Import")).toBeNull();
    expect(isValidHlc("1728650000000:00000:" + "a".repeat(36))).toBe(true);
    expect(isValidHlc("1728650000000:00000:" + "a".repeat(37))).toBe(false);
  });

  it("rejects out-of-range parts", () => {
    expect(() => formatHlc({ wallMs: 1, counter: 100000, node: "x" })).toThrow();
    expect(() => formatHlc({ wallMs: 1, counter: 0, node: "UPPER" })).toThrow();
  });

  it("orders lexicographically like the tuple", () => {
    const sorted = [
      "1728650000000:00000:a",
      "1728650000000:00000:b",
      "1728650000000:00001:a",
      "1728650000001:00000:a",
    ];
    const shuffled = [sorted[3]!, sorted[1]!, sorted[0]!, sorted[2]!];
    expect([...shuffled].sort(compareHlc)).toEqual(sorted);
  });
});

describe("HlcClock", () => {
  it("uses physical time and increments the counter within one millisecond", () => {
    let t = 1000;
    const clock = new HlcClock("web-a", () => t);
    expect(clock.now()).toBe("0000000001000:00000:web-a");
    expect(clock.now()).toBe("0000000001000:00001:web-a");
    t = 1005;
    expect(clock.now()).toBe("0000000001005:00000:web-a");
  });

  it("never goes backwards when the physical clock does", () => {
    let t = 2000;
    const clock = new HlcClock("web-a", () => t);
    const a = clock.now();
    t = 1500;
    const b = clock.now();
    expect(b > a).toBe(true);
    expect(b).toBe("0000000002000:00001:web-a");
  });

  it("receive raises the clock to the remote timestamp", () => {
    const clock = new HlcClock("web-a", () => 1000);
    clock.receive("0000000005000:00003:server");
    expect(clock.now()).toBe("0000000005000:00004:web-a");
    clock.receive("0000000004000:00009:server");
    expect(clock.now()).toBe("0000000005000:00005:web-a");
  });

  it("ignores malformed remote timestamps", () => {
    const clock = new HlcClock("web-a", () => 1000);
    clock.receive("garbage");
    expect(clock.now()).toBe("0000000001000:00000:web-a");
  });

  it("borrows the next millisecond when the counter overflows", () => {
    const clock = new HlcClock("web-a", () => 1000);
    clock.receive("0000000001000:99999:server");
    expect(clock.now()).toBe("0000000001001:00000:web-a");
  });

  it("rejects invalid node ids", () => {
    expect(() => new HlcClock("Web A")).toThrow();
  });
});
