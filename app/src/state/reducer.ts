// The state of the interface: what the host sent, kept as it sent it. Nothing here decides what
// the device does (plan ruling 2c-R10).
//
// Every start is numbered by the window (`start`), and every update carries the number of the
// emulator that sent it: the state shows one emulator, and an emulator that a newer one replaced
// may still be sending its last updates. Of those, only the notices are kept, in the journal (what
// its last life could not keep, for instance); the rest would show another device (2c-R18). A new
// emulator is shown from the answer to its start: what it sent before is kept until then, after
// the answer's lines.

import {
  type ConsoleChannel,
  type DeviceView,
  type HostError,
  type Look,
  NO_POINTS,
  type Notice,
  type Points,
  type RunEnd,
  type Started,
  type Status,
  type Update,
  type VerdictView,
} from "../ipc/types";
import { appendConsole, holdOpenEscape } from "./console";

export const MAX_JOURNAL = 1000;

export interface JournalEntry extends Notice {
  /** A stable key for the list, in the order of arrival. */
  id: number;
}

/** A refresh the host announced: of which emulator (its start), which life, which generation. */
export interface FrameSignal {
  start: number;
  generation: number;
  life: number;
}

export interface LifeEnd {
  life: number;
  end: RunEnd;
  verdict: VerdictView | null;
}

export interface AppState {
  /** Whether the emulator shown runs or ended without being powered off. */
  phase: "off" | "on";
  /** The number of the emulator shown; 0 before the first. */
  current: number;
  /** The number of a start not answered yet, which the shown emulator outlives if it is refused. */
  starting: number | null;
  /** What the emulator of that start sent before its answer, shown with the answer. */
  early: Update[];
  devices: DeviceView[];
  device: string;
  look: Look | null;
  deviceName: string;
  /** The console shown first. */
  console: ConsoleChannel;
  consoles: Record<ConsoleChannel, string>;
  /** Per console, the start of an escape its last piece of text ended in. */
  held: Record<ConsoleChannel, string>;
  journal: JournalEntry[];
  nextEntry: number;
  status: Status | null;
  lastEnd: LifeEnd | null;
  /** The generation of the last refresh the host announced, and of which life. */
  frame: FrameSignal | null;
  error: HostError | null;
  /** The last line of the interface's own, which a screen reader announces. */
  told: string;
  /**
   * The points of the debugger, as the host last showed them: a new start begins with them
   * (design SP4a, 4a-D8). They outlive the emulator that had them.
   */
  points: Points;
}

export type Action =
  | { type: "devices"; devices: DeviceView[] }
  | { type: "selectDevice"; device: string }
  | { type: "starting"; start: number }
  | { type: "started"; start: number; started: Started }
  /** A refusal; `start` names the start it answers, if it answers one. */
  | { type: "failed"; error: HostError; start?: number }
  | { type: "updates"; start: number; updates: Update[] }
  /** A line of the interface's own, translated: what a capture or a recording became. */
  | { type: "said"; notice: Notice }
  | { type: "clearConsole"; channel: ConsoleChannel }
  | { type: "dismissError" }
  /** The points the host showed with a view of the debugger, of emulator `start`. */
  | { type: "points"; start: number; points: Points };

const EMPTY_CONSOLES: Record<ConsoleChannel, string> = {
  uart0: "",
  uart1: "",
  "usb-serial-jtag": "",
};

export function initialState(): AppState {
  return {
    phase: "off",
    current: 0,
    starting: null,
    early: [],
    devices: [],
    device: "x4",
    look: null,
    deviceName: "",
    console: "usb-serial-jtag",
    consoles: EMPTY_CONSOLES,
    held: EMPTY_CONSOLES,
    journal: [],
    nextEntry: 0,
    status: null,
    lastEnd: null,
    frame: null,
    error: null,
    told: "",
    points: NO_POINTS,
  };
}

function withNotices(state: AppState, notices: readonly Notice[]): AppState {
  const added = notices.map((notice, i) => ({ ...notice, id: state.nextEntry + i }));
  const journal = [...state.journal, ...added];
  return {
    ...state,
    journal: journal.length > MAX_JOURNAL ? journal.slice(journal.length - MAX_JOURNAL) : journal,
    nextEntry: state.nextEntry + added.length,
  };
}

/** The state showing emulator `start` from its beginning: what the one before showed goes. */
function adopt(state: AppState, start: number): AppState {
  return {
    ...state,
    phase: "on",
    current: start,
    consoles: EMPTY_CONSOLES,
    held: EMPTY_CONSOLES,
    status: null,
    lastEnd: null,
    frame: null,
  };
}

function applyUpdate(state: AppState, update: Update): AppState {
  switch (update.event) {
    case "console": {
      const { channel, text } = update.data;
      const [ready, held] = holdOpenEscape(state.held[channel] + text);
      return {
        ...state,
        consoles: { ...state.consoles, [channel]: appendConsole(state.consoles[channel], ready) },
        held: { ...state.held, [channel]: held },
      };
    }
    case "notice":
      return withNotices(state, [update.data]);
    case "frame":
      return {
        ...state,
        frame: {
          start: state.current,
          generation: update.data.generation,
          life: update.data.life,
        },
      };
    case "status":
      return { ...state, status: update.data };
    case "lifeEnded":
      return { ...state, lastEnd: update.data };
    case "stopped":
      return { ...state, phase: "off" };
  }
}

function applyUpdates(state: AppState, start: number, updates: readonly Update[]): AppState {
  if (start === state.starting && start !== state.current) {
    // The new emulator speaks before its start is answered: kept until the answer, whose lines
    // (the first of the run) come first in the journal.
    return { ...state, early: [...state.early, ...updates] };
  }
  if (start !== state.current) {
    return withNotices(
      state,
      updates.flatMap((update) => (update.event === "notice" ? [update.data] : [])),
    );
  }
  return updates.reduce(applyUpdate, state);
}

export function reducer(state: AppState, action: Action): AppState {
  switch (action.type) {
    case "devices": {
      const known = action.devices.some((device) => device.id === state.device);
      const device = known ? state.device : (action.devices[0]?.id ?? state.device);
      return { ...state, devices: action.devices, device };
    }
    case "selectDevice":
      return { ...state, device: action.device };
    case "starting":
      return { ...state, starting: action.start, early: [], error: null };
    case "started": {
      if (action.start !== state.starting) {
        return state;
      }
      // Its lines first, then what it said while its start was answered.
      const shown = withNotices(
        {
          ...adopt(state, action.start),
          starting: null,
          early: [],
          look: action.started.look,
          deviceName: action.started.deviceName,
          console: action.started.console,
        },
        action.started.notices,
      );
      return state.early.reduce(applyUpdate, shown);
    }
    case "failed": {
      if (
        action.start !== undefined &&
        action.start !== state.starting &&
        action.start !== state.current
      ) {
        // A start already set aside (a later one took its place): its answer is a line of the
        // journal, not a refusal of anything the user is doing now.
        return withNotices(state, [{ level: "warning", text: action.error.message }]);
      }
      const answered = action.start !== undefined && action.start === state.starting;
      return {
        ...state,
        starting: answered ? null : state.starting,
        early: answered ? [] : state.early,
        error: action.error,
      };
    }
    case "updates":
      return applyUpdates(state, action.start, action.updates);
    case "said":
      return withNotices({ ...state, told: action.notice.text }, [action.notice]);
    case "clearConsole":
      return {
        ...state,
        consoles: { ...state.consoles, [action.channel]: "" },
        held: { ...state.held, [action.channel]: "" },
      };
    case "dismissError":
      return { ...state, error: null };
    case "points":
      // A view of an emulator a newer start replaced does not speak for the points any more.
      return action.start === state.current ? { ...state, points: action.points } : state;
  }
}
