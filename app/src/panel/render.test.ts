import { describe, expect, it } from "vitest";
import type { Frame } from "../ipc/frame";
import { fitScale, toPicture, turnedSize } from "./render";

/** 3 × 2: 1 2 3 / 4 5 6, as in crates/host/src/capture.rs. */
const frame: Frame = {
  width: 3,
  height: 2,
  grayLevels: 4,
  generation: 1,
  refresh: "full",
  region: null,
  durationNanos: null,
  pixels: new Uint8Array([1, 2, 3, 4, 5, 6]),
};

const greys = (data: Uint8ClampedArray) => [...data].filter((_, i) => i % 4 === 0);

describe("toPicture", () => {
  it("turns the panel clockwise as the host's capture does", () => {
    expect(greys(toPicture(frame, 0).data)).toEqual([1, 2, 3, 4, 5, 6]);
    expect(greys(toPicture(frame, 90).data)).toEqual([4, 1, 5, 2, 6, 3]);
    expect(greys(toPicture(frame, 180).data)).toEqual([6, 5, 4, 3, 2, 1]);
    expect(greys(toPicture(frame, 270).data)).toEqual([3, 6, 2, 5, 1, 4]);
    expect(toPicture(frame, 90)).toMatchObject({ width: 2, height: 3 });
  });

  it("makes each grey an opaque grey pixel", () => {
    const { data } = toPicture(frame, 0);
    expect([...data.slice(0, 8)]).toEqual([1, 1, 1, 255, 2, 2, 2, 255]);
  });
});

describe("sizes", () => {
  it("swap with a quarter turn", () => {
    expect(turnedSize(800, 480, 90)).toEqual({ width: 480, height: 800 });
    expect(turnedSize(800, 480, 180)).toEqual({ width: 800, height: 480 });
  });

  it("scale by whole numbers when the room allows, and shrink to fit otherwise", () => {
    expect(fitScale({ width: 1000, height: 1700 }, { width: 480, height: 800 })).toBe(2);
    expect(fitScale({ width: 959, height: 5000 }, { width: 480, height: 800 })).toBe(1);
    expect(fitScale({ width: 240, height: 800 }, { width: 480, height: 800 })).toBe(0.5);
    expect(fitScale({ width: 0, height: 0 }, { width: 480, height: 800 })).toBe(1);
  });
});
