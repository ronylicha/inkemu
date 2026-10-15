import { fireEvent, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Look, Status } from "../ipc/types";
import { LOOK, status, WIRED_LOOK } from "../test/fake-host";
import { renderWith } from "../test/render";
import { DeviceView } from "./device-view";

function device(onButton = vi.fn(), look: Look = LOOK, shown: Status = status()) {
  renderWith(
    <DeviceView
      look={look}
      deviceName="Xteink X4"
      signal={null}
      status={shown}
      onButton={onButton}
      onFrameError={vi.fn()}
    />,
  );
  return onButton;
}

describe("DeviceView of a wired device", () => {
  it("says its rendering is approximate, and shows no test-pattern banner", () => {
    device(vi.fn(), WIRED_LOOK);
    expect(
      screen.getByText("Approximate rendering: optical parameters not measured"),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Test pattern/)).toBeNull();
  });

  it("says nothing of an approximation for a test pattern", () => {
    device();
    expect(screen.queryByText(/Approximate rendering/)).toBeNull();
    expect(screen.getByText(/Test pattern/)).toBeInTheDocument();
  });

  it("shows the device off once its board cut the supply, and its power key still presses", () => {
    const onButton = device(
      vi.fn(),
      WIRED_LOOK,
      status({ run: "ended", end: "powerOff", machine: null }),
    );
    // The gesture that turns it on is a hold, not a click (2d-D14).
    expect(screen.getByText("Hold the power button to turn it on")).toBeInTheDocument();
    const power = screen.getByRole("button", { name: "Power" });
    fireEvent.pointerDown(power);
    fireEvent.pointerUp(power);
    expect(onButton.mock.calls).toEqual([
      ["power", true],
      ["power", false],
    ]);
  });
});

describe("DeviceView of a replay", () => {
  it("shows a device the session turned off as off, without asking for a hold it cannot take", () => {
    const off = status({ run: "ended", end: "powerOff", machine: null, replay: "life.inkrec" });
    device(vi.fn(), WIRED_LOOK, off);
    expect(
      screen.getByText("Off, as in the session: a replay cannot turn the device on again"),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Hold the power button/)).toBeNull();
  });

  it("says it in French", () => {
    renderWith(
      <DeviceView
        look={WIRED_LOOK}
        deviceName="Xteink X4"
        signal={null}
        status={status({ run: "ended", end: "powerOff", machine: null, replay: "life.inkrec" })}
        onButton={vi.fn()}
        onFrameError={vi.fn()}
      />,
      { language: "fr" },
    );
    expect(
      screen.getByText("Éteint, comme dans la session : un rejeu ne peut pas rallumer l’appareil"),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Maintenez/)).toBeNull();
  });
});

describe("DeviceView", () => {
  it("releases a key held on the device when the window loses the focus", () => {
    const onButton = device();
    const area = screen.getByRole("application", { name: "Device Xteink X4" });
    fireEvent.keyDown(area, { key: "ArrowUp" });
    fireEvent.blur(window);
    expect(onButton.mock.calls).toEqual([
      ["up", true],
      ["up", false],
    ]);
    // The key comes up elsewhere: nothing more is sent.
    fireEvent.keyUp(area, { key: "ArrowUp" });
    expect(onButton).toHaveBeenCalledTimes(2);
  });

  it("releases a key held on the device when the focus leaves it, not when it goes to a button", () => {
    const onButton = device();
    const area = screen.getByRole("application", { name: "Device Xteink X4" });
    const power = screen.getByRole("button", { name: "Power" });
    fireEvent.keyDown(area, { key: "Enter" });
    fireEvent.blur(area, { relatedTarget: power });
    expect(onButton.mock.calls).toEqual([["confirm", true]]);
    fireEvent.blur(area, { relatedTarget: document.body });
    expect(onButton.mock.calls).toEqual([
      ["confirm", true],
      ["confirm", false],
    ]);
  });

  it("presses a button on pointer down and releases it on pointer up", () => {
    const onButton = device();
    const power = screen.getByRole("button", { name: "Power" });
    fireEvent.pointerDown(power);
    expect(power).toHaveAttribute("aria-pressed", "true");
    fireEvent.pointerUp(power);
    expect(onButton.mock.calls).toEqual([
      ["power", true],
      ["power", false],
    ]);
    expect(power).toHaveAttribute("aria-pressed", "false");
  });

  it("presses a focused button with Space, and releases it on key up", async () => {
    const onButton = device();
    const user = userEvent.setup();
    await user.tab();
    await user.tab();
    expect(screen.getByRole("button", { name: "Power" })).toHaveFocus();
    await user.keyboard(" ");
    expect(onButton.mock.calls).toEqual([
      ["power", true],
      ["power", false],
    ]);
  });

  it("takes the arrows and Enter while the device has the focus", () => {
    const onButton = device();
    const area = screen.getByRole("application", { name: "Device Xteink X4" });
    fireEvent.keyDown(area, { key: "ArrowUp" });
    fireEvent.keyUp(area, { key: "ArrowUp" });
    fireEvent.keyDown(area, { key: "Enter" });
    fireEvent.keyDown(area, { key: "Enter", repeat: true });
    fireEvent.keyUp(area, { key: "Enter" });
    fireEvent.keyDown(area, { key: "ArrowLeft" });
    expect(onButton.mock.calls).toEqual([
      ["up", true],
      ["up", false],
      ["confirm", true],
      ["confirm", false],
    ]);
  });

  it("says that the image is a test pattern, and how the keyboard drives the device", () => {
    device();
    expect(screen.getByRole("note")).toHaveTextContent(/Test pattern.*Xteink X4/);
    expect(
      screen.getByRole("application", { name: "Device Xteink X4" }),
    ).toHaveAccessibleDescription(/arrows, Enter/);
    expect(screen.getByRole("img", { name: "No image yet" })).toBeInTheDocument();
  });
});
