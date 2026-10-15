// The only place the interface reaches the host: every command of the Tauri shell
// (app/src-tauri/src/commands.rs), behind an interface the components take from a context, so
// that tests give them a fake.

import { Channel, invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { createContext, useContext } from "react";
import { decodeFrame, type Frame } from "./frame";
import type { Command, DeviceView, HostError, Setup, Speed, Started, Update } from "./types";
import { isHostError } from "./types";

export type FileKind = "firmware" | "bootloader" | "flash" | "session" | "romDir";
export type SaveKind = "flashFile" | "session" | "capture";

export interface HostApi {
  devices(): Promise<DeviceView[]>;
  powerOn(setup: Setup, speed: Speed, onUpdate: (update: Update) => void): Promise<Started>;
  openReplay(
    path: string,
    romDir: string | null,
    speed: Speed,
    onUpdate: (update: Update) => void,
  ): Promise<Started>;
  command(command: Command): Promise<void>;
  /** The image of the last refresh, `null` before the first. */
  frame(): Promise<Frame | null>;
  saveCapture(path: string): Promise<void>;
  notices(): Promise<string>;
  pickFile(kind: FileKind): Promise<string | null>;
  pickSavePath(kind: SaveKind): Promise<string | null>;
}

/** Turns what a rejected command gives into a `HostError`. */
export function asHostError(reason: unknown): HostError {
  if (isHostError(reason)) {
    return reason;
  }
  return { kind: "ipc", message: reason instanceof Error ? reason.message : String(reason) };
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (reason) {
    throw asHostError(reason);
  }
}

const FILTERS: Record<FileKind | SaveKind, { name: string; extensions: string[] }[]> = {
  firmware: [{ name: "ESP-IDF application", extensions: ["bin"] }],
  bootloader: [{ name: "ESP-IDF bootloader", extensions: ["bin"] }],
  flash: [{ name: "Flash image", extensions: ["bin", "img"] }],
  session: [{ name: "inkemu session", extensions: ["inkrec"] }],
  romDir: [],
  flashFile: [{ name: "Flash file", extensions: ["flash", "bin"] }],
  capture: [{ name: "PNG", extensions: ["png"] }],
};

function channelOf(onUpdate: (update: Update) => void): Channel<Update> {
  const channel = new Channel<Update>();
  channel.onmessage = onUpdate;
  return channel;
}

/**
 * `run`, one call at a time: each call starts once the one before has settled, whatever its
 * outcome. The emulator does the commands in the order they reach it; this keeps that order the
 * order the user gave them (a press before its release), even when one takes long (2c-R16).
 */
export function serialized<A extends unknown[], T>(
  run: (...args: A) => Promise<T>,
): (...args: A) => Promise<T> {
  let tail: Promise<unknown> = Promise.resolve();
  return (...args) => {
    const next = tail.then(() => run(...args));
    tail = next.catch(() => undefined);
    return next;
  };
}

/**
 * Gives a command to the emulator: the shell gives it at once, in the order it received it, and
 * answers through a channel once the emulator has done it.
 */
function sendCommand(command: Command): Promise<void> {
  return new Promise((resolve, reject) => {
    const answer = new Channel<HostError | null>();
    answer.onmessage = (refusal) => {
      if (refusal === null) {
        resolve();
      } else {
        reject(asHostError(refusal));
      }
    };
    call<void>("command", { command, answer }).catch(reject);
  });
}

const command = serialized(sendCommand);

export const tauriHost: HostApi = {
  devices: () => call<DeviceView[]>("devices"),
  powerOn: (setup, speed, onUpdate) =>
    call<Started>("power_on", { setup, speed, onUpdate: channelOf(onUpdate) }),
  openReplay: (path, romDir, speed, onUpdate) =>
    call<Started>("open_replay", { path, romDir, speed, onUpdate: channelOf(onUpdate) }),
  command,
  frame: async () => {
    const buffer = await call<ArrayBuffer>("frame");
    return buffer.byteLength === 0 ? null : decodeFrame(buffer);
  },
  saveCapture: (path) => command({ kind: "capture", path }),
  notices: () => call<string>("notices"),
  pickFile: async (kind) => {
    const picked = await open({
      multiple: false,
      directory: kind === "romDir",
      filters: FILTERS[kind],
    });
    return typeof picked === "string" ? picked : null;
  },
  pickSavePath: async (kind) => (await save({ filters: FILTERS[kind] })) ?? null,
};

export const HostContext = createContext<HostApi>(tauriHost);

export function useHost(): HostApi {
  return useContext(HostContext);
}
