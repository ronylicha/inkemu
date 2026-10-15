import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { App } from "./app";
import { fakeHost, status } from "./test/fake-host";
import { renderWith } from "./test/render";

describe("App", () => {
  it("powers a device on from the dialog with the files chosen, and shows what the host sends", async () => {
    const host = fakeHost();
    const user = userEvent.setup();
    renderWith(<App />, { host });
    await user.click(screen.getByRole("button", { name: "Power on…" }));
    const dialog = screen.getByRole("dialog", { name: "Power a device on" });
    expect(dialog).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByRole("option", { name: "Xteink X4" })).toBeInTheDocument(),
    );
    await user.click(screen.getAllByRole("button", { name: "Choose…" })[0] as HTMLElement);
    expect(screen.getByText("app.bin")).toBeInTheDocument();
    expect(screen.getByText(/The one inkemu ships: esp-idf/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Power on" }));
    expect(host.powerOn).toHaveBeenCalledWith(
      expect.objectContaining({
        device: "x4",
        firmware: "/home/user/fw/app.bin",
        image: null,
        usbHost: "open",
        record: null,
      }),
      "realTime",
      expect.any(Function),
    );
    expect(await screen.findByText(/inkemu: Xteink X4/)).toBeInTheDocument();
    act(() => {
      host.send({
        event: "console",
        data: { channel: "usb-serial-jtag", text: "ESP-ROM:esp32c3\r\n" },
      });
      host.send({ event: "status", data: status() });
    });
    expect(screen.getByRole("log", { name: "USB-Serial-JTAG" })).toHaveTextContent(
      "ESP-ROM:esp32c3",
    );
    expect(screen.getByRole("log", { name: "Journal" })).toHaveTextContent("inkemu: Xteink X4");
    expect(screen.getByText("0x40000400")).toBeInTheDocument();
    expect(screen.getByText("160 MHz")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Run: Running");
    await user.click(screen.getByRole("button", { name: "Pause" }));
    expect(host.command).toHaveBeenCalledWith({ kind: "pause" });
  });

  it("names a refused start in its own words and keeps the host's text as the detail", async () => {
    const host = fakeHost({
      powerOn: vi.fn(async () => {
        throw { kind: "rom", message: "cannot download https://…; check the network" };
      }),
    });
    const user = userEvent.setup();
    renderWith(<App />, { host });
    await user.click(screen.getByRole("button", { name: "Power on…" }));
    await user.click(screen.getAllByRole("button", { name: "Choose…" })[0] as HTMLElement);
    await user.click(screen.getByRole("button", { name: "Power on" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Espressif's mask ROM is not available.");
    expect(alert).toHaveTextContent("check the network");
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Power on…" })).toBeEnabled();
  });

  it("closes the dialog with Escape and gives the focus back", async () => {
    const user = userEvent.setup();
    renderWith(<App />, { language: "fr" });
    const open = screen.getByRole("button", { name: "Allumer…" });
    await user.click(open);
    expect(screen.getByRole("dialog", { name: "Allumer un appareil" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(open).toHaveFocus();
  });
});
