import { describe, expect, it, vi } from "vitest";
import type { Update } from "../ipc/types";
import { type Scheduler, updateBatcher } from "./batch";

/** A scheduler the test runs by hand. */
function manual() {
  const runs = new Map<number, () => void>();
  let next = 1;
  const scheduler: Scheduler = {
    schedule: (run) => {
      runs.set(next, run);
      next += 1;
      return next - 1;
    },
    cancel: (handle) => {
      runs.delete(handle);
    },
  };
  const frame = () => {
    const due = [...runs.values()];
    runs.clear();
    for (const run of due) {
      run();
    }
  };
  return { scheduler, frame, waiting: () => runs.size };
}

const text = (channel: "uart0" | "usb-serial-jtag", value: string): Update => ({
  event: "console",
  data: { channel, text: value },
});

describe("updateBatcher", () => {
  it("delivers once per frame, joining console text that stays on a channel", () => {
    const { scheduler, frame, waiting } = manual();
    const deliver = vi.fn();
    const batcher = updateBatcher(deliver, scheduler);
    batcher.push(1, text("uart0", "a"));
    batcher.push(1, text("uart0", "b"));
    batcher.push(1, text("usb-serial-jtag", "c"));
    batcher.push(1, { event: "notice", data: { level: "info", text: "n" } });
    batcher.push(1, text("uart0", "d"));
    expect(waiting()).toBe(1);
    expect(deliver).not.toHaveBeenCalled();
    frame();
    expect(deliver.mock.calls).toEqual([
      [
        1,
        [
          text("uart0", "ab"),
          text("usb-serial-jtag", "c"),
          { event: "notice", data: { level: "info", text: "n" } },
          text("uart0", "d"),
        ],
      ],
    ]);
  });

  it("keeps the updates of two emulators apart, in their order", () => {
    const { scheduler, frame } = manual();
    const deliver = vi.fn();
    const batcher = updateBatcher(deliver, scheduler);
    batcher.push(1, text("uart0", "old"));
    batcher.push(2, text("uart0", "new"));
    frame();
    expect(deliver.mock.calls).toEqual([
      [1, [text("uart0", "old")]],
      [2, [text("uart0", "new")]],
    ]);
  });

  it("delivers at once on flush, and nothing once cancelled", () => {
    const { scheduler, frame, waiting } = manual();
    const deliver = vi.fn();
    const batcher = updateBatcher(deliver, scheduler);
    batcher.push(1, text("uart0", "a"));
    batcher.flush();
    expect(deliver).toHaveBeenCalledTimes(1);
    expect(waiting()).toBe(0);
    batcher.push(1, text("uart0", "b"));
    batcher.cancel();
    frame();
    expect(deliver).toHaveBeenCalledTimes(1);
  });
});
