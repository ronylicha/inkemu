import { screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWith } from "../test/render";
import { ErrorBanner } from "./error-banner";

const ended = {
  kind: "replayEnded",
  message:
    "the replay has ended, and a replay takes no input; reset it to replay the session from its start",
};

describe("ErrorBanner", () => {
  it("says a press during an ended replay is no input, not a run to power on again", () => {
    renderWith(<ErrorBanner error={ended} onDismiss={vi.fn()} />);
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent(
      "The replay has ended, and a replay takes no input: reset to replay the session from its start.",
    );
    expect(alert).not.toHaveTextContent("power the device on again");
  });

  const alias = {
    kind: "dataBusAlias",
    message:
      "0x3fc80000 is a data-bus alias: the CPU fetches its instructions through the instruction bus, so a breakpoint there would never stop; set it at 0x40380000, the same cell through the instruction bus",
  };

  it("says a breakpoint on a data-bus alias never stops, with the address that would", () => {
    renderWith(<ErrorBanner error={alias} onDismiss={vi.fn()} />);
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent(
      "This address is a data-bus alias: the CPU never executes there, so a breakpoint would never stop.",
    );
    expect(alert).toHaveTextContent("set it at 0x40380000");
  });

  it("says the data-bus alias in French", () => {
    renderWith(<ErrorBanner error={alias} onDismiss={vi.fn()} />, { language: "fr" });
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent(
      "Cette adresse est un alias du bus de données : le processeur n’y exécute jamais rien",
    );
    expect(alert).toHaveTextContent("0x40380000");
  });

  it("says it in French", () => {
    renderWith(<ErrorBanner error={ended} onDismiss={vi.fn()} />, { language: "fr" });
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Le rejeu est terminé, et un rejeu ne prend aucune entrée : faites un reset pour rejouer la session depuis son début.",
    );
  });
});
