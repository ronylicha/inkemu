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

  // The host's detail names another address than the one it suggests: what the banner's own
  // sentence says can only come from the structured field.
  const alias = {
    kind: "dataBusAlias",
    message: "0x3fc80100 is a data-bus alias; the host's detail",
    suggested: "0x40380100",
  };

  it("says a breakpoint on a data-bus alias never stops, with the address that would", () => {
    renderWith(<ErrorBanner error={alias} onDismiss={vi.fn()} />);
    expect(screen.getByRole("alert")).toHaveTextContent(
      "This address is a data-bus alias: the CPU never executes there, so a breakpoint would never stop. Set it at 0x40380100, the same cell through the instruction bus.",
    );
  });

  it("says the data-bus alias and the address that would stop in French", () => {
    renderWith(<ErrorBanner error={alias} onDismiss={vi.fn()} />, { language: "fr" });
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Cette adresse est un alias du bus de données : le processeur n’y exécute jamais rien, un point d’arrêt n’y arrêterait pas. Posez-le à 0x40380100, la même cellule vue par le bus d’instructions.",
    );
  });

  it("says it in French", () => {
    renderWith(<ErrorBanner error={ended} onDismiss={vi.fn()} />, { language: "fr" });
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Le rejeu est terminé, et un rejeu ne prend aucune entrée : faites un reset pour rejouer la session depuis son début.",
    );
  });
});
