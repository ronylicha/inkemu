// How the monitor writes the numbers the host computed (design SP4b, 4b-R7): in the language of
// the interface, in the unit that keeps them short. Presentation only: every share, window and
// current comes from the host.

import type { Language } from "../i18n/i18n";

/** A non-breaking space between a number and its unit. */
const NBSP = " ";

function numbers(language: Language, options: Intl.NumberFormatOptions = {}): Intl.NumberFormat {
  return new Intl.NumberFormat(language, options);
}

/** A count, grouped: `40,000` or `40 000`. */
export function count(value: number, language: Language): string {
  return numbers(language).format(value);
}

/** A share in thousandths as a percentage with one decimal: `62.5%`, `62,5 %`. */
export function percent(permille: number, language: Language): string {
  return numbers(language, {
    style: "percent",
    minimumFractionDigits: 1,
    maximumFractionDigits: 1,
  }).format(permille / 1000);
}

/** A number with at most three significant digits, then its unit. */
function short(value: number, unit: string, language: Language): string {
  return `${numbers(language, { maximumSignificantDigits: 3 }).format(value)}${NBSP}${unit}`;
}

/** A duration in nanoseconds, in ns, µs, ms or s. */
export function duration(nanos: number, language: Language): string {
  if (nanos < 1e3) {
    return short(nanos, "ns", language);
  }
  if (nanos < 1e6) {
    return short(nanos / 1e3, "µs", language);
  }
  if (nanos < 1e9) {
    return short(nanos / 1e6, "ms", language);
  }
  return short(nanos / 1e9, "s", language);
}

/** A current in microamperes, in µA or mA. */
export function current(microamps: number, language: Language): string {
  return microamps < 1000
    ? short(microamps, "µA", language)
    : short(microamps / 1000, "mA", language);
}

/** A charge in microampere-hours, in nAh, µAh or mAh. */
export function charge(microampHours: number, language: Language): string {
  if (microampHours === 0) {
    return short(0, "µAh", language);
  }
  if (microampHours < 1) {
    return short(microampHours * 1000, "nAh", language);
  }
  if (microampHours < 1000) {
    return short(microampHours, "µAh", language);
  }
  return short(microampHours / 1000, "mAh", language);
}

/** A size in bytes as kibibytes: `384 Kio`, `384 KiB`. */
export function kibibytes(bytes: number, unit: string, language: Language): string {
  return `${count(bytes / 1024, language)}${NBSP}${unit}`;
}

/** Device time over host time: `×0.50`. */
export function speedRatio(ratio: number, language: Language): string {
  return `×${numbers(language, { minimumFractionDigits: 2, maximumFractionDigits: 2 }).format(ratio)}`;
}
