// The shapes the host crate sends and takes, as serde writes them (camelCase). They mirror
// `crates/host/src/{device,engine,ports,catalog}.rs`: a change there is a change here.

export type ConsoleChannel = "uart0" | "uart1" | "usb-serial-jtag";
export const CONSOLE_CHANNELS: readonly ConsoleChannel[] = ["uart0", "uart1", "usb-serial-jtag"];

export type UsbHost = "absent" | "attached" | "open";
export const USB_HOSTS: readonly UsbHost[] = ["absent", "attached", "open"];

export type Speed = "realTime" | "fastest";
export type Level = "info" | "warning" | "error";
export type Halt = "idle" | "deepSleep";
export type RunState = "running" | "paused" | "halted" | "ended";
export type RunEnd =
  | "budget"
  | "timeLimit"
  | "consoleClosed"
  | "consoleError"
  | "strictMmio"
  | "idle"
  | "deepSleep"
  | "interrupted"
  /** The board cut its supply: the device is off; its power key turns it on again. */
  | "powerOff";
export type RefreshKind = "full" | "partial";
export type Edge = "top" | "right" | "bottom" | "left";
export type PanelOrigin = "testPattern" | "device";
export type Rotation = 0 | 90 | 180 | 270;

/** The greys of a panel: a few levels (a test pattern), or any value from 0 to 255. */
export type Shades = { levels: number } | "continuous";

export interface PanelDescription {
  width: number;
  height: number;
  shades: Shades;
  rotation: Rotation;
  origin: PanelOrigin;
  /** The image rests on optical parameters nobody measured: the interface says so. */
  approximate: boolean;
}

export interface ButtonSpec {
  id: string;
  edge: Edge;
  order: number;
}

export interface Look {
  panel: PanelDescription;
  buttons: ButtonSpec[];
}

export interface DeviceView {
  id: string;
  name: string;
  console: ConsoleChannel;
  flashMib: number;
  look: Look;
  shippedBootloader: string | null;
  noBootloader: string | null;
  /** The device has an SD card slot: the form offers a card. */
  sdSlot: boolean;
  /** The device sees a USB cable: the form offers to plug one in. */
  usbPower: boolean;
}

/** The SD card a run starts with: an existing image, or a blank one made first. */
export interface SdCard {
  path: string;
  /** The card is write-protected: the image is never written. */
  readOnly: boolean;
  /** Make a blank card of this many MiB at `path` first; `null` for an existing image. */
  createMib: number | null;
}

export interface Notice {
  level: Level;
  text: string;
}

export interface DeepSleepStatus {
  sinceNanos: number;
  timerWakeNanos: number | null;
  wakeSources: number;
}

export interface MachineStatus {
  deviceTimeNanos: number;
  cycles: number;
  instructions: number;
  pc: number;
  cpuHz: number;
  cpuAsleep: boolean;
  deepSleep: DeepSleepStatus | null;
  usbHost: UsbHost;
  usbHostChangesPending: number;
  journalComplete: boolean;
}

export interface Status {
  life: number;
  run: RunState;
  halt: Halt | null;
  end: RunEnd | null;
  speed: Speed;
  speedRatio: number | null;
  recording: string | null;
  replay: string | null;
  machine: MachineStatus | null;
  usbConsoleSilenced: boolean;
}

export type VerdictView =
  | { kind: "identical" }
  | { kind: "diverged"; detail: string }
  | { kind: "cut"; detail: string }
  | { kind: "unknown"; detail: string };

export type Update =
  | { event: "console"; data: { channel: ConsoleChannel; text: string } }
  | { event: "notice"; data: Notice }
  | { event: "frame"; data: { generation: number; refresh: RefreshKind; life: number } }
  | { event: "status"; data: Status }
  | { event: "lifeEnded"; data: { life: number; end: RunEnd; verdict: VerdictView | null } }
  | { event: "stopped" };

/** What the interface asks a run to be: the options of `inkemu run`. */
export interface Setup {
  device: string;
  image: string | null;
  firmware: string | null;
  bootloader: string | null;
  flashFile: string | null;
  resetFlash: boolean;
  romDir: string | null;
  usbHost: UsbHost;
  strictMmio: boolean;
  record: string | null;
  /** Seconds of device time, as `--max-time` reads them; empty for no limit. */
  maxTime: string;
  /** CPU cycles; empty for no limit. */
  maxCycles: string;
  console: ConsoleChannel | null;
  /** A USB cable powers the device from the start (`--usb-power`). */
  usbPower: boolean;
  /** The card in the slot (`--sd-image`); `null` for an empty slot. */
  sd: SdCard | null;
}

export type Command =
  | { kind: "run" }
  | { kind: "pause" }
  | { kind: "step" }
  | { kind: "reset" }
  | { kind: "stop" }
  | { kind: "speed"; speed: Speed }
  | { kind: "console"; channel: ConsoleChannel; bytes: number[] }
  | { kind: "usbHost"; host: UsbHost }
  | { kind: "button"; id: string; pressed: boolean }
  | { kind: "record"; path: string }
  | { kind: "capture"; path: string };

/** What starting a run or a replay gives back. */
export interface Started {
  notices: Notice[];
  look: Look;
  console: ConsoleChannel;
  deviceName: string;
}

/** A refusal: `kind` names what was refused, `message` is the host's text. */
export interface HostError {
  kind: string;
  message: string;
}

export function isHostError(value: unknown): value is HostError {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as { kind?: unknown }).kind === "string" &&
    typeof (value as { message?: unknown }).message === "string"
  );
}
