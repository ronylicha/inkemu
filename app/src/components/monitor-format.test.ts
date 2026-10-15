import { describe, expect, it } from "vitest";
import { charge, count, current, duration, kibibytes, percent, speedRatio } from "./monitor-format";

/** The spaces of the French formats: narrow between groups, plain non-breaking before a unit. */
const NARROW = " ";
const NBSP = " ";

describe("monitor formats", () => {
  it("groups counts and writes shares in the language of the interface", () => {
    expect(count(40_000, "en")).toBe("40,000");
    expect(count(40_000, "fr")).toBe(`40${NARROW}000`);
    expect(percent(625, "en")).toBe("62.5%");
    expect(percent(625, "fr")).toBe(`62,5${NBSP}%`);
    expect(percent(1000, "en")).toBe("100.0%");
  });

  it("writes a duration in the unit that keeps it short", () => {
    expect(duration(250, "en")).toBe(`250${NBSP}ns`);
    expect(duration(1_500, "en")).toBe(`1.5${NBSP}µs`);
    expect(duration(250_000, "fr")).toBe(`250${NBSP}µs`);
    expect(duration(1_500_000, "fr")).toBe(`1,5${NBSP}ms`);
    expect(duration(12_345_000_000, "en")).toBe(`12.3${NBSP}s`);
  });

  it("writes currents and charges in µA or mA, nAh, µAh or mAh", () => {
    expect(current(5, "en")).toBe(`5${NBSP}µA`);
    expect(current(23_000, "fr")).toBe(`23${NBSP}mA`);
    expect(charge(0, "en")).toBe(`0${NBSP}µAh`);
    expect(charge(0.0025, "en")).toBe(`2.5${NBSP}nAh`);
    expect(charge(39_000, "fr")).toBe(`39${NBSP}mAh`);
    expect(charge(12.5, "en")).toBe(`12.5${NBSP}µAh`);
  });

  it("writes sizes and the speed ratio", () => {
    expect(kibibytes(393_216, "KiB", "en")).toBe(`384${NBSP}KiB`);
    expect(speedRatio(0.5, "en")).toBe("×0.50");
    expect(speedRatio(1.25, "fr")).toBe("×1,25");
  });
});
