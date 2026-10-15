import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Status } from "../ipc/types";
import { fakeHost, monitorView, status } from "../test/fake-host";
import { renderWith } from "../test/render";
import { MONITOR_EVERY_MS, MonitorPanel } from "./monitor-panel";

// `NBSP`: the space between a number and its unit, as an accessible name keeps it. The text
// matchers (`toHaveTextContent`, `getByText`) turn every white space into a plain one: they are
// given plain spaces.
const NBSP = " ";

function panel(current: Status | null, { language = "en" as "en" | "fr", host = fakeHost() } = {}) {
  const dispatch = vi.fn();
  const rendered = renderWith(<MonitorPanel start={3} status={current} dispatch={dispatch} />, {
    host,
    language,
  });
  return { host, dispatch, rendered };
}

const paused = () => status({ run: "paused" });

afterEach(() => {
  vi.useRealTimers();
});

describe("MonitorPanel", () => {
  it("asks nothing before a device is on", () => {
    const { host } = panel(null);
    expect(screen.getByText("Power a device on to follow its resources.")).toBeInTheDocument();
    expect(host.monitor).not.toHaveBeenCalled();
  });

  it("shows the CPU and where device time went, since power-on and in the last window", async () => {
    const { host } = panel(paused());
    const time = await screen.findByRole("table", { name: "Device time" });
    expect(host.monitor).toHaveBeenCalledTimes(1);
    expect(host.monitor).toHaveBeenCalledWith(3);
    const load = within(time).getByRole("row", { name: /Load/ });
    expect(load).toHaveTextContent("62.5%");
    expect(load).toHaveTextContent("80.0%");
    expect(
      within(time).getByRole("columnheader", { name: `Last window (250${NBSP}µs)` }),
    ).toBeInTheDocument();
    expect(within(time).getByRole("row", { name: /Deep sleep/ })).toHaveTextContent("200 µs");
    const clocks = screen.getByRole("table", { name: "Time per frequency" });
    expect(within(clocks).getByRole("rowheader", { name: "20 MHz" })).toBeInTheDocument();
    expect(within(clocks).getByRole("rowheader", { name: "160 MHz" })).toBeInTheDocument();
    expect(screen.getByText("×0.50")).toBeInTheDocument();
  });

  it("maps the pages of each RAM, with a text for those who do not see the map", async () => {
    panel(paused());
    const map = await screen.findByRole("img", {
      name: "SRAM: 2 pages written out of 4, 1 of them since the last reset",
    });
    expect(map.children).toHaveLength(4);
    expect(map.children[1]).toHaveClass("monitor-page-before");
    expect(map.children[3]).toHaveClass("monitor-page-since");
    expect(screen.getByText("0x3fc81000 to 0x3fc84000")).toBeInTheDocument();
    expect(screen.getByText("By page; the content is never shown.")).toBeInTheDocument();
    const key = screen.getByRole("list", { name: "Key to the page map" });
    expect(
      within(key)
        .getAllByRole("listitem")
        .map((item) => item.textContent?.trim()),
    ).toEqual(["never written", "written before the last reset", "written since the last reset"]);
  });

  it("names the interrupt sources and the blocks the CPU reached, and the panel's refreshes", async () => {
    panel(paused());
    const sources = await screen.findByRole("table", { name: "Interrupts raised by source" });
    const row = within(sources).getByRole("row", { name: /SYSTIMER_TARGET0_INT/ });
    expect(row).toHaveTextContent("37");
    expect(row).toHaveTextContent("12");
    const blocks = screen.getByRole("table", { name: "Register accesses of the CPU, by block" });
    expect(within(blocks).getByRole("rowheader", { name: "0x60023000" })).toBeInTheDocument();
    expect(within(blocks).getByText("SYSTIMER")).toBeInTheDocument();
    expect(screen.getByText("3 refreshes in this life, the last one partial.")).toBeInTheDocument();
  });

  it("says each current is the datasheet's, and the time it has no figure for", async () => {
    panel(paused());
    const power = await screen.findByRole("table", { name: "Estimated consumption" });
    expect(within(power).getByRole("row", { name: /Executing at 160 MHz/ })).toHaveTextContent(
      "23 mA to 28 mA",
    );
    expect(within(power).getByRole("row", { name: /Executing at 20 MHz/ })).toHaveTextContent(
      "no figure in the datasheet",
    );
    expect(within(power).getByRole("row", { name: /Deep sleep/ })).toHaveTextContent("5 µA");
    expect(screen.getByText(/ESP32-C3 Series Datasheet v2\.4/)).toBeInTheDocument();
    expect(
      screen.getByText("100 µs spent where the datasheet gives no figure are not counted."),
    ).toBeInTheDocument();
  });

  it("speaks French with French numbers", async () => {
    panel(paused(), { language: "fr" });
    const time = await screen.findByRole("table", { name: "Temps de l’appareil" });
    expect(within(time).getByRole("row", { name: /Charge/ })).toHaveTextContent("62,5 %");
    expect(screen.getByRole("heading", { name: "Moniteur de ressources" })).toBeInTheDocument();
    expect(
      screen.getByText("3 rafraîchissements dans cette vie, le dernier partiel."),
    ).toBeInTheDocument();
  });

  it("says there is no window yet, and an unnamed block", async () => {
    const host = fakeHost({
      monitor: vi.fn(async () =>
        monitorView({
          recent: null,
          panel: null,
          peripherals: [{ base: 0x6000_1000, peripherals: [], reads: 1, writes: 0, recent: null }],
        }),
      ),
    });
    panel(paused(), { host });
    const time = await screen.findByRole("table", { name: "Device time" });
    expect(within(time).getByRole("row", { name: /Load/ })).toHaveTextContent("no window yet");
    expect(screen.getByText("undocumented")).toBeInTheDocument();
    expect(screen.getByText("No refresh in this life.")).toBeInTheDocument();
  });

  it("reads every second while the device runs, and no more once closed", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { host, rendered } = panel(status({ run: "running" }));
    await screen.findByRole("table", { name: "Device time" });
    expect(screen.getByText(/Read again every second/)).toBeInTheDocument();
    await act(async () => {
      vi.advanceTimersByTime(3 * MONITOR_EVERY_MS);
    });
    await waitFor(() => expect(host.monitor).toHaveBeenCalledTimes(4));
    rendered.unmount();
    await act(async () => {
      vi.advanceTimersByTime(3 * MONITOR_EVERY_MS);
    });
    expect(host.monitor).toHaveBeenCalledTimes(4);
  });

  it("does not read on a timer while the device does not move", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { host } = panel(paused());
    await screen.findByRole("table", { name: "Device time" });
    await act(async () => {
      vi.advanceTimersByTime(3 * MONITOR_EVERY_MS);
    });
    expect(host.monitor).toHaveBeenCalledTimes(1);
  });

  it("reports a refusal of the host", async () => {
    const host = fakeHost({
      monitor: vi.fn(async () => Promise.reject({ kind: "stopped", message: "stopped" })),
    });
    const { dispatch } = panel(paused(), { host });
    await waitFor(() =>
      expect(dispatch).toHaveBeenCalledWith({
        type: "failed",
        error: { kind: "stopped", message: "stopped" },
      }),
    );
  });
});
