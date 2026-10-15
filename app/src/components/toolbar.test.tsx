import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Status } from "../ipc/types";
import type { AppState } from "../state/reducer";
import { status } from "../test/fake-host";
import { renderWith } from "../test/render";
import { Toolbar } from "./toolbar";

function toolbar(phase: AppState["phase"], current: Status | null, starting = false) {
  const onCommand = vi.fn();
  renderWith(
    <Toolbar
      phase={phase}
      starting={starting}
      status={current}
      speed="realTime"
      hasImage={false}
      theme="system"
      onPowerOn={vi.fn()}
      onCommand={onCommand}
      onSpeed={vi.fn()}
      onRecord={vi.fn()}
      onReplay={vi.fn()}
      onCapture={vi.fn()}
      onAbout={vi.fn()}
      onTheme={vi.fn()}
    />,
  );
  return onCommand;
}

describe("Toolbar", () => {
  it("waits for a start without taking the controls of the device that runs", () => {
    toolbar("on", status({ run: "running" }), true);
    expect(screen.getByRole("button", { name: "Starting…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Pause" })).toBeEnabled();
  });

  it("offers to power on only, when no device runs", () => {
    toolbar("off", null);
    expect(screen.getByRole("button", { name: "Power on…" })).toBeEnabled();
    for (const name of ["Run", "Step", "Reset", "Power off", "Record…", "Capture the screen"]) {
      expect(screen.getByRole("button", { name })).toBeDisabled();
    }
  });

  it("pauses a running device and runs a paused one", async () => {
    const user = userEvent.setup();
    const onCommand = toolbar("on", status({ run: "running" }));
    await user.click(screen.getByRole("button", { name: "Pause" }));
    await user.click(screen.getByRole("button", { name: "Step" }));
    await user.click(screen.getByRole("button", { name: "Reset" }));
    expect(onCommand.mock.calls).toEqual([
      [{ kind: "pause" }],
      [{ kind: "step" }],
      [{ kind: "reset" }],
    ]);
  });

  it("does not step nor run a device that waits for an input", () => {
    toolbar("on", status({ run: "halted", halt: "idle" }));
    expect(screen.getByRole("button", { name: "Run" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Step" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Reset" })).toBeEnabled();
  });

  it("says that powering off writes the session of a recorded run", () => {
    toolbar("on", status({ recording: "run.inkrec" }));
    expect(screen.getByRole("button", { name: "Power off and write the session" })).toBeEnabled();
  });

  it("does not record a replay", () => {
    toolbar("on", status({ replay: "run.inkrec" }));
    expect(screen.getByRole("button", { name: "Record…" })).toBeDisabled();
  });
});
