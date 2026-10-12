import { describe, expect, it } from "vitest";
import { findTaskRefs } from "../src/components/editor/task-refs";

const refs = (text: string) => findTaskRefs(text).map((m) => m.ref);

describe("findTaskRefs", () => {
  it("finds references in running text", () => {
    expect(refs("Siehe LLH-1116 und ZUN-42.")).toEqual(["LLH-1116", "ZUN-42"]);
    expect(refs("(LLH-1116), LLH-7: erledigt")).toEqual(["LLH-1116", "LLH-7"]);
    const [m] = findTaskRefs("vor ZUN-42 nach");
    expect(m).toEqual({ ref: "ZUN-42", start: 4, end: 10 });
  });

  it("ignores what only looks similar", () => {
    expect(refs("zun-42 ZUN-042 ZUN-0 Z-1x ABCDEFGHIJK-1")).toEqual([]);
    expect(refs("xZUN-42 ZUN-42a ÄZUN-1 ZUN-1-2 a-ZUN-3")).toEqual([]);
    expect(refs("ZUN-1234567890")).toEqual([]);
  });

  it("accepts keys with digits and the longest form", () => {
    expect(refs("A1-1 ABCDEFGHIJ-123456789")).toEqual(["A1-1", "ABCDEFGHIJ-123456789"]);
  });
});
