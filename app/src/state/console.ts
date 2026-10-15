// What a console panel keeps of the text a console gave: without its colour escapes, its line
// ends made one, and bounded, the oldest lines dropped first.

export const MAX_CONSOLE_CHARS = 200_000;

const ESCAPE = String.fromCharCode(27);
// CSI sequences (colours of the ESP-IDF log, cursor moves): ESC [ parameters intermediates final.
const CSI = new RegExp(`${ESCAPE}\\[[0-?]*[ -/]*[@-~]`, "g");

// The start of an escape that has not ended yet: ESC alone, or ESC [ and its parameters.
const OPEN_ESCAPE = new RegExp(`${ESCAPE}(\\[[0-?]*[ -/]*)?$`);
/** Longer than any escape a console sends: what is still open past it is text, not an escape. */
const MAX_OPEN_ESCAPE = 32;

/**
 * `text` split where an escape it ends with is still open: the update that carries the rest of
 * it comes next (a console's text arrives in pieces, cut anywhere). The first part is ready to
 * show; the second waits to be put before the next piece of the same console.
 */
export function holdOpenEscape(text: string): [ready: string, held: string] {
  const open = OPEN_ESCAPE.exec(text);
  if (open === null || text.length - open.index > MAX_OPEN_ESCAPE) {
    return [text, ""];
  }
  return [text.slice(0, open.index), text.slice(open.index)];
}

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
