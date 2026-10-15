// How a refresh shows: a full refresh of an e-ink panel drives every pixel to black, then to
// white, then to the image (the "flash" a user sees); a partial one changes the image in place.

import type { RefreshKind } from "../ipc/types";

export type FlashStep = { show: "black" | "white"; ms: number };

/** The shortest and the longest a flash lasts on screen, whatever the panel's refresh lasts. */
export const FLASH_MIN_MS = 240;
export const FLASH_MAX_MS = 900;
const DEFAULT_FLASH_MS = 600;

/**
 * The steps before the image: none for a partial refresh or when the user asks for reduced
 * motion; black then white for a full one, half of the refresh's own duration in all, bounded.
 */
export function flashSteps(
  refresh: RefreshKind,
  durationNanos: number | null,
  reducedMotion: boolean,
): FlashStep[] {
  if (refresh === "partial" || reducedMotion) {
    return [];
  }
  const wanted = durationNanos === null ? DEFAULT_FLASH_MS : durationNanos / 2_000_000;
  const total = Math.min(FLASH_MAX_MS, Math.max(FLASH_MIN_MS, wanted));
  return [
    { show: "black", ms: total / 2 },
    { show: "white", ms: total / 2 },
  ];
}
