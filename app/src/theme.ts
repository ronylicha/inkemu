// Light or dark, following the system unless the user chose; the choice is kept.

export type ThemeChoice = "system" | "light" | "dark";
export type Theme = "light" | "dark";
export const THEME_CHOICES: readonly ThemeChoice[] = ["system", "light", "dark"];

const STORAGE_KEY = "inkemu.theme";

export function resolveTheme(choice: ThemeChoice, systemPrefersDark: boolean): Theme {
  if (choice === "system") {
    return systemPrefersDark ? "dark" : "light";
  }
  return choice;
}

export function keptTheme(storage: Pick<Storage, "getItem"> | null): ThemeChoice {
  const kept = storage?.getItem(STORAGE_KEY);
  return kept === "light" || kept === "dark" || kept === "system" ? kept : "system";
}

export function keepTheme(storage: Pick<Storage, "setItem"> | null, choice: ThemeChoice): void {
  storage?.setItem(STORAGE_KEY, choice);
}
