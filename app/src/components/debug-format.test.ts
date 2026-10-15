import { describe, expect, it } from "vitest";
import { debugView } from "../test/fake-host";
import {
  hexByte,
  hiddenRegions,
  memoryRows,
  parseAddress,
  parseLength,
  textOf,
} from "./debug-format";

describe("parseAddress", () => {
  it("reads hexadecimal with or without 0x, up to 8 digits", () => {
    expect(parseAddress("0x3fc80000")).toBe(0x3fc8_0000);
    expect(parseAddress("  3FC80000 ")).toBe(0x3fc8_0000);
    expect(parseAddress("0XFFFFFFFF")).toBe(0xffff_ffff);
    expect(parseAddress("0")).toBe(0);
  });

  it("refuses what is no address", () => {
    for (const text of ["", "0x", "0x123456789", "12g", "-4", "0x 1", "1.5"]) {
      expect(parseAddress(text), text).toBeNull();
    }
  });
});

describe("parseLength", () => {
  it("reads 1 to 4096 bytes in decimal", () => {
    expect(parseLength("4")).toBe(4);
    expect(parseLength(" 4096 ")).toBe(4096);
    for (const text of ["0", "4097", "", "0x4", "-1", "2.5"]) {
      expect(parseLength(text), text).toBeNull();
    }
  });
});

describe("memory rows", () => {
  it("cut the bytes 16 to a row, each with its address", () => {
    const bytes = Array.from({ length: 20 }, (_, i) => i);
    const rows = memoryRows({ address: 0xffff_fff0, bytes, regions: [] });
    expect(rows.map((row) => row.address)).toEqual([0xffff_fff0, 0x0000_0000]);
    expect(rows[1]?.bytes).toEqual([16, 17, 18, 19]);
  });

  it("show a byte not shown as dots, and as a space in the text", () => {
    expect(hexByte(0x0a)).toBe("0a");
    expect(hexByte(null)).toBe("··");
    expect(textOf([0x41, 0x00, null, 0x7e, 0x7f])).toBe("A. ~.");
  });

  it("name the regions whose bytes are not shown", () => {
    const memory = debugView().memory;
    expect(memory).not.toBeNull();
    if (memory !== null) {
      expect(hiddenRegions(memory)).toEqual([{ start: 0x3fc8_0004, length: 2, region: "flash" }]);
    }
  });
});
