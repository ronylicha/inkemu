// What a panel image looks like on a screen: its grey bytes as RGBA, turned as the panel is
// mounted in the device, at a scale that keeps every panel pixel sharp.

import type { Frame } from "../ipc/frame";
import type { Rotation } from "../ipc/types";

export interface Picture {
  width: number;
  height: number;
  /** RGBA, four bytes per pixel, line after line. */
  data: Uint8ClampedArray<ArrayBuffer>;
}

/** The size of the panel as the user of the device sees it. */
export function turnedSize(
  width: number,
  height: number,
  rotation: Rotation,
): { width: number; height: number } {
  return rotation === 90 || rotation === 270 ? { width: height, height: width } : { width, height };
}

/**
 * The panel image of `frame`, turned clockwise by `rotation`, as RGBA: each grey byte becomes an
 * opaque pixel of that grey. The same turning as `host::capture::rotated`.
 */
export function toPicture(frame: Frame, rotation: Rotation): Picture {
  const { width, height, pixels } = frame;
  const size = turnedSize(width, height, rotation);
  const data = new Uint8ClampedArray(size.width * size.height * 4);
  let out = 0;
  for (let row = 0; row < size.height; row += 1) {
    for (let column = 0; column < size.width; column += 1) {
      let x: number;
      let y: number;
      switch (rotation) {
        case 0:
          x = column;
          y = row;
          break;
        case 90:
          x = row;
          y = height - 1 - column;
          break;
        case 180:
          x = width - 1 - column;
          y = height - 1 - row;
          break;
        case 270:
          x = width - 1 - row;
          y = column;
          break;
      }
      const grey = pixels[y * width + x] ?? 0;
      data[out] = grey;
      data[out + 1] = grey;
      data[out + 2] = grey;
      data[out + 3] = 255;
      out += 4;
    }
  }
  return { width: size.width, height: size.height, data };
}

/**
 * The largest scale at which an image of `size` fits in `room`: a whole number when at least 1,
 * so that every panel pixel covers the same number of screen pixels; below 1 the image is
 * shrunk to fit. Never 0.
 */
export function fitScale(
  room: { width: number; height: number },
  size: { width: number; height: number },
): number {
  if (size.width <= 0 || size.height <= 0 || room.width <= 0 || room.height <= 0) {
    return 1;
  }
  const fits = Math.min(room.width / size.width, room.height / size.height);
  return fits >= 1 ? Math.floor(fits) : fits;
}
