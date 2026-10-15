// The resource monitor (design SP4b): what the host measured of the machine, read every second
// while the device runs and once each time it stops moving. Closed, the panel is not mounted and
// asks nothing. Every share, window, name and current comes from the host (4b-R7): the panel
// only formats numbers in the language of the interface.

import { useEffect, useState } from "react";
import { type Key, type Language, type Translate, useI18n } from "../i18n/i18n";
import { asHostError, useHost } from "../ipc/host";
import type {
  MemoryUseView,
  MonitorView,
  PageState,
  PowerRow,
  RecentView,
  Status,
  TimeView,
} from "../ipc/types";
import type { Action } from "../state/reducer";
import { frequency, hex32, labelOf } from "./labels";
import { charge, count, current, duration, kibibytes, percent, speedRatio } from "./monitor-format";

/** How often the panel reads the machine while it runs. */
export const MONITOR_EVERY_MS = 1000;

interface Props {
  /** The number of the start whose emulator is shown. */
  start: number;
  status: Status | null;
  /** The reducer's: a refusal of the host (stable, as `useReducer` gives it). */
  dispatch(action: Action): void;
}

export function MonitorPanel({ start, status, dispatch }: Props) {
  const { t } = useI18n();
  const host = useHost();
  const [view, setView] = useState<MonitorView | null>(null);
  const moving = status?.run === "running";
  // What the readings follow of the status: nothing before a first life; the life while the run
  // moves (read on a timer then); where the machine is when it does not move.
  const where =
    status === null || status.life === 0
      ? ""
      : moving
        ? `${status.life}:moving`
        : `${status.life}:${status.run}:${status.machine?.instructions ?? ""}:${status.machine?.deviceTimeNanos ?? ""}`;

  useEffect(() => {
    if (where === "") {
      return;
    }
    // A reading asked before this effect, or for a start this panel no longer shows, is dropped.
    let wanted = true;
    const read = () => {
      host
        .monitor(start)
        .then((next) => {
          if (wanted) {
            setView(next);
          }
        })
        .catch((reason: unknown) => {
          if (wanted) {
            dispatch({ type: "failed", error: asHostError(reason) });
          }
        });
    };
    read();
    const timer = moving ? window.setInterval(read, MONITOR_EVERY_MS) : null;
    return () => {
      wanted = false;
      if (timer !== null) {
        window.clearInterval(timer);
      }
    };
  }, [host, start, where, moving, dispatch]);

  if (status === null || status.life === 0) {
    return (
      <section id="monitor" className="monitor" aria-labelledby="monitor-title">
        <h2 id="monitor-title">{t("monitor.title")}</h2>
        <p className="note">{t("monitor.none")}</p>
      </section>
    );
  }

  return (
    <section id="monitor" className="monitor" aria-labelledby="monitor-title">
      <h2 id="monitor-title">{t("monitor.title")}</h2>
      <p className="note">
        {t("monitor.measured")} {moving ? t("monitor.live") : null}
      </p>
      {view === null ? null : (
        <div className="monitor-grid">
          <Cpu view={view} />
          <Time view={view} />
          <Memories memories={view.memories} />
          <Activity view={view} />
          <Power view={view} />
        </div>
      )}
    </section>
  );
}

function Cpu({ view }: { view: MonitorView }) {
  const { t, language } = useI18n();
  return (
    <section aria-labelledby="monitor-cpu">
      <h3 id="monitor-cpu">{t("monitor.cpu")}</h3>
      <dl className="monitor-facts">
        <dt>{t("monitor.frequency")}</dt>
        <dd>{frequency(view.cpuHz)}</dd>
        <dt>{t("monitor.deviceTime")}</dt>
        <dd>{duration(view.deviceTimeNanos, language)}</dd>
        <dt>{t("monitor.cycles")}</dt>
        <dd>{count(view.cycles, language)}</dd>
        <dt>{t("monitor.instructions")}</dt>
        <dd>{count(view.instructions, language)}</dd>
        <dt>{t("monitor.speedRatio")}</dt>
        <dd>
          {view.speedRatio === null
            ? t("monitor.speedRatio.none")
            : speedRatio(view.speedRatio, language)}
        </dd>
        <dt>{t("monitor.resets")}</dt>
        <dd>{count(view.resets, language)}</dd>
      </dl>
    </section>
  );
}

function load(time: TimeView, t: Translate, language: Language): string {
  return time.loadPermille === null ? t("monitor.load.none") : percent(time.loadPermille, language);
}

function Time({ view }: { view: MonitorView }) {
  const { t, language } = useI18n();
  const recent: RecentView | null = view.recent;
  const rows: [Key, keyof Omit<TimeView, "loadPermille">][] = [
    ["monitor.time.running", "runningNanos"],
    ["monitor.time.waiting", "waitingNanos"],
    ["monitor.time.deepSleep", "deepSleepNanos"],
  ];
  const noWindow = t("monitor.noWindow");
  return (
    <section aria-labelledby="monitor-time">
      <h3 id="monitor-time">{t("monitor.time")}</h3>
      <table className="monitor-table">
        <caption className="sr-only">{t("monitor.time")}</caption>
        <thead>
          <tr>
            <th scope="col">{t("monitor.time.state")}</th>
            <th scope="col">{t("monitor.time.total")}</th>
            <th scope="col">
              {recent === null
                ? t("monitor.recent")
                : t("monitor.time.recent", { duration: duration(recent.deviceNanos, language) })}
            </th>
          </tr>
        </thead>
        <tbody>
          {rows.map(([label, field]) => (
            <tr key={field}>
              <th scope="row">{t(label)}</th>
              <td>{duration(view.time[field], language)}</td>
              <td>{recent === null ? noWindow : duration(recent.time[field], language)}</td>
            </tr>
          ))}
          <tr>
            <th scope="row">{t("monitor.load")}</th>
            <td>{load(view.time, t, language)}</td>
            <td>{recent === null ? noWindow : load(recent.time, t, language)}</td>
          </tr>
        </tbody>
      </table>
      <table className="monitor-table">
        <caption>{t("monitor.clocks")}</caption>
        <thead>
          <tr>
            <th scope="col">{t("monitor.frequency")}</th>
            <th scope="col">{t("monitor.time.running")}</th>
            <th scope="col">{t("monitor.time.waiting")}</th>
          </tr>
        </thead>
        <tbody>
          {view.clocks.map((clock) => (
            <tr key={clock.hz}>
              <th scope="row">{frequency(clock.hz)}</th>
              <td>{duration(clock.runningNanos, language)}</td>
              <td>{duration(clock.waitingNanos, language)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

const PAGE_STATES: readonly PageState[] = ["never", "before", "since"];

function Memories({ memories }: { memories: MemoryUseView[] }) {
  const { t } = useI18n();
  return (
    <section aria-labelledby="monitor-memory">
      <h3 id="monitor-memory">{t("monitor.memory")}</h3>
      {memories.map((memory) => (
        <Memory key={memory.region} memory={memory} />
      ))}
      <ul className="monitor-legend" aria-label={t("monitor.memory")}>
        {PAGE_STATES.map((state) => (
          <li key={state}>
            <span className={`monitor-page monitor-page-${state}`} aria-hidden="true" />{" "}
            {labelOf(t, "monitor.memory.legend", state)}
          </li>
        ))}
      </ul>
      <p className="note">{t("monitor.memory.note")}</p>
    </section>
  );
}

function Memory({ memory }: { memory: MemoryUseView }) {
  const { t, language } = useI18n();
  const name = labelOf(t, "monitor.memory.region", memory.region);
  const kib = t("monitor.kib");
  const summary = t("monitor.memory.summary", {
    name,
    written: count(memory.written, language),
    pages: count(memory.pages.length, language),
    since: count(memory.writtenSinceReset, language),
  });
  return (
    <div className="monitor-memory">
      <h4>
        {name}{" "}
        <span className="note">
          {t("monitor.memory.size", {
            size: kibibytes(memory.size, kib, language),
            page: kibibytes(memory.pageBytes, kib, language),
          })}
        </span>
      </h4>
      <div className="monitor-pages" role="img" aria-label={summary}>
        {memory.pages.map((state, page) => (
          // The pages of a RAM are its, in order, and never move: their place is their identity.
          // biome-ignore lint/suspicious/noArrayIndexKey: a page is its index
          <span key={page} className={`monitor-page monitor-page-${state}`} />
        ))}
      </div>
      <dl className="monitor-facts">
        <dt>{t("monitor.memory.written")}</dt>
        <dd>
          {count(memory.written, language)} / {count(memory.pages.length, language)}
        </dd>
        <dt>{t("monitor.memory.sinceReset")}</dt>
        <dd>{count(memory.writtenSinceReset, language)}</dd>
        <dt>{t("monitor.memory.range")}</dt>
        <dd className="mono">
          {memory.lowest === null || memory.highestEnd === null
            ? t("monitor.memory.range.none")
            : t("monitor.memory.range.value", {
                low: hex32(memory.lowest),
                high: hex32(memory.highestEnd),
              })}
        </dd>
        <dt>{t("monitor.memory.writes")}</dt>
        <dd>
          {t("monitor.memory.writes.value", {
            all: count(memory.writes, language),
            since: count(memory.writesSinceReset, language),
          })}
        </dd>
      </dl>
    </div>
  );
}

function Activity({ view }: { view: MonitorView }) {
  const { t, language } = useI18n();
  const windowed = (value: number | null) =>
    value === null ? t("monitor.noWindow") : count(value, language);
  return (
    <section aria-labelledby="monitor-activity">
      <h3 id="monitor-activity">{t("monitor.activity")}</h3>
      {view.interrupts.length === 0 ? (
        <p className="note">{t("monitor.interrupts.none")}</p>
      ) : (
        <table className="monitor-table">
          <caption>{t("monitor.interrupts")}</caption>
          <thead>
            <tr>
              <th scope="col">{t("monitor.interrupts.source")}</th>
              <th scope="col">{t("monitor.interrupts.name")}</th>
              <th scope="col">{t("monitor.interrupts.raised")}</th>
              <th scope="col">{t("monitor.recent")}</th>
            </tr>
          </thead>
          <tbody>
            {view.interrupts.map((source) => (
              <tr key={source.source}>
                <th scope="row">{source.source}</th>
                <td className="mono">{source.name ?? ""}</td>
                <td>{count(source.raised, language)}</td>
                <td>{windowed(source.recent)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {view.lines.length === 0 ? null : (
        <table className="monitor-table">
          <caption>{t("monitor.lines")}</caption>
          <thead>
            <tr>
              <th scope="col">{t("monitor.lines.line")}</th>
              <th scope="col">{t("monitor.lines.taken")}</th>
            </tr>
          </thead>
          <tbody>
            {view.lines.map((line) => (
              <tr key={line.line}>
                <th scope="row">{line.line}</th>
                <td>{count(line.taken, language)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {view.peripherals.length === 0 ? (
        <p className="note">{t("monitor.peripherals.none")}</p>
      ) : (
        <table className="monitor-table">
          <caption>{t("monitor.peripherals")}</caption>
          <thead>
            <tr>
              <th scope="col">{t("monitor.peripherals.address")}</th>
              <th scope="col">{t("monitor.peripherals.block")}</th>
              <th scope="col">{t("monitor.peripherals.reads")}</th>
              <th scope="col">{t("monitor.peripherals.writes")}</th>
              <th scope="col">{t("monitor.recent")}</th>
            </tr>
          </thead>
          <tbody>
            {view.peripherals.map((block) => (
              <tr key={block.base}>
                <th scope="row" className="mono">
                  {hex32(block.base)}
                </th>
                <td className="mono">
                  {block.peripherals.length === 0
                    ? t("monitor.peripherals.unnamed")
                    : block.peripherals.join(", ")}
                </td>
                <td>{count(block.reads, language)}</td>
                <td>{count(block.writes, language)}</td>
                <td>{windowed(block.recent)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <h4>{t("monitor.panel")}</h4>
      <p>
        {view.panel === null
          ? t("monitor.panel.none")
          : t(view.panel.refreshes === 1 ? "monitor.panel.one" : "monitor.panel.other", {
              count: count(view.panel.refreshes, language),
              kind: labelOf(t, "monitor.panel", view.panel.last),
            })}
      </p>
    </section>
  );
}

function powerState(row: PowerRow, t: Translate): string {
  return labelOf(t, "monitor.power.state", row.state, {
    frequency: row.hz === null ? "" : frequency(row.hz),
  });
}

function Power({ view }: { view: MonitorView }) {
  const { t, language } = useI18n();
  const { power } = view;
  return (
    <section aria-labelledby="monitor-power">
      <h3 id="monitor-power">{t("monitor.power")}</h3>
      <p className="note">{t("monitor.power.note", { source: power.source })}</p>
      <table className="monitor-table">
        <caption className="sr-only">{t("monitor.power")}</caption>
        <thead>
          <tr>
            <th scope="col">{t("monitor.power.state")}</th>
            <th scope="col">{t("monitor.power.duration")}</th>
            <th scope="col">{t("monitor.power.current")}</th>
            <th scope="col">{t("monitor.power.charge")}</th>
          </tr>
        </thead>
        <tbody>
          {power.rows.map((row) => (
            <tr key={`${row.state}:${row.hz ?? ""}`}>
              <th scope="row">{powerState(row, t)}</th>
              <td>{duration(row.nanos, language)}</td>
              <td>
                {row.microamps === null
                  ? t("monitor.power.noFigure")
                  : range(row.microamps, (value) => current(value, language), t)}
              </td>
              <td>
                {row.microampHours === null
                  ? t("monitor.power.noFigure")
                  : range(row.microampHours, (value) => charge(value, language), t)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <p>
        {t("monitor.power.total", {
          charge: range(power.microampHours, (value) => charge(value, language), t),
        })}
      </p>
      {power.unpricedNanos === 0 ? null : (
        <p className="note">
          {t("monitor.power.unpriced", { duration: duration(power.unpricedNanos, language) })}
        </p>
      )}
    </section>
  );
}

/** `low to high`, or the one value when both are the same. */
function range([low, high]: [number, number], write: (value: number) => string, t: Translate) {
  return low === high
    ? write(low)
    : t("monitor.power.range", { low: write(low), high: write(high) });
}
