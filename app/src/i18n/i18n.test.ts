import { describe, expect, it } from "vitest";
import { en } from "./en";
import { fr } from "./fr";
import { detectLanguage, initialLanguage, interpolate, translator } from "./i18n";

const placeholders = (text: string) => [...text.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

describe("catalogs", () => {
  it("hold the same keys", () => {
    expect(Object.keys(en).sort()).toEqual(Object.keys(fr).sort());
  });

  it("give every key a text, with the same placeholders in both languages", () => {
    for (const key of Object.keys(fr) as (keyof typeof fr)[]) {
      expect(fr[key].trim(), key).not.toBe("");
      expect(en[key].trim(), key).not.toBe("");
      expect(placeholders(en[key]), key).toEqual(placeholders(fr[key]));
    }
  });
});

/**
 * Every kind of refusal the host and the shell give (`CommandError::kind`, `StartError::kind`,
 * `ErrorDto::new` in `app/src-tauri`): the banner shows the host's bare text for a kind it cannot
 * name.
 */
const ERROR_KINDS = [
  "button",
  "capture",
  "ended",
  "firmware",
  "flashFile",
  "flashImage",
  "frame",
  "input",
  "ipc",
  "maxCycles",
  "maxTime",
  "noImage",
  "noLife",
  "nothingToStep",
  "notKept",
  "notRecordable",
  "onItsThread",
  "powerOn",
  "profile",
  "recordPath",
  "replay",
  "replaying",
  "replayNotRecorded",
  "rom",
  "session",
  "stopped",
] as const;

describe("refusals", () => {
  it("are named in both languages for every kind the host gives", () => {
    for (const kind of ERROR_KINDS) {
      expect(Object.keys(fr), kind).toContain(`error.${kind}`);
      expect(Object.keys(en), kind).toContain(`error.${kind}`);
    }
  });
});

describe("language", () => {
  it("is the first of the system's that inkemu speaks", () => {
    expect(detectLanguage(["de-DE", "fr-CA", "en"])).toBe("fr");
    expect(detectLanguage(["en-GB", "fr"])).toBe("en");
    expect(detectLanguage(["ja"])).toBe("en");
    expect(detectLanguage([])).toBe("en");
  });

  it("is the kept one when the user chose", () => {
    expect(initialLanguage({ getItem: () => "fr" }, ["en"])).toBe("fr");
    expect(initialLanguage({ getItem: () => "xx" }, ["fr"])).toBe("fr");
    expect(initialLanguage(null, ["en-US"])).toBe("en");
  });
});

describe("interpolate", () => {
  it("replaces the named placeholders and leaves the unknown ones", () => {
    expect(interpolate("{a} and {b}", { a: 1, b: "two" })).toBe("1 and two");
    expect(interpolate("{a} {missing}", { a: "x" })).toBe("x {missing}");
  });

  it("translates through a catalog", () => {
    expect(translator(fr)("status.speedRatio", { ratio: "0,98" })).toBe("0,98 × le temps réel");
    expect(translator(en)("button.power")).toBe("Power");
  });
});
