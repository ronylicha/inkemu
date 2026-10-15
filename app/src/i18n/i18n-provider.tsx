import { type ReactNode, useEffect, useState } from "react";
import { en } from "./en";
import { fr } from "./fr";
import {
  type Catalog,
  I18nContext,
  initialLanguage,
  keepLanguage,
  type Language,
  translator,
} from "./i18n";

const CATALOGS: Record<Language, Catalog> = { fr, en };

function storage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

export function I18nProvider({
  children,
  language: forced,
}: {
  children: ReactNode;
  /** A language for tests; the system's or the kept one otherwise. */
  language?: Language;
}) {
  const [language, setLanguage] = useState<Language>(
    () => forced ?? initialLanguage(storage(), navigator.languages),
  );
  useEffect(() => {
    document.documentElement.lang = language;
  }, [language]);
  const value = {
    language,
    setLanguage: (next: Language) => {
      keepLanguage(storage(), next);
      setLanguage(next);
    },
    t: translator(CATALOGS[language]),
  };
  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}
