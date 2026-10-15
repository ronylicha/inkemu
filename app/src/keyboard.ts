// The keys that press the buttons of a device while it has the focus.

const KEYS: Record<string, string> = {
  ArrowUp: "up",
  ArrowDown: "down",
  ArrowLeft: "left",
  ArrowRight: "right",
  Enter: "confirm",
  Escape: "back",
  Backspace: "back",
  p: "power",
  P: "power",
};

/** The button `key` presses on a device with the buttons `ids`, if any. */
export function buttonForKey(key: string, ids: readonly string[]): string | null {
  const id = KEYS[key];
  return id !== undefined && ids.includes(id) ? id : null;
}
