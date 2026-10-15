import { describe, expect, it } from "vitest";
import { decodeFrame, FrameError } from "./frame";

/** The panel of the frames below. */
const PANEL = { width: 3, height: 2 };

/**
 * The layout of `an_encoded_frame_is_its_header_then_its_pixels` in crates/host/src/ports.rs,
 * with a generation a JavaScript number holds exactly (a refresh counter never reaches 2^53).
 */
function encoded(): ArrayBuffer {
  const bytes = new Uint8Array(48 + 6);
  bytes.set([0x49, 0x4b, 0x50, 0x46, 2, 1, 4, 1]);
  const view = new DataView(bytes.buffer);
  view.setUint32(8, 3, true);
  view.setUint32(12, 2, true);
  view.setBigUint64(16, 0x0001_0304_0506_0708n, true);
  view.setBigUint64(24, 0xffff_ffff_ffff_ffffn, true);
  view.setUint32(32, 1, true);
  view.setUint32(36, 0, true);
  view.setUint32(40, 2, true);
  view.setUint32(44, 1, true);
  bytes.set([0, 85, 170, 255, 0, 85], 48);
  return bytes.buffer;
}

/** [`encoded`] changed by `change`. */
function changed(change: (bytes: Uint8Array, view: DataView) => void): ArrayBuffer {
  const buffer = encoded();
  change(new Uint8Array(buffer), new DataView(buffer));
  return buffer;
}

describe("decodeFrame", () => {
  it("reads the header and the pixels the host writes", () => {
    const frame = decodeFrame(encoded(), PANEL);
    expect(frame).toMatchObject({
      width: 3,
      height: 2,
      shades: { levels: 4 },
      generation: 0x0001_0304_0506_0708,
      refresh: "partial",
      region: { x: 1, y: 0, width: 2, height: 1 },
      durationNanos: null,
    });
    expect([...frame.pixels]).toEqual([0, 85, 170, 255, 0, 85]);
  });

  it("reads a full refresh with its duration", () => {
    const buffer = changed((bytes, view) => {
      bytes[5] = 0;
      bytes[7] = 0;
      view.setBigUint64(24, 1_700_000_000n, true);
    });
    const frame = decodeFrame(buffer, PANEL);
    expect(frame.refresh).toBe("full");
    expect(frame.region).toBeNull();
    expect(frame.durationNanos).toBe(1_700_000_000);
  });

  it("reads continuous shades from a 0 in the levels byte", () => {
    const buffer = changed((bytes) => {
      bytes[6] = 0;
    });
    expect(decodeFrame(buffer, PANEL).shades).toBe("continuous");
  });

  it("refuses what is not a frame", () => {
    expect(() => decodeFrame(new ArrayBuffer(10), PANEL)).toThrow(FrameError);
    const truncated = encoded().slice(0, 50);
    expect(() => decodeFrame(truncated, PANEL)).toThrow(/holds 2 pixels/);
    const older = changed((bytes) => {
      bytes[4] = 1;
    });
    expect(() => decodeFrame(older, PANEL)).toThrow(/version 1/);
    const single = changed((bytes) => {
      bytes[6] = 1;
    });
    expect(() => decodeFrame(single, PANEL)).toThrow(/1 grey level/);
  });

  it("refuses a frame of another size than the panel's", () => {
    expect(() => decodeFrame(encoded(), { width: 2, height: 3 })).toThrow(
      /a 3 × 2 frame is not of the 2 × 3 panel/,
    );
  });

  it("refuses a flag byte other than 0 or 1", () => {
    for (const at of [5, 7]) {
      const buffer = changed((bytes) => {
        bytes[at] = 2;
      });
      expect(() => decodeFrame(buffer, PANEL), `byte ${at}`).toThrow(/is 2, not 0 or 1/);
    }
  });

  it("refuses the glass at power-on, generation 0", () => {
    const buffer = changed((_, view) => view.setBigUint64(16, 0n, true));
    expect(() => decodeFrame(buffer, PANEL)).toThrow(/generation 0/);
  });

  it("refuses a region outside the image or empty", () => {
    const regions: [number, number, number, number][] = [
      [2, 0, 2, 1],
      [0, 1, 1, 2],
      [0, 0, 0, 1],
      [0, 0, 1, 0],
    ];
    for (const [x, y, width, height] of regions) {
      const buffer = changed((_, view) => {
        view.setUint32(32, x, true);
        view.setUint32(36, y, true);
        view.setUint32(40, width, true);
        view.setUint32(44, height, true);
      });
      expect(() => decodeFrame(buffer, PANEL), `${x} ${y} ${width} ${height}`).toThrow(
        /is not inside the 3 × 2 frame/,
      );
    }
  });
});
