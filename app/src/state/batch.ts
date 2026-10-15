// The updates of the emulators, gathered until the next frame of the screen: a firmware that
// prints fast sends many console updates, and the window draws once per frame whatever it got
// (plan ruling 2c-R15). The host already sends its console text at most every 40 ms; this keeps
// the interface's work per frame to one render however the updates arrive.

import type { Update } from "../ipc/types";

export interface UpdateBatcher {
  /** Keeps `update` of the emulator started as `start`, for the next frame. */
  push(start: number, update: Update): void;
  /** Delivers what is kept now. */
  flush(): void;
  /** Forgets what is kept and the frame asked for: the window goes. */
  cancel(): void;
}

export interface Scheduler {
  schedule(run: () => void): number;
  cancel(handle: number): void;
}

const frames: Scheduler = {
  schedule: (run) =>
    typeof requestAnimationFrame === "function"
      ? requestAnimationFrame(run)
      : window.setTimeout(run, 16),
  cancel: (handle) =>
    typeof cancelAnimationFrame === "function"
      ? cancelAnimationFrame(handle)
      : window.clearTimeout(handle),
};

interface Kept {
  start: number;
  updates: Update[];
}

/**
 * Gathers updates and gives them to `deliver` once per frame, in the order they came, one call per
 * run of updates of the same emulator. Console text that follows console text of the same channel
 * joins it.
 */
export function updateBatcher(
  deliver: (start: number, updates: Update[]) => void,
  scheduler: Scheduler = frames,
): UpdateBatcher {
  let kept: Kept[] = [];
  let pending: number | null = null;

  const flush = () => {
    if (pending !== null) {
      scheduler.cancel(pending);
      pending = null;
    }
    const runs = kept;
    kept = [];
    for (const run of runs) {
      deliver(run.start, run.updates);
    }
  };

  return {
    push(start, update) {
      const last = kept.at(-1);
      if (last === undefined || last.start !== start) {
        kept.push({ start, updates: [update] });
      } else {
        const previous = last.updates.at(-1);
        if (
          previous?.event === "console" &&
          update.event === "console" &&
          previous.data.channel === update.data.channel
        ) {
          last.updates[last.updates.length - 1] = {
            event: "console",
            data: { channel: update.data.channel, text: previous.data.text + update.data.text },
          };
        } else {
          last.updates.push(update);
        }
      }
      if (pending === null) {
        pending = scheduler.schedule(() => {
          pending = null;
          flush();
        });
      }
    },
    flush,
    cancel() {
      if (pending !== null) {
        scheduler.cancel(pending);
        pending = null;
      }
      kept = [];
    },
  };
}
