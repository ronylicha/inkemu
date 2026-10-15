import { describe, expect, it } from "vitest";
import { buttonForKey } from "./keyboard";
import { keepTheme, keptTheme, resolveTheme } from "./theme";

const X4 = ["power", "up", "down", "back", "confirm", "left", "right"];

describe("buttonForKey", () => {
  it("maps the arrows, Enter, Escape, Backspace and P to the device's buttons", () => {
    expect(buttonForKey("ArrowUp", X4)).toBe("up");
    expect(buttonForKey("Enter", X4)).toBe("confirm");
    expect(buttonForKey("Escape", X4)).toBe("back");
    expect(buttonForKey("Backspace", X4)).toBe("back");
    expect(buttonForKey("P", X4)).toBe("power");
  });

  it("gives nothing for a key it does not map or a button the device lacks", () => {
    expect(buttonForKey("a", X4)).toBeNull();
    expect(buttonForKey("ArrowUp", ["power"])).toBeNull();
  });
});

describe("theme", () => {
  it("follows the system unless the user chose", () => {
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
    expect(resolveTheme("light", true)).toBe("light");
  });

  it("keeps the choice", () => {
    const store = new Map<string, string>();
    const storage = {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => {
        store.set(key, value);
      },
    };
    expect(keptTheme(storage)).toBe("system");
    keepTheme(storage, "dark");
    expect(keptTheme(storage)).toBe("dark");
    expect(keptTheme(null)).toBe("system");
  });
});
