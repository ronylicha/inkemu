import { describe, expect, it } from "vitest";
import { en } from "../i18n/en";
import { fr } from "../i18n/fr";
import { translator } from "../i18n/i18n";
import { stopText, watchText } from "./debug-views";

const french = translator(fr);
const english = translator(en);

const read = (size: number) =>
  ({
    kind: "watchpoint",
    address: 0x3fc8_0000,
    size,
    access: "read",
    value: 0x2a,
    pc: 0x4000_0008,
  }) as const;

describe("the bytes of a watchpoint and of its stop", () => {
  it("are one byte, in the singular", () => {
    expect(stopText(english, read(1))).toBe(
      "Stopped: the instruction at 0x40000008 read 1 byte at 0x3fc80000 (value 0x2a).",
    );
    expect(stopText(french, read(1))).toBe(
      "Arrêté : l’instruction à 0x40000008 a lu 1 octet à 0x3fc80000 (valeur 0x2a).",
    );
    const one = { address: 0x3fc8_0000, length: 1, on: "write" } as const;
    expect(watchText(english, one)).toBe("1 byte at 0x3fc80000, writes");
    expect(watchText(french, one)).toMatch(/^1 octet à 0x3fc80000, /);
  });

  it("are several bytes, in the plural", () => {
    expect(stopText(english, read(2))).toContain("read 2 bytes at");
    expect(stopText(french, read(4))).toContain("a lu 4 octets à");
    const many = { address: 0x3fc8_0000, length: 4096, on: "access" } as const;
    expect(watchText(english, many)).toMatch(/^4096 bytes at /);
    expect(watchText(french, many)).toMatch(/^4096 octets à /);
  });
});
