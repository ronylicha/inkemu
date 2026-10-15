import { describe, expect, it } from "vitest";
import { asHostError, serialized } from "./host";

describe("asHostError", () => {
  it("keeps the address a refusal suggests, and nothing that is not one", () => {
    const refusal = { kind: "dataBusAlias", message: "detail", suggested: "0x40380100" };
    expect(asHostError(refusal)).toEqual(refusal);
    expect(asHostError({ kind: "point", message: "detail", suggested: 7 })).toEqual({
      kind: "point",
      message: "detail",
    });
  });
});

describe("serialized", () => {
  it("starts a call once the one before has settled: a release never overtakes its press", async () => {
    const order: string[] = [];
    let letPressEnd: () => void = () => undefined;
    const send = serialized(async (button: string) => {
      order.push(`give ${button}`);
      if (button === "press") {
        await new Promise<void>((resolve) => {
          letPressEnd = resolve;
        });
      }
      order.push(`done ${button}`);
    });
    const press = send("press");
    const release = send("release");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(order).toEqual(["give press"]);
    letPressEnd();
    await Promise.all([press, release]);
    expect(order).toEqual(["give press", "done press", "give release", "done release"]);
  });

  it("goes on after a refusal, which reaches its own caller only", async () => {
    const send = serialized(async (fails: boolean) => {
      if (fails) {
        throw new Error("refused");
      }
      return "ok";
    });
    const refused = send(true);
    const next = send(false);
    await expect(refused).rejects.toThrow("refused");
    await expect(next).resolves.toBe("ok");
  });
});
