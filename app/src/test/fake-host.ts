// A host for the tests: it records the commands, answers as told, and lets a test send updates
// as the emulator's thread would.

import { vi } from "vitest";
import type { HostApi } from "../ipc/host";
import type {
  DebugView,
  DeviceView,
  Look,
  MonitorView,
  Started,
  Status,
  Update,
} from "../ipc/types";

export const LOOK: Look = {
  panel: {
    width: 8,
    height: 4,
    shades: { levels: 4 },
    rotation: 90,
    origin: "testPattern",
    approximate: false,
  },
  buttons: [
    { id: "power", edge: "top", order: 0 },
    { id: "up", edge: "right", order: 0 },
    { id: "confirm", edge: "bottom", order: 0 },
  ],
};

export const X4: DeviceView = {
  id: "x4",
  name: "Xteink X4",
  console: "usb-serial-jtag",
  flashMib: 16,
  look: LOOK,
  shippedBootloader: "esp-idf-v5.5.2-esp32c3-dio",
  noBootloader: null,
  // The sizes host::catalog gives for a new card.
  sdSlot: { minMib: 64, maxMib: 2_097_024, defaultMib: 1024 },
  usbPower: true,
};

/** The look of a wired device: its own panel, approximate while its optics are unmeasured. */
export const WIRED_LOOK: Look = {
  ...LOOK,
  panel: { ...LOOK.panel, shades: "continuous", origin: "device", approximate: true },
};

export function status(overrides: Partial<Status> = {}): Status {
  return {
    life: 1,
    run: "running",
    halt: null,
    end: null,
    speed: "realTime",
    speedRatio: null,
    recording: null,
    replay: null,
    machine: {
      deviceTimeNanos: 1_500_000_000,
      cycles: 1000,
      instructions: 900,
      pc: 0x4000_0400,
      cpuHz: 160_000_000,
      cpuAsleep: false,
      deepSleep: null,
      usbHost: "open",
      usbHostChangesPending: 0,
      journalComplete: true,
    },
    usbConsoleSilenced: false,
    stop: null,
    ...overrides,
  };
}

/** What the debugger shows: a CPU stopped at a breakpoint in the ROM, two lines of code around
 * it, a word of the SRAM followed by peripheral registers, and the points. */
export function debugView(overrides: Partial<DebugView> = {}): DebugView {
  const names = [
    "zero",
    "ra",
    "sp",
    "gp",
    "tp",
    "t0",
    "t1",
    "t2",
    "s0",
    "s1",
    "a0",
    "a1",
    "a2",
    "a3",
    "a4",
    "a5",
    "a6",
    "a7",
    "s2",
    "s3",
    "s4",
    "s5",
    "s6",
    "s7",
    "s8",
    "s9",
    "s10",
    "s11",
    "t3",
    "t4",
    "t5",
    "t6",
  ];
  return {
    cpu: {
      pc: 0x4000_0004,
      privilege: "machine",
      asleep: false,
      registers: names.map((name, number) => ({
        number,
        name,
        value: number === 2 ? 0x3fcd_fff0 : number,
      })),
      csrs: [
        { address: 0x300, name: "mstatus", value: 0x1800 },
        { address: 0x342, name: "mcause", value: 0 },
      ],
    },
    disassembly: [
      {
        address: 0x4000_0000,
        length: 4,
        instruction: { text: "lui t0, 0x3fc80", compressed: false },
        notExecuted: false,
        current: false,
        breakpoint: false,
      },
      {
        address: 0x4000_0004,
        length: 2,
        instruction: { text: "addi t1, t1, 1", compressed: true },
        notExecuted: false,
        current: true,
        breakpoint: true,
      },
      {
        address: 0x4000_0006,
        length: 2,
        instruction: null,
        notExecuted: false,
        current: false,
        breakpoint: false,
      },
      {
        address: 0x4201_0000,
        length: 2,
        instruction: null,
        notExecuted: true,
        current: false,
        breakpoint: false,
      },
    ],
    memory: {
      address: 0x3fc8_0000,
      bytes: [0x2a, 0x00, 0x41, 0x7f, null, null],
      regions: [
        { start: 0x3fc8_0000, length: 4, region: "sram" },
        { start: 0x3fc8_0004, length: 2, region: "flash" },
      ],
    },
    points: {
      breakpoints: [0x4000_0004],
      watchpoints: [{ address: 0x3fc8_0000, length: 4, on: "write" }],
    },
    ...overrides,
  };
}

/**
 * What the monitor shows: 1 ms of device time, at 20 then 160 MHz, with a deep sleep; a RAM of four
 * pages, one written before the last reset and one since; one source, one line, one block; a
 * window since the reading before.
 */
export function monitorView(overrides: Partial<MonitorView> = {}): MonitorView {
  return {
    deviceTimeNanos: 1_000_000,
    cycles: 50_000,
    instructions: 40_000,
    cpuHz: 160_000_000,
    resets: 1,
    speedRatio: 0.5,
    time: {
      runningNanos: 500_000,
      waitingNanos: 300_000,
      deepSleepNanos: 200_000,
      loadPermille: 625,
    },
    recent: {
      deviceNanos: 250_000,
      time: {
        runningNanos: 200_000,
        waitingNanos: 50_000,
        deepSleepNanos: 0,
        loadPermille: 800,
      },
      instructions: 10_000,
      interrupts: 3,
      registerAccesses: 12,
    },
    clocks: [
      { hz: 20_000_000, runningNanos: 100_000, waitingNanos: 0 },
      { hz: 160_000_000, runningNanos: 400_000, waitingNanos: 300_000 },
    ],
    memories: [
      {
        region: "sram",
        base: 0x3fc8_0000,
        size: 4 * 4096,
        pageBytes: 4096,
        pages: ["never", "before", "never", "since"],
        written: 2,
        writtenSinceReset: 1,
        lowest: 0x3fc8_1000,
        highestEnd: 0x3fc8_4000,
        writes: 9,
        writesSinceReset: 5,
      },
    ],
    interrupts: [{ source: 37, name: "SYSTIMER_TARGET0_INT", raised: 12, recent: 3 }],
    lines: [{ line: 4, taken: 11 }],
    peripherals: [
      { base: 0x6002_3000, peripherals: ["SYSTIMER"], reads: 30, writes: 12, recent: 12 },
    ],
    power: {
      source: "ESP32-C3 Series Datasheet v2.4 (2026-05-06), tables 5-8 and 5-9",
      rows: [
        {
          state: "running",
          hz: 20_000_000,
          nanos: 100_000,
          microamps: null,
          microampHours: null,
        },
        {
          state: "running",
          hz: 160_000_000,
          nanos: 400_000,
          microamps: [23_000, 28_000],
          microampHours: [0.002_555, 0.003_111],
        },
        {
          state: "deepSleep",
          hz: null,
          nanos: 200_000,
          microamps: [5, 5],
          microampHours: [0.000_000_28, 0.000_000_28],
        },
      ],
      microampHours: [0.002_555, 0.003_111],
      unpricedNanos: 100_000,
    },
    panel: { refreshes: 3, last: "partial" },
    ...overrides,
  };
}

export interface FakeHost extends HostApi {
  /** Sends an update to the interface, as the thread of the emulator does. */
  send(update: Update): void;
}

export function fakeHost(overrides: Partial<HostApi> = {}): FakeHost {
  let listener: ((update: Update) => void) | null = null;
  const started: Started = {
    notices: [{ level: "info", text: "inkemu: Xteink X4 (ESP32-C3 v0.3, mask ROM rev3)" }],
    look: LOOK,
    console: "usb-serial-jtag",
    deviceName: "Xteink X4",
  };
  const host: HostApi = {
    devices: vi.fn(async () => [X4]),
    powerOn: vi.fn(async (_setup, _speed, _start, _points, onUpdate) => {
      listener = onUpdate;
      return started;
    }),
    openReplay: vi.fn(async (_path, _romDir, _speed, _start, _points, onUpdate) => {
      listener = onUpdate;
      return started;
    }),
    command: vi.fn(async () => undefined),
    inspect: vi.fn(async () => debugView()),
    monitor: vi.fn(async () => monitorView()),
    frame: vi.fn(async () => null),
    saveCapture: vi.fn(async () => undefined),
    notices: vi.fn(async () => "NOTICE"),
    setLanguage: vi.fn(async () => undefined),
    pickFile: vi.fn(async () => "/home/user/fw/app.bin"),
    pickSavePath: vi.fn(async () => "/home/user/out.png"),
    ...overrides,
  };
  return {
    ...host,
    send(update) {
      listener?.(update);
    },
  };
}
