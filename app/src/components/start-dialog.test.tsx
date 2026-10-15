import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { DeviceView } from "../ipc/types";
import { fakeHost, X4 } from "../test/fake-host";
import { renderWith } from "../test/render";
import { StartDialog, sdCardOf } from "./start-dialog";

describe("sdCardOf", () => {
  it("is an empty slot for no card", () => {
    expect(sdCardOf("none", "/x.img", true, "abc")).toBeNull();
  });

  it("waits for an image to be chosen", () => {
    expect(sdCardOf("existing", null, false, "1024")).toBeUndefined();
    expect(sdCardOf("new", null, false, "1024")).toBeUndefined();
  });

  it("opens an existing image as it is, write-protected if asked", () => {
    expect(sdCardOf("existing", "/c.img", true, "")).toEqual({
      path: "/c.img",
      readOnly: true,
      createMib: null,
    });
  });

  it("makes a new card of a whole number of MiB within the bounds of FAT32 and the SPI mode", () => {
    expect(sdCardOf("new", "/n.img", true, " 1024 ")).toEqual({
      path: "/n.img",
      readOnly: false,
      createMib: 1024,
    });
    expect(sdCardOf("new", "/n.img", false, "64")?.createMib).toBe(64);
    expect(sdCardOf("new", "/n.img", false, "2097152")?.createMib).toBe(2_097_152);
    for (const size of ["63", "2097153", "1.5", "", "1e3", "-64"]) {
      expect(sdCardOf("new", "/n.img", false, size), size).toBeUndefined();
    }
  });
});

function dialog(devices: DeviceView[] = [X4]) {
  const host = fakeHost({ pickSavePath: vi.fn(async () => "/home/user/card.img") });
  const onStart = vi.fn();
  renderWith(
    <StartDialog
      devices={devices}
      device={devices[0]?.id ?? "x4"}
      onDevice={vi.fn()}
      onStart={onStart}
      onClose={vi.fn()}
    />,
    { host },
  );
  return onStart;
}

describe("StartDialog", () => {
  it("starts with a USB cable and a new blank card of 1 GiB when asked", async () => {
    const onStart = dialog();
    const user = userEvent.setup();
    await user.click(screen.getByRole("radio", { name: "Erased flash" }));
    await user.click(screen.getByRole("checkbox", { name: /A USB cable is plugged in/ }));
    await user.click(screen.getByRole("radio", { name: "A new blank card" }));
    const submit = screen.getByRole("button", { name: "Power on" });
    // No image chosen yet: the form is not complete.
    expect(submit).toBeDisabled();
    const sd = screen.getByRole("group", { name: "SD card" });
    await user.click(
      [...sd.querySelectorAll("button")].find((b) => b.textContent === "Choose…") as HTMLElement,
    );
    expect(screen.getByText("card.img")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: /Size in MiB/ })).toHaveValue("1024");
    await user.click(submit);
    expect(onStart).toHaveBeenCalledWith(
      expect.objectContaining({
        usbPower: true,
        sd: { path: "/home/user/card.img", readOnly: false, createMib: 1024 },
      }),
    );
  });

  it("refuses a size out of bounds", async () => {
    dialog();
    const user = userEvent.setup();
    await user.click(screen.getByRole("radio", { name: "Erased flash" }));
    await user.click(screen.getByRole("radio", { name: "A new blank card" }));
    const sd = screen.getByRole("group", { name: "SD card" });
    await user.click(
      [...sd.querySelectorAll("button")].find((b) => b.textContent === "Choose…") as HTMLElement,
    );
    const size = screen.getByRole("textbox", { name: /Size in MiB/ });
    await user.clear(size);
    await user.type(size, "32");
    expect(size).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("button", { name: "Power on" })).toBeDisabled();
  });

  it("offers neither a cable nor a card to a device without them, and sends none", async () => {
    const onStart = dialog([
      { ...X4, id: "x3", name: "Xteink X3", sdSlot: false, usbPower: false },
    ]);
    const user = userEvent.setup();
    await user.click(screen.getByRole("radio", { name: "Erased flash" }));
    expect(screen.queryByRole("group", { name: "SD card" })).toBeNull();
    expect(screen.queryByRole("checkbox", { name: /USB cable/ })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Power on" }));
    expect(onStart).toHaveBeenCalledWith(expect.objectContaining({ usbPower: false, sd: null }));
  });
});
