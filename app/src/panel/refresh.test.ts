import { describe, expect, it } from "vitest";
import { FLASH_MAX_MS, FLASH_MIN_MS, flashSteps } from "./refresh";

describe("flashSteps", () => {
  it("flashes black then white for a full refresh, half its duration in all", () => {
    expect(flashSteps("full", 1_200_000_000, false)).toEqual([
      { show: "black", ms: 300 },
      { show: "white", ms: 300 },
    ]);
  });

  it("bounds the flash", () => {
    const total = (steps: { ms: number }[]) => steps.reduce((sum, step) => sum + step.ms, 0);
    expect(total(flashSteps("full", 60_000_000_000, false))).toBe(FLASH_MAX_MS);
    expect(total(flashSteps("full", 1_000, false))).toBe(FLASH_MIN_MS);
    expect(total(flashSteps("full", null, false))).toBe(600);
  });

  it("does not flash a partial refresh, nor for a user who asks for reduced motion", () => {
    expect(flashSteps("partial", 500_000_000, false)).toEqual([]);
    expect(flashSteps("full", 1_700_000_000, true)).toEqual([]);
  });
});
