import { describe, expect, it } from "vitest";
import { appendConsole, cleanText, lineBytes } from "./console";

const ESC = String.fromCharCode(27);

describe("console text", () => {
  it("loses the colours of the ESP-IDF log and keeps one line end", () => {
    expect(cleanText(`${ESC}[0;32mI (31) boot: ok${ESC}[0m\r\n`)).toBe("I (31) boot: ok\n");
    expect(cleanText("a\rb")).toBe("ab");
  });

  it("keeps a CR LF cut between two slices as one line end", () => {
    expect(appendConsole(appendConsole("", "ready\r"), "\nnext")).toBe("ready\nnext");
  });

  it("drops the oldest lines past its bound, from the start of a line", () => {
    expect(appendConsole("one\ntwo\n", "three\n", 10)).toBe("three\n");
    expect(appendConsole("", "abcdefghijkl", 5)).toBe("hijkl");
  });

  it("sends a typed line as UTF-8 with its line ending", () => {
    expect(lineBytes("é", "crlf")).toEqual([0xc3, 0xa9, 13, 10]);
    expect(lineBytes("ls", "lf")).toEqual([108, 115, 10]);
    expect(lineBytes("x", "none")).toEqual([120]);
  });
});
