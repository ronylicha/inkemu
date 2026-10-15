import { LANGUAGES, type Language, useI18n } from "../i18n/i18n";
import type { Command, Speed, Status } from "../ipc/types";
import type { AppState } from "../state/reducer";
import { THEME_CHOICES, type ThemeChoice } from "../theme";
import { labelOf } from "./labels";

interface Props {
  phase: AppState["phase"];
  status: Status | null;
  speed: Speed;
  hasImage: boolean;
  theme: ThemeChoice;
  onPowerOn(): void;
  onCommand(command: Command): void;
  onSpeed(speed: Speed): void;
  onRecord(): void;
  onReplay(): void;
  onCapture(): void;
  onAbout(): void;
  onTheme(theme: ThemeChoice): void;
}

const LANGUAGE_NAMES: Record<Language, string> = { fr: "Français", en: "English" };

/**
 * The controls of the emulator. Which are enabled follows the status the host sent: the
 * interface decides nothing about the device. A labelled region, not an ARIA toolbar: it holds
 * selects, which take the arrow keys a toolbar would use to move between its controls; Tab moves.
 */
export function Toolbar(props: Props) {
  const { phase, status, speed, hasImage, theme } = props;
  const { t, language, setLanguage } = useI18n();
  const on = phase === "on";
  const run = status?.run ?? null;
  const live = on && (run === "running" || run === "paused");
  const replaying = status?.replay != null;
  return (
    <section aria-label={t("toolbar.label")} className="toolbar">
      <button type="button" onClick={props.onPowerOn} disabled={phase === "starting"}>
        {phase === "starting" ? t("start.starting") : t("toolbar.powerOn")}
      </button>
      {run === "running" ? (
        <button type="button" onClick={() => props.onCommand({ kind: "pause" })} disabled={!on}>
          {t("toolbar.pause")}
        </button>
      ) : (
        <button type="button" onClick={() => props.onCommand({ kind: "run" })} disabled={!live}>
          {t("toolbar.run")}
        </button>
      )}
      <button type="button" onClick={() => props.onCommand({ kind: "step" })} disabled={!live}>
        {t("toolbar.step")}
      </button>
      <button type="button" onClick={() => props.onCommand({ kind: "reset" })} disabled={!on}>
        {t("toolbar.reset")}
      </button>
      <button type="button" onClick={() => props.onCommand({ kind: "stop" })} disabled={!on}>
        {status?.recording != null ? t("toolbar.stopAndSave") : t("toolbar.stop")}
      </button>
      <label>
        {t("toolbar.speed")}{" "}
        <select value={speed} onChange={(event) => props.onSpeed(event.target.value as Speed)}>
          <option value="realTime">{t("toolbar.speed.realTime")}</option>
          <option value="fastest">{t("toolbar.speed.fastest")}</option>
        </select>
      </label>
      <button
        type="button"
        onClick={props.onRecord}
        disabled={!on || replaying}
        title={t("toolbar.recordHint")}
      >
        {t("toolbar.record")}
      </button>
      <button type="button" onClick={props.onReplay} disabled={phase === "starting"}>
        {t("toolbar.replay")}
      </button>
      <button type="button" onClick={props.onCapture} disabled={!hasImage}>
        {t("toolbar.capture")}
      </button>
      <span className="toolbar-spacer" />
      <label>
        {t("toolbar.theme")}{" "}
        <select
          value={theme}
          onChange={(event) => props.onTheme(event.target.value as ThemeChoice)}
        >
          {THEME_CHOICES.map((choice) => (
            <option key={choice} value={choice}>
              {labelOf(t, "toolbar.theme", choice)}
            </option>
          ))}
        </select>
      </label>
      <label>
        {t("toolbar.language")}{" "}
        <select value={language} onChange={(event) => setLanguage(event.target.value as Language)}>
          {LANGUAGES.map((one) => (
            <option key={one} value={one} lang={one}>
              {LANGUAGE_NAMES[one]}
            </option>
          ))}
        </select>
      </label>
      <button type="button" onClick={props.onAbout}>
        {t("toolbar.about")}
      </button>
    </section>
  );
}
