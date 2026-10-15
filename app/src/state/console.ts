// What a console panel keeps of the text a console gave: without its colour escapes, its line
// ends made one, and bounded, the oldest lines dropped first.

export const MAX_CONSOLE_CHARS = 200_000;

const ESCAPE = String.fromCharCode(27);
// CSI sequences (colours of the ESP-IDF log, cursor moves): ESC [ parameters intermediates final.
const CSI = new RegExp(`${ESCAPE}\\[[0-?]*[ -/]*[@-~]`, "g");

/** `text` without its terminal escapes, CR LF as LF and other CRs dropped. */
export function cleanText(text: string): string {
  return text.replace(CSI, "").replace(/\r\n/g, "\n").replace(/\r/g, "");
}

/**
 * `shown` followed by `text`, cleaned, at most `limit` characters: when longer, the oldest lines
 * go, from the start of a line.
 */
export function appendConsole(shown: string, text: string, limit = MAX_CONSOLE_CHARS): string {
  const all = shown + cleanText(text);
  if (all.length <= limit) {
    return all;
  }
  const cut = all.length - limit;
  const nextLine = all.indexOf("\n", cut);
  return nextLine === -1 ? all.slice(cut) : all.slice(nextLine + 1);
}

export type LineEnding = "lf" | "cr" | "crlf" | "none";
export const LINE_ENDINGS: readonly LineEnding[] = ["lf", "cr", "crlf", "none"];

const ENDINGS: Record<LineEnding, string> = { lf: "\n", cr: "\r", crlf: "\r\n", none: "" };

/** The bytes a line typed by the user sends: its UTF-8 and the line ending chosen. */
export function lineBytes(line: string, ending: LineEnding): number[] {
  return [...new TextEncoder().encode(line + ENDINGS[ending])];
}
