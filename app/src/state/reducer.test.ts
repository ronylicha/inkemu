import { describe, expect, it } from "vitest";
import type { Look, Status } from "../ipc/types";
import { initialState, MAX_JOURNAL, reducer } from "./reducer";

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

function started() {
  const starting = reducer(initialState(), { type: "starting" });
  return reducer(starting, {
    type: "started",
    started: {
      notices: [{ level: "info", text: "inkemu: Xteink X4" }],
      look,
      console: "uart0",
      deviceName: "Xteink X4",
    },
  });
}

describe("reducer", () => {
  it("starts a run with what the host gave back", () => {
    const state = started();
    expect(state.phase).toBe("on");
    expect(state.console).toBe("uart0");
    expect(state.journal).toEqual([{ id: 0, level: "info", text: "inkemu: Xteink X4" }]);
  });

  it("keeps the console text of each channel apart", () => {
    let state = started();
    state = reducer(state, {
      type: "update",
      update: { event: "console", data: { channel: "uart0", text: "a\r\n" } },
    });
    state = reducer(state, {
      type: "update",
      update: { event: "console", data: { channel: "usb-serial-jtag", text: "b" } },
    });
    expect(state.consoles).toEqual({ uart0: "a\n", uart1: "", "usb-serial-jtag": "b" });
    state = reducer(state, { type: "clearConsole", channel: "uart0" });
    expect(state.consoles.uart0).toBe("");
  });

  it("keeps the last status, refresh and end the host sent", () => {
    let state = started();
    state = reducer(state, { type: "update", update: { event: "status", data: status } });
    state = reducer(state, {
      type: "update",
      update: { event: "frame", data: { generation: 3, refresh: "full", life: 1 } },
    });
    state = reducer(state, {
      type: "update",
      update: {
        event: "lifeEnded",
        data: { life: 1, end: "interrupted", verdict: null },
      },
    });
    expect(state.status).toEqual(status);
    expect(state.frame).toEqual({ generation: 3, life: 1 });
    expect(state.lastEnd).toEqual({ life: 1, end: "interrupted", verdict: null });
    state = reducer(state, { type: "update", update: { event: "stopped" } });
    expect(state.phase).toBe("off");
  });

  it("bounds the journal, the oldest entries going first, with stable keys", () => {
    let state = started();
    for (let i = 0; i < MAX_JOURNAL + 5; i += 1) {
      state = reducer(state, {
        type: "update",
        update: { event: "notice", data: { level: "warning", text: `w${i}` } },
      });
    }
    expect(state.journal).toHaveLength(MAX_JOURNAL);
    expect(state.journal[0]?.id).toBe(6);
    expect(state.journal.at(-1)?.text).toBe(`w${MAX_JOURNAL + 4}`);
  });

  it("goes back to off when a start is refused, and shows why", () => {
    const starting = reducer(initialState(), { type: "starting" });
    const failed = reducer(starting, { type: "failed", error: { kind: "rom", message: "m" } });
    expect(failed.phase).toBe("off");
    expect(failed.error).toEqual({ kind: "rom", message: "m" });
    expect(reducer(failed, { type: "dismissError" }).error).toBeNull();
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
