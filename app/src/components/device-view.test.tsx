import { fireEvent, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { LOOK, status } from "../test/fake-host";
import { renderWith } from "../test/render";
import { DeviceView } from "./device-view";

function device(onButton = vi.fn()) {
  renderWith(
    <DeviceView
      look={LOOK}
      deviceName="Xteink X4"
      signal={null}
      status={status()}
      onButton={onButton}
    />,
  );
  return onButton;
}

describe("DeviceView", () => {
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
