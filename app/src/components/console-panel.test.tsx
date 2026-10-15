import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { renderWith } from "../test/render";
import { ConsolePanel } from "./console-panel";

function panel({ silenced = false, takesInput = true } = {}) {
  const onSend = vi.fn();
  const onUsbHost = vi.fn();
  renderWith(
    <ConsolePanel
      first="usb-serial-jtag"
      consoles={{ uart0: "boot\n", uart1: "", "usb-serial-jtag": "ESP-ROM:esp32c3\n" }}
      takesInput={takesInput}
      usbHost="open"
      usbSilenced={silenced}
      onSend={onSend}
      onUsbHost={onUsbHost}
      onClear={vi.fn()}
    />,
  );
  return { onSend, onUsbHost };
}

describe("ConsolePanel", () => {
  it("shows the model's console first", () => {
    panel();
    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((tab) => tab.textContent)).toEqual([
      "USB-Serial-JTAG (the model's console)",
      "UART0",
      "UART1",
    ]);
    expect(tabs[0]).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("log")).toHaveTextContent("ESP-ROM:esp32c3");
  });

  it("moves between consoles with the arrows", async () => {
    panel();
    const user = userEvent.setup();
    await user.click(screen.getAllByRole("tab")[0] as HTMLElement);
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "UART0" })).toHaveFocus();
    expect(screen.getByRole("log")).toHaveTextContent("boot");
  });

  it("sends a typed line with the chosen line ending", async () => {
    const { onSend } = panel();
    const user = userEvent.setup();
    await user.type(screen.getByPlaceholderText("Text to send"), "ls{Enter}");
    expect(onSend).toHaveBeenCalledWith("usb-serial-jtag", [108, 115, 13, 10]);
    await user.selectOptions(screen.getByRole("combobox", { name: "Line ending" }), "lf");
    await user.type(screen.getByPlaceholderText("Text to send"), "x{Enter}");
    expect(onSend).toHaveBeenLastCalledWith("usb-serial-jtag", [120, 10]);
  });

  it("changes the USB host", async () => {
    const { onUsbHost } = panel();
    const user = userEvent.setup();
    await user.selectOptions(screen.getByRole("combobox", { name: "USB host" }), "absent");
    expect(onUsbHost).toHaveBeenCalledWith("absent");
  });

  it("says when nothing reaches the USB-Serial-JTAG, and takes nothing without a run", () => {
    panel({ silenced: true, takesInput: false });
    expect(screen.getByText(/has not opened the port/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  });
});
