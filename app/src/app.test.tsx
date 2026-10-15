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
      1,
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
    // The updates are drawn at the next frame of the screen.
    await waitFor(() =>
      expect(screen.getByRole("log", { name: "USB-Serial-JTAG" })).toHaveTextContent(
        "ESP-ROM:esp32c3",
      ),
    );
    expect(screen.getByRole("log", { name: "Journal" })).toHaveTextContent("inkemu: Xteink X4");
    expect(screen.getByText("0x40000400")).toBeInTheDocument();
    expect(screen.getByText("160 MHz")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Run: Running");
    await user.click(screen.getByRole("button", { name: "Pause" }));
    // To the emulator of the first start.
    expect(host.command).toHaveBeenCalledWith({ kind: "pause" }, 1);
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

  it("tells the shell its language at once and at every change, for the dialog before closing", async () => {
    const host = fakeHost();
    const user = userEvent.setup();
    renderWith(<App />, { host, language: "fr" });
    await waitFor(() => expect(host.setLanguage).toHaveBeenCalledWith("fr"));
    await user.selectOptions(screen.getByRole("combobox", { name: "Langue" }), "en");
    await waitFor(() => expect(host.setLanguage).toHaveBeenLastCalledWith("en"));
  });

  /** Powers the fake device on from the dialog, its first file chosen. */
  async function powerOn(user: ReturnType<typeof userEvent.setup>) {
    await user.click(screen.getByRole("button", { name: "Power on…" }));
    await user.click(screen.getAllByRole("button", { name: "Choose…" })[0] as HTMLElement);
    await user.click(screen.getByRole("button", { name: "Power on" }));
  }

  it("keeps the device that runs when a second start is refused", async () => {
    const host = fakeHost();
    const user = userEvent.setup();
    renderWith(<App />, { host });
    await powerOn(user);
    await screen.findByText(/inkemu: Xteink X4/);
    act(() => {
      host.send({ event: "console", data: { channel: "usb-serial-jtag", text: "first" } });
      host.send({ event: "status", data: status() });
    });
    await waitFor(() =>
      expect(screen.getByRole("log", { name: "USB-Serial-JTAG" })).toHaveTextContent("first"),
    );
    vi.mocked(host.powerOn).mockRejectedValueOnce({ kind: "firmware", message: "not an app" });
    await powerOn(user);
    expect(await screen.findByRole("alert")).toHaveTextContent("not an app");
    act(() => {
      host.send({ event: "console", data: { channel: "usb-serial-jtag", text: " still" } });
    });
    await waitFor(() =>
      expect(screen.getByRole("log", { name: "USB-Serial-JTAG" })).toHaveTextContent("first still"),
    );
    expect(screen.getByRole("button", { name: "Pause" })).toBeEnabled();
    // The device that runs is still the first start's: commands go to it.
    await user.click(screen.getByRole("button", { name: "Pause" }));
    expect(host.command).toHaveBeenLastCalledWith({ kind: "pause" }, 1);
  });

  it("sends commands to the emulator of the last start, which the shell checks", async () => {
    const host = fakeHost();
    const user = userEvent.setup();
    renderWith(<App />, { host });
    await powerOn(user);
    await screen.findByText(/inkemu: Xteink X4/);
    await powerOn(user);
    await waitFor(() => expect(host.powerOn).toHaveBeenCalledTimes(2));
    expect(vi.mocked(host.powerOn).mock.calls[1]?.[2]).toBe(2);
    act(() => {
      host.send({ event: "status", data: status() });
    });
    const pause = await screen.findByRole("button", { name: "Pause" });
    await user.click(pause);
    expect(host.command).toHaveBeenLastCalledWith({ kind: "pause" }, 2);
  });

  it("says in the journal where a capture went, and why one was refused", async () => {
    const host = fakeHost();
    const user = userEvent.setup();
    renderWith(<App />, { host });
    await powerOn(user);
    await screen.findByText(/inkemu: Xteink X4/);
    act(() => {
      host.send({ event: "frame", data: { generation: 1, refresh: "full", life: 1 } });
      host.send({ event: "status", data: status() });
    });
    const capture = await screen.findByRole("button", { name: "Capture the screen" });
    await waitFor(() => expect(capture).toBeEnabled());
    await user.click(capture);
    expect(host.saveCapture).toHaveBeenCalledWith("/home/user/out.png", 1);
    // The image is asked of the emulator that announced it.
    expect(host.frame).toHaveBeenCalledWith(1);
    await waitFor(() =>
      expect(screen.getByRole("log", { name: "Journal" })).toHaveTextContent(
        "Capture saved: out.png",
      ),
    );
    vi.mocked(host.saveCapture).mockRejectedValueOnce({
      kind: "capture",
      message: "the capture cannot be written to /home/user/out.png: it is the flash file",
    });
    await user.click(capture);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The capture could not be written.");
    expect(alert).toHaveTextContent("it is the flash file");
  });
});
