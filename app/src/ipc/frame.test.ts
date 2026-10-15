import { describe, expect, it } from "vitest";
import { decodeFrame, FrameError } from "./frame";

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

describe("decodeFrame", () => {
  it("reads the header and the pixels the host writes", () => {
    const frame = decodeFrame(encoded());
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
    const buffer = encoded();
    const bytes = new Uint8Array(buffer);
    bytes[5] = 0;
    bytes[7] = 0;
    new DataView(buffer).setBigUint64(24, 1_700_000_000n, true);
    const frame = decodeFrame(buffer);
    expect(frame.refresh).toBe("full");
    expect(frame.region).toBeNull();
    expect(frame.durationNanos).toBe(1_700_000_000);
  });

  it("reads continuous shades from a 0 in the levels byte", () => {
    const buffer = encoded();
    new Uint8Array(buffer)[6] = 0;
    expect(decodeFrame(buffer).shades).toBe("continuous");
  });

  it("refuses what is not a frame", () => {
    expect(() => decodeFrame(new ArrayBuffer(10))).toThrow(FrameError);
    const truncated = encoded().slice(0, 50);
    expect(() => decodeFrame(truncated)).toThrow(/holds 2 pixels/);
    const older = encoded();
    new Uint8Array(older)[4] = 1;
    expect(() => decodeFrame(older)).toThrow(/version 1/);
    const single = encoded();
    new Uint8Array(single)[6] = 1;
    expect(() => decodeFrame(single)).toThrow(/1 grey level/);
  });
});
