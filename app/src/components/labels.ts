// Labels the interface builds from names the host sends: a known name has its translation, an
// unknown one (a button of a future device) is shown as the host names it.

import { fr } from "../i18n/fr";
import type { Key, Translate } from "../i18n/i18n";

function isKey(key: string): key is Key {
  return key in fr;
}

/** The translation of `prefix.name`, with `params` in it, else `name` itself. */
export function labelOf(
  t: Translate,
  prefix: string,
  name: string,
  params?: Record<string, string | number>,
): string {
  const key = `${prefix}.${name}`;
  return isKey(key) ? t(key, params) : name;
}

/** Device time in nanoseconds as seconds with three decimals: `12.345 s`. */
export function seconds(nanos: number): string {
  return `${(nanos / 1e9).toFixed(3)} s`;
}

/** A frequency in the largest unit that keeps it whole, as the command line writes it. */
export function frequency(hz: number): string {
  if (hz % 1_000_000 === 0) {
    return `${hz / 1_000_000} MHz`;
  }
  if (hz % 1_000 === 0) {
    return `${hz / 1_000} kHz`;
  }
  return `${hz} Hz`;
}

export function hex32(value: number): string {
  return `0x${value.toString(16).padStart(8, "0")}`;
}
