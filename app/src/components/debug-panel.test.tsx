import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Command, Status } from "../ipc/types";
import { debugView, fakeHost, status } from "../test/fake-host";
import { renderWith } from "../test/render";
import { DebugPanel } from "./debug-panel";

const paused = (overrides: Partial<Status> = {}) =>
  status({ run: "paused", stop: { kind: "breakpoint", pc: 0x4000_0004 }, ...overrides });

function panel(current: Status | null, { language = "en" as "en" | "fr", host = fakeHost() } = {}) {
  const onCommand = vi.fn(async (_command: Command) => undefined);
  const dispatch = vi.fn();
  renderWith(<DebugPanel start={3} status={current} onCommand={onCommand} dispatch={dispatch} />, {
    host,
    language,
  });
  return { host, onCommand, dispatch };
}

describe("DebugPanel", () => {
  it("asks the host for the code around the PC and a page of the SRAM, and keeps the points", async () => {
    const { host, dispatch } = panel(paused());
    await screen.findByRole("table", { name: "Registers" });
    expect(host.inspect).toHaveBeenCalledWith(
      { disassemblyAt: null, disassemblyLines: 24, memory: { address: 0x3fc8_0000, length: 256 } },
      3,
    );
    expect(dispatch).toHaveBeenCalledWith({
      type: "points",
      start: 3,
      points: debugView().points,
    });
  });

  it("shows the CPU, its registers by name, its CSRs, and why it stopped", async () => {
    panel(paused());
    const registers = await screen.findByRole("table", { name: "Registers" });
    expect(within(registers).getByRole("rowheader", { name: "x2 sp" })).toBeInTheDocument();
    expect(within(registers).getByText("0x3fcdfff0")).toBeInTheDocument();
    const csrs = screen.getByRole("table", { name: "Control and status registers (CSRs)" });
    expect(within(csrs).getByRole("rowheader", { name: "mstatus" })).toBeInTheDocument();
    expect(screen.getByText("machine")).toBeInTheDocument();
    expect(
      screen.getByText("Stopped at the breakpoint 0x40000004, before its instruction."),
    ).toBeInTheDocument();
    // The window's announcement of the run says it: the panel adds no second live region.
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("marks the line of the PC and the breakpoints, and says what cannot be read", async () => {
    panel(paused());
    const code = await screen.findByRole("list", { name: "Code" });
    const lines = within(code).getAllByRole("listitem");
    expect(lines[1]).toHaveAttribute("aria-current", "true");
    expect(lines[1]).toHaveTextContent("addi t1, t1, 1");
    expect(lines[1]).toHaveTextContent("compressed instruction (16 bits)");
    expect(within(code).getByRole("button", { name: "Breakpoint at 0x40000004" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(within(code).getByRole("button", { name: "Breakpoint at 0x40000000" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    expect(lines[2]).toHaveTextContent("no code can be read here");
    expect(within(code).getByRole("button", { name: "Breakpoint at 0x40000006" })).toBeDisabled();
  });

  it("sets and clears a breakpoint from the code, then reads the machine again", async () => {
    const user = userEvent.setup();
    const { host, onCommand } = panel(paused());
    const code = await screen.findByRole("list", { name: "Code" });
    await user.click(within(code).getByRole("button", { name: "Breakpoint at 0x40000004" }));
    await user.click(within(code).getByRole("button", { name: "Breakpoint at 0x40000000" }));
    expect(onCommand.mock.calls).toEqual([
      [{ kind: "breakpoint", address: 0x4000_0004, set: false }],
      [{ kind: "breakpoint", address: 0x4000_0000, set: true }],
    ]);
    await waitFor(() => expect(host.inspect).toHaveBeenCalledTimes(3));
  });

  it("shows the memory 16 bytes to a row, and why some bytes are not shown", async () => {
    panel(paused());
    const memory = await screen.findByRole("table", { name: "Memory" });
    const row = within(memory).getByRole("row", { name: /0x3fc80000/ });
    expect(row).toHaveTextContent("2a");
    expect(row).toHaveTextContent("··");
    expect(row).toHaveTextContent("*.A");
    expect(
      screen.getByText(
        "0x3fc80004 to 0x3fc80005: content of the flash, not shown (it may hold personal data)",
      ),
    ).toBeInTheDocument();
  });

  it("reads an address the user types and refuses what is no address", async () => {
    const user = userEvent.setup();
    const { host, onCommand } = panel(paused());
    await screen.findByRole("table", { name: "Registers" });
    const field = screen.getByRole("textbox", { name: "Breakpoint at the address" });
    await user.type(field, "zz{Enter}");
    expect(field).toHaveAttribute("aria-invalid", "true");
    expect(field).toHaveAccessibleDescription(
      "An address in hexadecimal, 1 to 8 digits, such as 0x3fc80000.",
    );
    expect(onCommand).not.toHaveBeenCalled();
    await user.clear(field);
    await user.type(field, "0x42000020{Enter}");
    expect(field).toHaveAttribute("aria-invalid", "false");
    expect(onCommand).toHaveBeenCalledWith({ kind: "breakpoint", address: 0x4200_0020, set: true });

    const memoryAt = screen.getByRole("textbox", { name: "Memory at the address" });
    await user.clear(memoryAt);
    await user.type(memoryAt, "50000000{Enter}");
    await waitFor(() =>
      expect(host.inspect).toHaveBeenLastCalledWith(
        expect.objectContaining({ memory: { address: 0x5000_0000, length: 256 } }),
        3,
      ),
    );
    const codeAt = screen.getByRole("textbox", { name: "Code at the address" });
    await user.type(codeAt, "0x40000100{Enter}");
    await waitFor(() =>
      expect(host.inspect).toHaveBeenLastCalledWith(
        expect.objectContaining({ disassemblyAt: 0x4000_0100 }),
        3,
      ),
    );
    await user.click(screen.getByRole("button", { name: "Follow the PC" }));
    await waitFor(() =>
      expect(host.inspect).toHaveBeenLastCalledWith(
        expect.objectContaining({ disassemblyAt: null }),
        3,
      ),
    );
  });

  it("sets a watchpoint and lists the points with their remove buttons", async () => {
    const user = userEvent.setup();
    const { onCommand } = panel(paused());
    await screen.findByRole("table", { name: "Registers" });
    await user.type(screen.getByRole("textbox", { name: "Watch from the address" }), "3fc80010");
    const length = screen.getByRole("textbox", { name: "Bytes" });
    await user.clear(length);
    await user.type(length, "5000");
    await user.selectOptions(screen.getByRole("combobox", { name: "Stop on" }), "access");
    await user.click(screen.getByRole("button", { name: "Watch" }));
    expect(length).toHaveAttribute("aria-invalid", "true");
    expect(onCommand).not.toHaveBeenCalled();
    await user.clear(length);
    await user.type(length, "8");
    await user.click(screen.getByRole("button", { name: "Watch" }));
    expect(onCommand).toHaveBeenLastCalledWith({
      kind: "watchpoint",
      address: 0x3fc8_0010,
      length: 8,
      on: "access",
      set: true,
    });
    await user.click(
      screen.getByRole("button", { name: "Remove the watchpoint: 4 bytes at 0x3fc80000, writes" }),
    );
    expect(onCommand).toHaveBeenLastCalledWith({
      kind: "watchpoint",
      address: 0x3fc8_0000,
      length: 4,
      on: "write",
      set: false,
    });
    await user.click(screen.getByRole("button", { name: "Remove the breakpoint at 0x40000004" }));
    expect(onCommand).toHaveBeenLastCalledWith({
      kind: "breakpoint",
      address: 0x4000_0004,
      set: false,
    });
  });

  it("reads a running device only when asked, and says the view may be old", async () => {
    const user = userEvent.setup();
    const { host } = panel(status({ run: "running" }));
    expect(screen.getByText(/The device runs/)).toBeInTheDocument();
    expect(host.inspect).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Refresh" }));
    await screen.findByRole("table", { name: "Registers" });
    expect(host.inspect).toHaveBeenCalledTimes(1);
  });

  it("hands a refusal of the host to the banner", async () => {
    const user = userEvent.setup();
    const refusal = { kind: "point", message: "a breakpoint goes on an even address" };
    const { onCommand, dispatch } = panel(paused());
    onCommand.mockRejectedValueOnce(refusal);
    const code = await screen.findByRole("list", { name: "Code" });
    await user.click(within(code).getByRole("button", { name: "Breakpoint at 0x40000000" }));
    await waitFor(() => expect(dispatch).toHaveBeenCalledWith({ type: "failed", error: refusal }));
  });

  it("asks to power a device on when none runs", () => {
    const { host } = panel(null);
    expect(screen.getByText("Power a device on to debug it.")).toBeInTheDocument();
    expect(host.inspect).not.toHaveBeenCalled();
  });

  it("speaks French", async () => {
    panel(
      paused({
        stop: {
          kind: "watchpoint",
          address: 0x3fc8_0000,
          size: 4,
          access: "write",
          value: 0x2a,
          pc: 0x4000_0008,
        },
      }),
      { language: "fr" },
    );
    await screen.findByRole("table", { name: "Registres" });
    expect(
      screen.getByText(
        "Arrêté : l’instruction à 0x40000008 a écrit 4 octets à 0x3fc80000 (valeur 0x2a).",
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Points de surveillance" })).toBeInTheDocument();
  });
});
