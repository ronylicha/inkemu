// The state of the interface: what the host sent, kept as it sent it. Nothing here decides what
// the device does (plan ruling 2c-R10).

import type {
  ConsoleChannel,
  DeviceView,
  HostError,
  Look,
  Notice,
  RunEnd,
  Started,
  Status,
  Update,
  VerdictView,
} from "../ipc/types";
import { appendConsole } from "./console";

export const MAX_JOURNAL = 1000;

export interface JournalEntry extends Notice {
  /** A stable key for the list, in the order of arrival. */
  id: number;
}

export interface LifeEnd {
  life: number;
  end: RunEnd;
  verdict: VerdictView | null;
}

export interface AppState {
  /** No emulator, one starting, one running or ended but not powered off. */
  phase: "off" | "starting" | "on";
  devices: DeviceView[];
  device: string;
  look: Look | null;
  deviceName: string;
  /** The console shown first. */
  console: ConsoleChannel;
  consoles: Record<ConsoleChannel, string>;
  journal: JournalEntry[];
  nextEntry: number;
  status: Status | null;
  lastEnd: LifeEnd | null;
  /** The generation of the last refresh the host announced, and of which life. */
  frame: { generation: number; life: number } | null;
  error: HostError | null;
}

export type Action =
  | { type: "devices"; devices: DeviceView[] }
  | { type: "selectDevice"; device: string }
  | { type: "starting" }
  | { type: "started"; started: Started }
  | { type: "failed"; error: HostError }
  | { type: "update"; update: Update }
  | { type: "clearConsole"; channel: ConsoleChannel }
  | { type: "dismissError" };

const EMPTY_CONSOLES: Record<ConsoleChannel, string> = {
  uart0: "",
  uart1: "",
  "usb-serial-jtag": "",
};

export function initialState(): AppState {
  return {
    phase: "off",
    devices: [],
    device: "x4",
    look: null,
    deviceName: "",
    console: "usb-serial-jtag",
    consoles: EMPTY_CONSOLES,
    journal: [],
    nextEntry: 0,
    status: null,
    lastEnd: null,
    frame: null,
    error: null,
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

function applyUpdate(state: AppState, update: Update): AppState {
  switch (update.event) {
    case "console": {
      const { channel, text } = update.data;
      return {
        ...state,
        consoles: { ...state.consoles, [channel]: appendConsole(state.consoles[channel], text) },
      };
    }
    case "notice":
      return withNotices(state, [update.data]);
    case "frame":
      return { ...state, frame: { generation: update.data.generation, life: update.data.life } };
    case "status":
      return { ...state, status: update.data };
    case "lifeEnded":
      return { ...state, lastEnd: update.data };
    case "stopped":
      return { ...state, phase: "off" };
  }
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
      return {
        ...state,
        phase: "starting",
        consoles: EMPTY_CONSOLES,
        status: null,
        lastEnd: null,
        frame: null,
        error: null,
      };
    case "started":
      return withNotices(
        {
          ...state,
          phase: "on",
          look: action.started.look,
          deviceName: action.started.deviceName,
          console: action.started.console,
        },
        action.started.notices,
      );
    case "failed":
      return {
        ...state,
        phase: state.phase === "starting" ? "off" : state.phase,
        error: action.error,
      };
    case "update":
      return applyUpdate(state, action.update);
    case "clearConsole":
      return { ...state, consoles: { ...state.consoles, [action.channel]: "" } };
    case "dismissError":
      return { ...state, error: null };
  }
}
