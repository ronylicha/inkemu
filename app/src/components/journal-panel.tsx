import { useI18n } from "../i18n/i18n";
import type { JournalEntry } from "../state/reducer";
import { labelOf } from "./labels";

/** What the run said: the lines the command line prints on its standard error. */
export function JournalPanel({ entries }: { entries: readonly JournalEntry[] }) {
  const { t } = useI18n();
  return (
    <section className="journal" aria-labelledby="journal-title">
      <h2 id="journal-title">{t("journal.title")}</h2>
      {entries.length === 0 ? (
        <p>{t("journal.empty")}</p>
      ) : (
        <ol role="log" aria-live="off" aria-labelledby="journal-title">
          {entries.map((entry) => (
            <li key={entry.id} className={`journal-${entry.level}`}>
              <span className="sr-only">{labelOf(t, "journal.level", entry.level)}: </span>
              {entry.text}
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
