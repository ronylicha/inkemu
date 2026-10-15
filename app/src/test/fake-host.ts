// A host for the tests: it records the commands, answers as told, and lets a test send updates
// as the emulator's thread would.

import { vi } from "vitest";
import type { HostApi } from "../ipc/host";
import type { DeviceView, Look, Started, Status, Update } from "../ipc/types";

export const LOOK: Look = {
  panel: { width: 8, height: 4, grayLevels: 4, rotation: 90, origin: "testPattern" },
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
    powerOn: vi.fn(async (_setup, _speed, _start, onUpdate) => {
      listener = onUpdate;
      return started;
    }),
    openReplay: vi.fn(async (_path, _romDir, _speed, _start, onUpdate) => {
      listener = onUpdate;
      return started;
    }),
    command: vi.fn(async () => undefined),
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
