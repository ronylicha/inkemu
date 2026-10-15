// French and English, the language of the system first (spec §6.5), the user's choice kept.

import { createContext, useContext } from "react";
import type { fr } from "./fr";

export type Key = keyof typeof fr;
export type Catalog = Record<Key, string>;
export type Language = "fr" | "en";
export const LANGUAGES: readonly Language[] = ["fr", "en"];

const STORAGE_KEY = "inkemu.language";

/** The first of the system's languages inkemu speaks; English otherwise. */
export function detectLanguage(preferred: readonly string[]): Language {
  for (const tag of preferred) {
    const primary = tag.toLowerCase().split("-")[0];
    if (primary === "fr" || primary === "en") {
      return primary;
    }
  }
  return "en";
}

/** The language kept from an earlier choice, else the system's. */
export function initialLanguage(
  storage: Pick<Storage, "getItem"> | null,
  preferred: readonly string[],
): Language {
  const kept = storage?.getItem(STORAGE_KEY);
  return kept === "fr" || kept === "en" ? kept : detectLanguage(preferred);
}

export function keepLanguage(storage: Pick<Storage, "setItem"> | null, language: Language): void {
  storage?.setItem(STORAGE_KEY, language);
}

/** `text` with each `{name}` replaced by `params[name]`; an unknown name stays as it is. */
export function interpolate(text: string, params: Record<string, string | number> = {}): string {
  return text.replace(/\{(\w+)\}/g, (whole, name: string) =>
    name in params ? String(params[name]) : whole,
  );
}

export type Translate = (key: Key, params?: Record<string, string | number>) => string;

export function translator(catalog: Catalog): Translate {
  return (key, params) => interpolate(catalog[key], params);
}

export interface I18n {
  language: Language;
  setLanguage(language: Language): void;
  t: Translate;
}

export const I18nContext = createContext<I18n | null>(null);

export function useI18n(): I18n {
  const i18n = useContext(I18nContext);
  if (i18n === null) {
    throw new Error("useI18n outside of an I18nContext");
  }
  return i18n;
}
