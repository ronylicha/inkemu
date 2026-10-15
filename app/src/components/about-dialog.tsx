import { useEffect, useState } from "react";
import { useI18n } from "../i18n/i18n";
import { useHost } from "../ipc/host";
import { Modal } from "./modal";

/** What inkemu is, and the notices of what it embeds (`inkemu licenses`). */
export function AboutDialog({ onClose }: { onClose(): void }) {
  const { t } = useI18n();
  const host = useHost();
  const [notices, setNotices] = useState("");
  useEffect(() => {
    let current = true;
    host
      .notices()
      .then((text) => {
        if (current) {
          setNotices(text);
        }
      })
      .catch(() => {
        // The dialog still says what inkemu is.
      });
    return () => {
      current = false;
    };
  }, [host]);
  return (
    <Modal title={t("about.title")} onClose={onClose}>
      <p>{t("about.intro")}</p>
      <h3>{t("about.notices")}</h3>
      <pre
        className="notices"
        // biome-ignore lint/a11y/noNoninteractiveTabindex: a scrollable region must take the focus to scroll by keyboard (WCAG 2.1.1).
        tabIndex={0}
      >
        {notices}
      </pre>
      <div className="actions">
        <button type="button" onClick={onClose}>
          {t("about.close")}
        </button>
      </div>
    </Modal>
  );
}
