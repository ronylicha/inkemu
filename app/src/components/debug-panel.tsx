// The debugger panel (design SP4a): what the host shows of the machine, read again whenever the
// run does not move (a pause, a stop at a point, a step, a halt, an end) and at the user's
// request while it moves; and the forms that set the points. The host decides what can be read
// and refuses what cannot be set (plan ruling 4a-R8).

import { type FormEvent, useEffect, useRef, useState } from "react";
import { useI18n } from "../i18n/i18n";
import { asHostError, useHost } from "../ipc/host";
import {
  type Command,
  type DebugView,
  type Status,
  WATCH_ONS,
  type WatchOn,
  type Watchpoint,
} from "../ipc/types";
import type { Action } from "../state/reducer";
import { parseAddress, parseLength } from "./debug-format";
import {
  BreakpointList,
  CpuSummary,
  CsrTable,
  Disassembly,
  MemoryTable,
  RegisterTable,
  stopText,
  WatchpointList,
} from "./debug-views";
import { labelOf } from "./labels";

/** The first word of the SRAM: where the memory view starts until the user asks elsewhere. */
const SRAM_START = 0x3fc8_0000;
/** The bytes of a memory view and the lines of a disassembly the panel asks for. */
const MEMORY_BYTES = 256;
const CODE_LINES = 24;

interface Props {
  /** The number of the start whose emulator is shown. */
  start: number;
  status: Status | null;
  /** Gives a command to that emulator; resolves once it is done. */
  onCommand(command: Command): Promise<void>;
  /** The reducer's: the points shown, a refusal (stable, as `useReducer` gives it). */
  dispatch(action: Action): void;
}

/** A field whose text must be an address: its error, when the last submit could not read it. */
function useAddressField(initial = "") {
  const [text, setText] = useState(initial);
  const [invalid, setInvalid] = useState(false);
  return { text, setText, invalid, setInvalid };
}

export function DebugPanel({ start, status, onCommand, dispatch }: Props) {
  const { t } = useI18n();
  const host = useHost();
  const [view, setView] = useState<DebugView | null>(null);
  /** Where the code is shown from; `null` follows the PC. */
  const [codeAt, setCodeAt] = useState<number | null>(null);
  const [memoryAt, setMemoryAt] = useState(SRAM_START);
  /** Asks to read again, whatever the run does: a refresh, a point set or cleared. */
  const [asks, setAsks] = useState(0);
  const seenAsks = useRef(0);
  const latest = useRef(0);
  const code = useAddressField();
  const memory = useAddressField(`0x${SRAM_START.toString(16)}`);
  const breakpoint = useAddressField();
  const watch = useAddressField();
  const [watchLength, setWatchLength] = useState("4");
  const [lengthInvalid, setLengthInvalid] = useState(false);
  const [watchOn, setWatchOn] = useState<WatchOn>("write");

  const moving = status?.run === "running";
  // Where the machine shown is, as the status says it, empty with none: the view is read again
  // when it changes while the run does not move (a new life, a stop, a step, an end).
  const where =
    status === null
      ? ""
      : `${status.life}:${status.run}:${status.machine?.instructions ?? ""}:${status.machine?.pc ?? ""}`;

  useEffect(() => {
    if (where === "") {
      return;
    }
    // While the run moves, the view is read at the user's request only.
    if (moving && asks === seenAsks.current) {
      return;
    }
    seenAsks.current = asks;
    latest.current += 1;
    const asked = latest.current;
    host
      .inspect(
        {
          disassemblyAt: codeAt,
          disassemblyLines: CODE_LINES,
          memory: { address: memoryAt, length: MEMORY_BYTES },
        },
        start,
      )
      .then((next) => {
        if (asked === latest.current) {
          setView(next);
          dispatch({ type: "points", start, points: next.points });
        }
      })
      .catch((reason: unknown) => {
        if (asked === latest.current) {
          dispatch({ type: "failed", error: asHostError(reason) });
        }
      });
  }, [host, start, moving, where, codeAt, memoryAt, asks, dispatch]);

  const send = (command: Command) => {
    onCommand(command)
      .then(() => setAsks((n) => n + 1))
      .catch((reason: unknown) => dispatch({ type: "failed", error: asHostError(reason) }));
  };
  const setBreakpoint = (address: number, set: boolean) =>
    send({ kind: "breakpoint", address, set });
  const setWatchpoint = (point: Watchpoint, set: boolean) =>
    send({ kind: "watchpoint", ...point, set });

  /** Reads `field` as an address and hands it to `use`; says so when it cannot. */
  const submitAddress =
    (field: ReturnType<typeof useAddressField>, use: (address: number) => void) =>
    (event: FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const address = parseAddress(field.text);
      field.setInvalid(address === null);
      if (address !== null) {
        use(address);
      }
    };

  const submitWatch = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const address = parseAddress(watch.text);
    const length = parseLength(watchLength);
    watch.setInvalid(address === null);
    setLengthInvalid(length === null);
    if (address !== null && length !== null) {
      setWatchpoint({ address, length, on: watchOn }, true);
    }
  };

  if (status === null) {
    return (
      <section id="debugger" className="debugger" aria-labelledby="debugger-title">
        <h2 id="debugger-title">{t("debug.title")}</h2>
        <p className="note">{t("debug.none")}</p>
      </section>
    );
  }

  const invalidAddress = (id: string, invalid: boolean) =>
    invalid ? (
      <p id={id} className="warning">
        {t("debug.invalidAddress")}
      </p>
    ) : null;

  return (
    <section id="debugger" className="debugger" aria-labelledby="debugger-title">
      <div className="debugger-head">
        <h2 id="debugger-title">{t("debug.title")}</h2>
        <button type="button" onClick={() => setAsks((n) => n + 1)}>
          {t("debug.refresh")}
        </button>
      </div>
      {status.stop === null ? null : (
        // Seen here; heard through the window's announcement of the run, which says it too.
        <p className="debugger-stop">{stopText(t, status.stop)}</p>
      )}
      {moving ? <p className="note">{t("debug.running")}</p> : null}
      {view === null ? null : (
        <div className="debugger-grid">
          <section aria-labelledby="debugger-cpu">
            <h3 id="debugger-cpu">{t("debug.cpu")}</h3>
            <CpuSummary cpu={view.cpu} />
            <RegisterTable cpu={view.cpu} />
            <CsrTable cpu={view.cpu} />
          </section>
          <section aria-labelledby="debugger-code">
            <h3 id="debugger-code">{t("debug.code")}</h3>
            <form
              className="debugger-form"
              onSubmit={submitAddress(code, (address) => setCodeAt(address))}
            >
              <label>
                {t("debug.code.goTo")}{" "}
                <input
                  type="text"
                  className="mono"
                  value={code.text}
                  onChange={(event) => code.setText(event.target.value)}
                  aria-invalid={code.invalid}
                  aria-describedby={code.invalid ? "debugger-code-error" : undefined}
                />
              </label>
              <button type="submit">{t("debug.code.show")}</button>
              <button type="button" onClick={() => setCodeAt(null)} disabled={codeAt === null}>
                {t("debug.code.followPc")}
              </button>
            </form>
            {invalidAddress("debugger-code-error", code.invalid)}
            <Disassembly lines={view.disassembly} onBreakpoint={setBreakpoint} />
          </section>
          <div className="debugger-stack">
            <section aria-labelledby="debugger-memory">
              <h3 id="debugger-memory">{t("debug.memory")}</h3>
              <form
                className="debugger-form"
                onSubmit={submitAddress(memory, (address) => setMemoryAt(address))}
              >
                <label>
                  {t("debug.memory.at")}{" "}
                  <input
                    type="text"
                    className="mono"
                    value={memory.text}
                    onChange={(event) => memory.setText(event.target.value)}
                    aria-invalid={memory.invalid}
                    aria-describedby={memory.invalid ? "debugger-memory-error" : undefined}
                  />
                </label>
                <button type="submit">{t("debug.memory.read")}</button>
              </form>
              {invalidAddress("debugger-memory-error", memory.invalid)}
              {view.memory === null ? null : <MemoryTable memory={view.memory} />}
            </section>
            <section aria-labelledby="debugger-breakpoints">
              <h3 id="debugger-breakpoints">{t("debug.breakpoints")}</h3>
              <form
                className="debugger-form"
                onSubmit={submitAddress(breakpoint, (address) => setBreakpoint(address, true))}
              >
                <label>
                  {t("debug.breakpoint.address")}{" "}
                  <input
                    type="text"
                    className="mono"
                    value={breakpoint.text}
                    onChange={(event) => breakpoint.setText(event.target.value)}
                    aria-invalid={breakpoint.invalid}
                    aria-describedby={breakpoint.invalid ? "debugger-breakpoint-error" : undefined}
                  />
                </label>
                <button type="submit">{t("debug.breakpoint.set")}</button>
              </form>
              {invalidAddress("debugger-breakpoint-error", breakpoint.invalid)}
              <BreakpointList points={view.points} onBreakpoint={setBreakpoint} />
            </section>
            <section aria-labelledby="debugger-watchpoints">
              <h3 id="debugger-watchpoints">{t("debug.watchpoints")}</h3>
              <form className="debugger-form" onSubmit={submitWatch}>
                <label>
                  {t("debug.watch.address")}{" "}
                  <input
                    type="text"
                    className="mono"
                    value={watch.text}
                    onChange={(event) => watch.setText(event.target.value)}
                    aria-invalid={watch.invalid}
                    aria-describedby={watch.invalid ? "debugger-watch-error" : undefined}
                  />
                </label>
                <label>
                  {t("debug.watch.length")}{" "}
                  <input
                    type="text"
                    inputMode="numeric"
                    size={5}
                    value={watchLength}
                    onChange={(event) => setWatchLength(event.target.value)}
                    aria-invalid={lengthInvalid}
                    aria-describedby={lengthInvalid ? "debugger-length-error" : undefined}
                  />
                </label>
                <label>
                  {t("debug.watch.on")}{" "}
                  <select
                    value={watchOn}
                    onChange={(event) => setWatchOn(event.target.value as WatchOn)}
                  >
                    {WATCH_ONS.map((on) => (
                      <option key={on} value={on}>
                        {labelOf(t, "debug.watch.on", on)}
                      </option>
                    ))}
                  </select>
                </label>
                <button type="submit">{t("debug.watch.set")}</button>
              </form>
              {invalidAddress("debugger-watch-error", watch.invalid)}
              {lengthInvalid ? (
                <p id="debugger-length-error" className="warning">
                  {t("debug.invalidLength")}
                </p>
              ) : null}
              <WatchpointList points={view.points} onWatchpoint={setWatchpoint} />
            </section>
          </div>
        </div>
      )}
    </section>
  );
}
