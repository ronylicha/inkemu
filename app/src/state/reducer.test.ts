import { describe, expect, it } from "vitest";
import type { Look, Started, Status, Update } from "../ipc/types";
import { type AppState, initialState, MAX_JOURNAL, reducer } from "./reducer";

const look: Look = {
  panel: { width: 800, height: 480, grayLevels: 4, rotation: 90, origin: "testPattern" },
  buttons: [{ id: "power", edge: "top", order: 0 }],
};

const status: Status = {
  life: 1,
  run: "running",
  halt: null,
  end: null,
  speed: "realTime",
  speedRatio: null,
  recording: null,
  replay: null,
  machine: null,
  usbConsoleSilenced: false,
};

const answer: Started = {
  notices: [{ level: "info", text: "inkemu: Xteink X4" }],
  look,
  console: "uart0",
  deviceName: "Xteink X4",
};

function started(from: AppState = initialState(), start = 1) {
  const starting = reducer(from, { type: "starting", start });
  return reducer(starting, { type: "started", start, started: answer });
}

const send = (state: AppState, start: number, ...updates: Update[]) =>
  reducer(state, { type: "updates", start, updates });

const text = (channel: "uart0" | "usb-serial-jtag", value: string): Update => ({
  event: "console",
  data: { channel, text: value },
});

describe("reducer", () => {
  it("starts a run with what the host gave back", () => {
    const state = started();
    expect(state.phase).toBe("on");
    expect(state.current).toBe(1);
    expect(state.starting).toBeNull();
    expect(state.console).toBe("uart0");
    expect(state.journal).toEqual([{ id: 0, level: "info", text: "inkemu: Xteink X4" }]);
  });

  it("keeps the console text of each channel apart", () => {
    let state = send(started(), 1, text("uart0", "a\r\n"), text("usb-serial-jtag", "b"));
    expect(state.consoles).toEqual({ uart0: "a\n", uart1: "", "usb-serial-jtag": "b" });
    state = reducer(state, { type: "clearConsole", channel: "uart0" });
    expect(state.consoles.uart0).toBe("");
  });

  it("removes a colour escape cut between two pieces of a console's text", () => {
    const esc = String.fromCharCode(27);
    let state = send(started(), 1, text("uart0", `ok${esc}[0;3`), text("usb-serial-jtag", "x"));
    expect(state.consoles.uart0).toBe("ok");
    state = send(state, 1, text("uart0", `2mgreen${esc}[0m\n`));
    expect(state.consoles.uart0).toBe("okgreen\n");
    expect(state.consoles["usb-serial-jtag"]).toBe("x");
  });

  it("keeps the last status, refresh and end the host sent", () => {
    let state = send(
      started(),
      1,
      { event: "status", data: status },
      { event: "frame", data: { generation: 3, refresh: "full", life: 1 } },
      { event: "lifeEnded", data: { life: 1, end: "interrupted", verdict: null } },
    );
    expect(state.status).toEqual(status);
    // Of the emulator that announced it, the first start's.
    expect(state.frame).toEqual({ start: 1, generation: 3, life: 1 });
    expect(state.lastEnd).toEqual({ life: 1, end: "interrupted", verdict: null });
    state = send(state, 1, { event: "stopped" });
    expect(state.phase).toBe("off");
  });

  it("keeps only the notices of an emulator a newer one replaced", () => {
    let state = send(started(), 1, text("uart0", "old"));
    state = started(state, 2);
    state = send(
      state,
      1,
      text("uart0", "late"),
      { event: "status", data: { ...status, life: 9 } },
      { event: "notice", data: { level: "error", text: "error: the flash was not saved" } },
      { event: "stopped" },
    );
    expect(state.current).toBe(2);
    expect(state.phase).toBe("on");
    expect(state.consoles.uart0).toBe("");
    expect(state.status).toBeNull();
    expect(state.journal.at(-1)?.text).toBe("error: the flash was not saved");
  });

  it("shows a new emulator from the answer to its start, its lines before what it said first", () => {
    let state = send(started(), 1, text("uart0", "old"));
    state = reducer(state, { type: "starting", start: 2 });
    state = send(state, 1, text("uart0", " more"));
    expect(state.consoles.uart0).toBe("old more");
    state = send(state, 2, text("uart0", "new"), {
      event: "notice",
      data: { level: "info", text: "early" },
    });
    // Not shown yet: the one shown is still the first.
    expect(state.current).toBe(1);
    expect(state.consoles.uart0).toBe("old more");
    state = reducer(state, { type: "started", start: 2, started: answer });
    expect(state.current).toBe(2);
    expect(state.consoles.uart0).toBe("new");
    expect(state.starting).toBeNull();
    expect(state.journal.slice(-2).map((entry) => entry.text)).toEqual([
      "inkemu: Xteink X4",
      "early",
    ]);
  });

  it("keeps the answer of a start set aside in the journal, without a banner", () => {
    let state = started(started(), 2);
    state = reducer(state, {
      type: "failed",
      start: 1,
      error: { kind: "stopped", message: "the emulator has stopped" },
    });
    expect(state.error).toBeNull();
    expect(state.current).toBe(2);
    expect(state.journal.at(-1)).toMatchObject({
      level: "warning",
      text: "the emulator has stopped",
    });
  });

  it("forgets what a refused start's emulator said", () => {
    let state = reducer(started(), { type: "starting", start: 2 });
    state = send(state, 2, text("uart0", "never"));
    state = reducer(state, { type: "failed", start: 2, error: { kind: "rom", message: "m" } });
    expect(state.early).toEqual([]);
    expect(state.current).toBe(1);
  });

  it("leaves the device that runs as it is when a start is refused, and shows why", () => {
    let state = send(started(), 1, text("uart0", "running"));
    state = reducer(state, { type: "starting", start: 2 });
    state = reducer(state, { type: "failed", start: 2, error: { kind: "rom", message: "m" } });
    expect(state.phase).toBe("on");
    expect(state.current).toBe(1);
    expect(state.starting).toBeNull();
    expect(state.error).toEqual({ kind: "rom", message: "m" });
    state = send(state, 1, text("uart0", " still"));
    expect(state.consoles.uart0).toBe("running still");
    expect(reducer(state, { type: "dismissError" }).error).toBeNull();
  });

  it("stays off when a first start is refused", () => {
    const starting = reducer(initialState(), { type: "starting", start: 1 });
    const failed = reducer(starting, {
      type: "failed",
      start: 1,
      error: { kind: "rom", message: "m" },
    });
    expect(failed.phase).toBe("off");
    expect(failed.starting).toBeNull();
  });

  it("bounds the journal, the oldest entries going first, with stable keys", () => {
    let state = started();
    for (let i = 0; i < MAX_JOURNAL + 5; i += 1) {
      state = send(state, 1, { event: "notice", data: { level: "warning", text: `w${i}` } });
    }
    expect(state.journal).toHaveLength(MAX_JOURNAL);
    expect(state.journal[0]?.id).toBe(6);
    expect(state.journal.at(-1)?.text).toBe(`w${MAX_JOURNAL + 4}`);
  });

  it("adds the interface's own lines to the journal", () => {
    const state = reducer(started(), {
      type: "said",
      notice: { level: "info", text: "Capture saved: a.png" },
    });
    expect(state.journal.at(-1)).toEqual({ id: 1, level: "info", text: "Capture saved: a.png" });
    expect(state.told).toBe("Capture saved: a.png");
  });

  it("selects the first device when the kept one is not listed", () => {
    const state = reducer(
      { ...initialState(), device: "gone" },
      {
        type: "devices",
        devices: [
          {
            id: "x3",
            name: "Xteink X3",
            console: "usb-serial-jtag",
            flashMib: 16,
            look,
            shippedBootloader: null,
            noBootloader: null,
          },
        ],
      },
    );
    expect(state.device).toBe("x3");
  });
});
