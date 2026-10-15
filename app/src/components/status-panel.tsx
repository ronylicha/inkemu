import { useI18n } from "../i18n/i18n";
import type { Status } from "../ipc/types";
import type { LifeEnd } from "../state/reducer";
import { frequency, hex32, labelOf, seconds } from "./labels";

interface Props {
  status: Status | null;
  lastEnd: LifeEnd | null;
}

/** Where the run is, as the host last said. */
export function StatusPanel({ status, lastEnd }: Props) {
  const { t, language } = useI18n();
  const number = new Intl.NumberFormat(language === "fr" ? "fr-FR" : "en-US");
  const ratio = new Intl.NumberFormat(language === "fr" ? "fr-FR" : "en-US", {
    maximumFractionDigits: 2,
  });
  if (status === null) {
    return (
      <section className="status" aria-labelledby="status-title">
        <h2 id="status-title">{t("status.title")}</h2>
        <p>{t("status.off")}</p>
      </section>
    );
  }
  const machine = status.machine;
  return (
    <section className="status" aria-labelledby="status-title">
      <h2 id="status-title">{t("status.title")}</h2>
      <dl>
        <dt>{t("status.state")}</dt>
        <dd>{labelOf(t, "status.run", status.run)}</dd>
        <dt>{t("status.life")}</dt>
        <dd>{status.life}</dd>
        {status.halt === null ? null : (
          <>
            <dt className="sr-only">{t("status.run.halted")}</dt>
            <dd className="note">{labelOf(t, "status.halt", status.halt)}</dd>
          </>
        )}
        {status.end === null ? null : (
          <>
            <dt>{t("status.end")}</dt>
            <dd>{labelOf(t, "status.end", status.end)}</dd>
          </>
        )}
        {machine === null ? null : (
          <>
            <dt>{t("status.deviceTime")}</dt>
            <dd>{seconds(machine.deviceTimeNanos)}</dd>
            <dt>{t("status.cycles")}</dt>
            <dd>{number.format(machine.cycles)}</dd>
            <dt>{t("status.instructions")}</dt>
            <dd>{number.format(machine.instructions)}</dd>
            <dt>{t("status.pc")}</dt>
            <dd className="mono">{hex32(machine.pc)}</dd>
            <dt>{t("status.cpuClock")}</dt>
            <dd>{frequency(machine.cpuHz)}</dd>
            <dt>{t("status.cpuAsleep")}</dt>
            <dd>{machine.cpuAsleep ? t("status.yes") : t("status.no")}</dd>
            {machine.deepSleep === null ? null : (
              <>
                <dt>{t("status.deepSleep")}</dt>
                <dd>{seconds(machine.deepSleep.sinceNanos)}</dd>
                {machine.deepSleep.timerWakeNanos === null ? null : (
                  <>
                    <dt>{t("status.timerWake")}</dt>
                    <dd>{seconds(machine.deepSleep.timerWakeNanos)}</dd>
                  </>
                )}
              </>
            )}
            <dt>{t("status.usbHost")}</dt>
            <dd>{labelOf(t, "usbHost", machine.usbHost)}</dd>
          </>
        )}
        <dt>{t("status.speed")}</dt>
        <dd>
          {labelOf(t, "toolbar.speed", status.speed)}
          {status.speedRatio === null
            ? null
            : ` — ${t("status.speedRatio", { ratio: ratio.format(status.speedRatio) })}`}
        </dd>
        {status.recording === null ? null : (
          <>
            <dt>{t("status.recording")}</dt>
            <dd>{status.recording}</dd>
          </>
        )}
        {status.replay === null ? null : (
          <>
            <dt>{t("status.replay")}</dt>
            <dd>{status.replay}</dd>
          </>
        )}
      </dl>
      {machine !== null && status.recording !== null && !machine.journalComplete ? (
        <p className="warning" role="alert">
          {t("status.journalFull")}
        </p>
      ) : null}
      {lastEnd === null ? null : (
        <p className="note">
          {t("life.ended", { life: lastEnd.life, end: labelOf(t, "status.end", lastEnd.end) })}
          {lastEnd.verdict === null ? null : (
            <>
              {" "}
              {lastEnd.verdict.kind === "identical"
                ? t("verdict.identical")
                : t(`verdict.${lastEnd.verdict.kind}`, { detail: lastEnd.verdict.detail })}
            </>
          )}
        </p>
      )}
    </section>
  );
}
