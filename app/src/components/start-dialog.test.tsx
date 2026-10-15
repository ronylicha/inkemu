import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { DeviceView, SdSlotView } from "../ipc/types";
import { fakeHost, X4 } from "../test/fake-host";
import { renderWith } from "../test/render";
import { missingOf, StartDialog, sdCardOf } from "./start-dialog";

/** Bounds other than the host's real ones: the dialog must use what it is given. */
const SLOT: SdSlotView = { minMib: 100, maxMib: 5000, defaultMib: 512 };

describe("sdCardOf", () => {
  it("is an empty slot for no card", () => {
    expect(sdCardOf("none", "/x.img", true, "abc", SLOT)).toBeNull();
  });

  it("waits for an image to be chosen", () => {
    expect(sdCardOf("existing", null, false, "1024", SLOT)).toBeUndefined();
    expect(sdCardOf("new", null, false, "1024", SLOT)).toBeUndefined();
  });

  it("opens an existing image as it is, write-protected if asked", () => {
    expect(sdCardOf("existing", "/c.img", true, "", SLOT)).toEqual({
      path: "/c.img",
      readOnly: true,
      createMib: null,
    });
  });

  it("makes a new card of a whole number of MiB within the bounds the host gives", () => {
    expect(sdCardOf("new", "/n.img", true, " 1024 ", SLOT)).toEqual({
      path: "/n.img",
      readOnly: false,
      createMib: 1024,
    });
    expect(sdCardOf("new", "/n.img", false, "100", SLOT)?.createMib).toBe(100);
    expect(sdCardOf("new", "/n.img", false, "5000", SLOT)?.createMib).toBe(5000);
    for (const size of ["99", "5001", "1.5", "", "1e3", "-64"]) {
      expect(sdCardOf("new", "/n.img", false, size, SLOT), size).toBeUndefined();
    }
  });
});

describe("missingOf", () => {
  const form = {
    source: "erased" as const,
    image: null,
    firmware: null,
    sdKind: "none" as const,
    sdPath: null,
    sdSizeMib: "512",
    slot: SLOT,
  };

  it("names the first thing the form misses, nothing when it is complete", () => {
    expect(missingOf(form)).toBeNull();
    expect(missingOf({ ...form, source: "image" })).toEqual({ key: "start.missing.image" });
    expect(missingOf({ ...form, source: "firmware" })).toEqual({ key: "start.missing.firmware" });
    expect(missingOf({ ...form, sdKind: "existing" })).toEqual({ key: "start.missing.sdImage" });
    expect(missingOf({ ...form, sdKind: "new", sdPath: "/n.img", sdSizeMib: "50" })).toEqual({
      key: "start.missing.sdSize",
      min: 100,
      max: 5000,
    });
    // A device without a slot misses no card.
    expect(missingOf({ ...form, sdKind: "existing", slot: null })).toBeNull();
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

/** Clicks "Choose…" in the SD card section. */
async function chooseCard(user: ReturnType<typeof userEvent.setup>) {
  const sd = screen.getByRole("group", { name: "SD card" });
  await user.click(
    [...sd.querySelectorAll("button")].find((b) => b.textContent === "Choose…") as HTMLElement,
  );
}

describe("StartDialog", () => {
  it("starts with a USB cable and a new blank card of the size the host offers", async () => {
    const onStart = dialog();
    const user = userEvent.setup();
    await user.click(screen.getByRole("radio", { name: "Erased flash" }));
    await user.click(screen.getByRole("checkbox", { name: /A USB cable is plugged in/ }));
    await user.click(screen.getByRole("radio", { name: "A new blank card" }));
    const submit = screen.getByRole("button", { name: "Power on" });
    // No image chosen yet: the form is not complete, and says what it misses.
    expect(submit).toBeDisabled();
    expect(submit).toHaveAccessibleDescription("Choose the image of the SD card.");
    await chooseCard(user);
    expect(screen.getByText("card.img")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: /Size in MiB/ })).toHaveValue("1024");
    expect(submit).not.toHaveAttribute("aria-describedby");
    await user.click(submit);
    expect(onStart).toHaveBeenCalledWith(
      expect.objectContaining({
        usbPower: true,
        sd: { path: "/home/user/card.img", readOnly: false, createMib: 1024 },
      }),
    );
  });

  it("takes the bounds of a new card from the device, and says them when a size is out", async () => {
    dialog([{ ...X4, sdSlot: SLOT }]);
    const user = userEvent.setup();
    await user.click(screen.getByRole("radio", { name: "Erased flash" }));
    await user.click(screen.getByRole("radio", { name: "A new blank card" }));
    await chooseCard(user);
    const size = screen.getByRole("textbox", { name: "Size in MiB (100 to 5000)" });
    expect(size).toHaveValue("512");
    await user.clear(size);
    await user.type(size, "99");
    expect(size).toHaveAttribute("aria-invalid", "true");
    const said = "The size of a new card is a whole number of MiB, from 100 to 5000.";
    expect(size).toHaveAccessibleDescription(said);
    const submit = screen.getByRole("button", { name: "Power on" });
    expect(submit).toBeDisabled();
    expect(submit).toHaveAccessibleDescription(said);
  });

  it("describes the size field by what is missing only when the size is", async () => {
    dialog();
    const user = userEvent.setup();
    await user.click(screen.getByRole("radio", { name: "A new blank card" }));
    await chooseCard(user);
    const size = screen.getByRole("textbox", { name: /Size in MiB/ });
    // The application is what is missing, not the size.
    expect(screen.getByRole("button", { name: "Power on" })).toHaveAccessibleDescription(
      "Choose an application to start the device.",
    );
    expect(size).not.toHaveAttribute("aria-describedby");
    expect(size).toHaveAttribute("aria-invalid", "false");
  });

  it("says in French what the form misses", async () => {
    const host = fakeHost();
    renderWith(
      <StartDialog
        devices={[X4]}
        device="x4"
        onDevice={vi.fn()}
        onStart={vi.fn()}
        onClose={vi.fn()}
      />,
      { host, language: "fr" },
    );
    expect(screen.getByRole("button", { name: "Allumer" })).toHaveAccessibleDescription(
      "Choisissez une application pour démarrer l’appareil.",
    );
  });

  it("offers neither a cable nor a card to a device without them, and sends none", async () => {
    const onStart = dialog([{ ...X4, id: "x3", name: "Xteink X3", sdSlot: null, usbPower: false }]);
    const user = userEvent.setup();
    await user.click(screen.getByRole("radio", { name: "Erased flash" }));
    expect(screen.queryByRole("group", { name: "SD card" })).toBeNull();
    expect(screen.queryByRole("checkbox", { name: /USB cable/ })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Power on" }));
    expect(onStart).toHaveBeenCalledWith(expect.objectContaining({ usbPower: false, sd: null }));
  });
});
