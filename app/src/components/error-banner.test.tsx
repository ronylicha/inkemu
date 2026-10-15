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

  it("says it in French", () => {
    renderWith(<ErrorBanner error={ended} onDismiss={vi.fn()} />, { language: "fr" });
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Le rejeu est terminé, et un rejeu ne prend aucune entrée : faites un reset pour rejouer la session depuis son début.",
    );
  });
});
