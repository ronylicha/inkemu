import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { renderWith } from "../test/render";
import { Modal } from "./modal";

function modal(onClose = vi.fn()) {
  renderWith(
    <>
      <button type="button">Outside</button>
      <Modal title="Power on" onClose={onClose}>
        <button type="button">First</button>
        <button type="button">Last</button>
      </Modal>
    </>,
  );
  return onClose;
}

describe("Modal", () => {
  it("opens on its first control and keeps the focus inside, both ways", async () => {
    modal();
    const user = userEvent.setup();
    const first = screen.getByRole("button", { name: "First" });
    const last = screen.getByRole("button", { name: "Last" });
    expect(first).toHaveFocus();
    await user.tab();
    expect(last).toHaveFocus();
    await user.tab();
    expect(first).toHaveFocus();
    await user.tab({ shift: true });
    expect(last).toHaveFocus();
    expect(screen.getByRole("button", { name: "Outside" })).not.toHaveFocus();
  });

  it("closes on Escape", async () => {
    const onClose = modal();
    await userEvent.setup().keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
