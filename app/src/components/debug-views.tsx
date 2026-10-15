// The parts of the debugger panel that show what the host sent: the CPU, its registers and
// CSRs, the code, the memory and the points. They decide nothing; the panel holds the forms.

import type { Translate } from "../i18n/i18n";
import { useI18n } from "../i18n/i18n";
import type {
  CpuView,
  DisassemblyLine,
  MemoryView,
  Points,
  StopView,
  Watchpoint,
} from "../ipc/types";
import { hexByte, hiddenRegions, memoryRows, textOf } from "./debug-format";
import { hex32, labelOf } from "./labels";

/** `count` bytes in words: singular for one, plural above; never 0 here (an access is 1 to 4
 * bytes, a watchpoint 1 to 4096). */
export function bytesText(t: Translate, count: number): string {
  return t(count === 1 ? "debug.bytes.one" : "debug.bytes.other", { count });
}

/** The sentence that says why the debugger stopped the run. */
export function stopText(t: Translate, stop: StopView): string {
  if (stop.kind === "breakpoint") {
    return t("debug.stop.breakpoint", { pc: hex32(stop.pc) });
  }
  return t(stop.access === "read" ? "debug.stop.watchpoint.read" : "debug.stop.watchpoint.write", {
    pc: hex32(stop.pc),
    bytes: bytesText(t, stop.size),
    address: hex32(stop.address),
    value: `0x${stop.value.toString(16)}`,
  });
}

/** A watchpoint in words, for its line and its remove button. */
export function watchText(t: Translate, watch: Watchpoint): string {
  return t("debug.watch.item", {
    bytes: bytesText(t, watch.length),
    address: hex32(watch.address),
    on: labelOf(t, "debug.watch.on", watch.on),
  });
}

export function CpuSummary({ cpu }: { cpu: CpuView }) {
  const { t } = useI18n();
  return (
    <dl className="debug-cpu">
      <dt>{t("debug.pc")}</dt>
      <dd className="mono">{hex32(cpu.pc)}</dd>
      <dt>{t("debug.privilege")}</dt>
      <dd>{labelOf(t, "debug.privilege", cpu.privilege)}</dd>
      {cpu.asleep ? (
        <>
          <dt className="sr-only">{t("debug.cpu")}</dt>
          <dd className="note">{t("debug.asleep")}</dd>
        </>
      ) : null}
    </dl>
  );
}

export function RegisterTable({ cpu }: { cpu: CpuView }) {
  const { t } = useI18n();
  return (
    <table className="debug-table">
      <caption>{t("debug.registers")}</caption>
      <thead>
        <tr>
          <th scope="col">{t("debug.register")}</th>
          <th scope="col">{t("debug.value")}</th>
        </tr>
      </thead>
      <tbody>
        {cpu.registers.map((register) => (
          <tr key={register.number}>
            <th scope="row" className="mono">
              x{register.number} {register.name}
            </th>
            <td className="mono">{hex32(register.value)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function CsrTable({ cpu }: { cpu: CpuView }) {
  const { t } = useI18n();
  return (
    <table className="debug-table">
      <caption>{t("debug.csrs")}</caption>
      <thead>
        <tr>
          <th scope="col">{t("debug.csr")}</th>
          <th scope="col">{t("debug.value")}</th>
        </tr>
      </thead>
      <tbody>
        {cpu.csrs.map((csr) => (
          <tr key={csr.address}>
            <th scope="row" className="mono">
              {csr.name}
            </th>
            <td className="mono">{hex32(csr.value)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

interface DisassemblyProps {
  lines: DisassemblyLine[];
  /** Sets (`set`) or clears the breakpoint at `address`. */
  onBreakpoint(address: number, set: boolean): void;
}

/** The code: one line per instruction, the PC's marked, each with its breakpoint button. */
export function Disassembly({ lines, onBreakpoint }: DisassemblyProps) {
  const { t } = useI18n();
  return (
    <ol className="disassembly" aria-label={t("debug.code")}>
      {lines.map((line) => (
        <li
          key={line.address}
          aria-current={line.current ? "true" : undefined}
          className={line.current ? "disassembly-current" : undefined}
        >
          <button
            type="button"
            className="breakpoint-toggle"
            aria-pressed={line.breakpoint}
            aria-label={t("debug.breakpoint.toggle", { address: hex32(line.address) })}
            // A page of the flash not executed yet may hold code that will run: a breakpoint
            // may wait there.
            disabled={line.instruction === null && !line.notExecuted && !line.breakpoint}
            onClick={() => onBreakpoint(line.address, !line.breakpoint)}
          >
            <span aria-hidden="true">{line.breakpoint ? "●" : "○"}</span>
          </button>
          <span className="mono disassembly-address">{hex32(line.address)}</span>
          {line.instruction === null ? (
            <span className="note">
              {t(line.notExecuted ? "debug.code.notExecuted" : "debug.code.unreadable")}
            </span>
          ) : (
            <span className="mono">
              {line.instruction.text}
              {line.instruction.compressed ? (
                <span className="note" title={t("debug.code.compressed")}>
                  {" "}
                  <span aria-hidden="true">(c)</span>
                  <span className="sr-only">{t("debug.code.compressed")}</span>
                </span>
              ) : null}
            </span>
          )}
          {line.current ? <span className="sr-only">{t("debug.code.current")}</span> : null}
        </li>
      ))}
    </ol>
  );
}

const COLUMNS = Array.from({ length: 16 }, (_, i) => i);

/** The memory: 16 bytes to a row with their text, and what is not shown, with why. */
export function MemoryTable({ memory }: { memory: MemoryView }) {
  const { t } = useI18n();
  const hidden = hiddenRegions(memory);
  return (
    <>
      <table className="debug-table memory-table">
        <caption className="sr-only">{t("debug.memory")}</caption>
        <thead>
          <tr>
            <th scope="col">{t("debug.address")}</th>
            {COLUMNS.map((column) => (
              <th scope="col" key={column} className="mono">
                {column.toString(16)}
              </th>
            ))}
            <th scope="col">{t("debug.memory.text")}</th>
          </tr>
        </thead>
        <tbody>
          {memoryRows(memory).map((row) => (
            <tr key={row.address}>
              <th scope="row" className="mono">
                {hex32(row.address)}
              </th>
              {COLUMNS.map((column) => (
                <td key={column} className="mono">
                  {column < row.bytes.length ? hexByte(row.bytes[column] ?? null) : ""}
                </td>
              ))}
              <td className="mono memory-text">{textOf(row.bytes)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {hidden.length === 0 ? null : (
        <ul className="note memory-hidden">
          {hidden.map((span) => (
            <li key={span.start}>
              {t("debug.memory.hidden", {
                from: hex32(span.start),
                to: hex32((span.start + span.length - 1) >>> 0),
                why: labelOf(t, "debug.region", span.region),
              })}
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

interface BreakpointListProps {
  points: Points;
  onBreakpoint(address: number, set: boolean): void;
}

/** The breakpoints set, each with its remove button. */
export function BreakpointList({ points, onBreakpoint }: BreakpointListProps) {
  const { t } = useI18n();
  if (points.breakpoints.length === 0) {
    return <p className="note">{t("debug.breakpoints.none")}</p>;
  }
  return (
    <ul className="point-list">
      {points.breakpoints.map((address) => (
        <li key={address}>
          <span className="mono">{hex32(address)}</span>{" "}
          <button
            type="button"
            aria-label={t("debug.remove.breakpoint", { address: hex32(address) })}
            onClick={() => onBreakpoint(address, false)}
          >
            {t("debug.remove")}
          </button>
        </li>
      ))}
    </ul>
  );
}

interface WatchpointListProps {
  points: Points;
  onWatchpoint(watch: Watchpoint, set: boolean): void;
}

/** The watchpoints set, each with its remove button. */
export function WatchpointList({ points, onWatchpoint }: WatchpointListProps) {
  const { t } = useI18n();
  if (points.watchpoints.length === 0) {
    return <p className="note">{t("debug.watchpoints.none")}</p>;
  }
  return (
    <ul className="point-list">
      {points.watchpoints.map((watch) => {
        const text = watchText(t, watch);
        return (
          <li key={`${watch.address}:${watch.length}:${watch.on}`}>
            {text}{" "}
            <button
              type="button"
              aria-label={t("debug.remove.watchpoint", { item: text })}
              onClick={() => onWatchpoint(watch, false)}
            >
              {t("debug.remove")}
            </button>
          </li>
        );
      })}
    </ul>
  );
}
