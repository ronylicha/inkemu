import { useEffect, useState } from "react";
import { keepTheme, keptTheme, resolveTheme, type ThemeChoice } from "./theme";

function storage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

const DARK_QUERY = "(prefers-color-scheme: dark)";

function systemPrefersDark(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia(DARK_QUERY).matches;
}

/** The theme the user chose (kept), and the one in effect on the document. */
export function useTheme(): [ThemeChoice, (choice: ThemeChoice) => void] {
  const [choice, setChoice] = useState<ThemeChoice>(() => keptTheme(storage()));
  const [dark, setDark] = useState(systemPrefersDark);
  useEffect(() => {
    if (typeof window.matchMedia !== "function") {
      return;
    }
    const query = window.matchMedia(DARK_QUERY);
    const follow = (event: MediaQueryListEvent) => setDark(event.matches);
    query.addEventListener("change", follow);
    return () => query.removeEventListener("change", follow);
  }, []);
  useEffect(() => {
    document.documentElement.dataset.theme = resolveTheme(choice, dark);
  }, [choice, dark]);
  return [
    choice,
    (next) => {
      keepTheme(storage(), next);
      setChoice(next);
    },
  ];
}
