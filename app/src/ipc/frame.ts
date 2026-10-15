// The panel image as the host encodes it (`PanelFrame::encode`, crates/host/src/ports.rs): a
// 48-byte little-endian header, then one byte per pixel in the controller's scan order.

import type { RefreshKind, Shades } from "./types";

export const FRAME_HEADER_LEN = 48;
const MAGIC = [0x49, 0x4b, 0x50, 0x46]; // "IKPF"
/** Version 2 says continuous shades by a 0 in byte 6 (`FRAME_VERSION`, ports.rs). */
const VERSION = 2;

export interface Region {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Frame {
  width: number;
  height: number;
  shades: Shades;
  generation: number;
  refresh: RefreshKind;
  region: Region | null;
  /** How long the refresh lasts in device time, when the controller says. */
  durationNanos: number | null;
  pixels: Uint8Array;
}

export class FrameError extends Error {}

/** Reads an encoded frame; throws `FrameError` on bytes that are not one. */
export function decodeFrame(buffer: ArrayBuffer): Frame {
  const bytes = new Uint8Array(buffer);
  if (bytes.length < FRAME_HEADER_LEN || MAGIC.some((byte, i) => bytes[i] !== byte)) {
    throw new FrameError("not a panel frame");
  }
  if (bytes[4] !== VERSION) {
    throw new FrameError(`panel frame version ${bytes[4]} is not ${VERSION}`);
  }
  const view = new DataView(buffer);
  const width = view.getUint32(8, true);
  const height = view.getUint32(12, true);
  const pixels = bytes.subarray(FRAME_HEADER_LEN);
  if (pixels.length !== width * height) {
    throw new FrameError(`a ${width} × ${height} frame holds ${pixels.length} pixels`);
  }
  const duration = view.getBigUint64(24, true);
  const hasRegion = bytes[7] === 1;
  const levels = bytes[6] ?? 0;
  if (levels === 1) {
    throw new FrameError("a panel frame of 1 grey level");
  }
  return {
    width,
    height,
    shades: levels === 0 ? "continuous" : { levels },
    generation: Number(view.getBigUint64(16, true)),
    refresh: bytes[5] === 0 ? "full" : "partial",
    region: hasRegion
      ? {
          x: view.getUint32(32, true),
          y: view.getUint32(36, true),
          width: view.getUint32(40, true),
          height: view.getUint32(44, true),
        }
      : null,
    durationNanos: duration === 0xffff_ffff_ffff_ffffn ? null : Number(duration),
    pixels,
  };
}
