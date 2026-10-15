// What the debugger panel computes from what the host sent: the addresses a user types, the rows
// of a memory view, the regions whose bytes are not shown. Nothing here decides what is readable:
// the host does (plan ruling 4a-R8).

import type { MemoryView, RegionSpan } from "../ipc/types";

const ADDRESS = /^(?:0[xX])?([0-9a-fA-F]{1,8})$/;
const LENGTH = /^[0-9]{1,4}$/;
export const MAX_WATCH_LENGTH = 4096;

/** The address `text` names: hexadecimal, `0x` optional, 1 to 8 digits; `null` otherwise. */
export function parseAddress(text: string): number | null {
  const digits = ADDRESS.exec(text.trim())?.[1];
  return digits === undefined ? null : Number.parseInt(digits, 16);
}

/** The bytes of a watchpoint `text` names: 1 to 4096, in decimal; `null` otherwise. */
export function parseLength(text: string): number | null {
  const trimmed = text.trim();
  if (!LENGTH.test(trimmed)) {
    return null;
  }
  const length = Number.parseInt(trimmed, 10);
  return length >= 1 && length <= MAX_WATCH_LENGTH ? length : null;
}

export interface MemoryRow {
  address: number;
  bytes: (number | null)[];
}

/** The bytes of `memory`, `perRow` to a row, each row with the address of its first byte. */
export function memoryRows(memory: MemoryView, perRow = 16): MemoryRow[] {
  const rows: MemoryRow[] = [];
  for (let offset = 0; offset < memory.bytes.length; offset += perRow) {
    rows.push({
      address: (memory.address + offset) >>> 0,
      bytes: memory.bytes.slice(offset, offset + perRow),
    });
  }
  return rows;
}

/** A byte in two hexadecimal digits; a byte not shown as two middle dots. */
export function hexByte(byte: number | null): string {
  return byte === null ? "··" : byte.toString(16).padStart(2, "0");
}

/** The bytes as text: a printable ASCII character as itself, another byte as `.`, a byte not
 * shown as a space. */
export function textOf(bytes: readonly (number | null)[]): string {
  return bytes
    .map((byte) => {
      if (byte === null) {
        return " ";
      }
      return byte >= 0x20 && byte < 0x7f ? String.fromCharCode(byte) : ".";
    })
    .join("");
}

/** The regions of `memory` whose bytes are not shown, and why: the flash, registers, nothing. */
export function hiddenRegions(memory: MemoryView): RegionSpan[] {
  return memory.regions.filter(
    (span) =>
      span.region === "flash" ||
      span.region === "unbacked" ||
      span.region === "registers" ||
      span.region === "unmapped",
  );
}
