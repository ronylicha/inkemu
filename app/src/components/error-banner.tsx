import { useI18n } from "../i18n/i18n";
import type { HostError } from "../ipc/types";
import { labelOf } from "./labels";

/**
 * A refusal: the interface's own words for what was refused, chosen by its kind, and the host's
 * text, which states the cause and the remedy, as the detail.
 */
export function ErrorBanner({ error, onDismiss }: { error: HostError; onDismiss(): void }) {
  const { t } = useI18n();
  const title = labelOf(t, "error", error.kind);
  return (
    <div className="error" role="alert">
      <strong>{t("error.title")}</strong> {title === error.kind ? error.message : title}
      {title === error.kind ? null : <p className="error-detail">{error.message}</p>}
      <button type="button" onClick={onDismiss}>
        {t("error.dismiss")}
      </button>
    </div>
  );
}
