import { type KeyboardEvent, useState } from "react";
import { useI18n } from "../i18n/i18n";
import type { ButtonSpec, Edge, Look, Status } from "../ipc/types";
import { buttonForKey } from "../keyboard";
import { labelOf } from "./labels";
import { PanelCanvas } from "./panel-canvas";

interface Props {
  look: Look;
  deviceName: string;
  signal: { generation: number; life: number } | null;
  status: Status | null;
  /** Presses (`true`) or releases a button. */
  onButton(id: string, pressed: boolean): void;
}

const EDGES: readonly Edge[] = ["top", "left", "right", "bottom"];

function onEdge(buttons: readonly ButtonSpec[], edge: Edge): ButtonSpec[] {
  return buttons.filter((button) => button.edge === edge).toSorted((a, b) => a.order - b.order);
}

/**
 * The device: its screen, with its buttons around it where they sit on the device. A button
 * presses on pointer down and releases on pointer up, or with Space and Enter while it has the
 * focus; the device itself takes the arrows, Enter, Escape, Backspace and P (keyboard.ts).
 */
export function DeviceView({ look, deviceName, signal, status, onButton }: Props) {
  const { t } = useI18n();
  const [down, setDown] = useState<ReadonlySet<string>>(new Set());
  const ids = look.buttons.map((button) => button.id);

  const set = (id: string, pressed: boolean) => {
    if (down.has(id) === pressed) {
      return;
    }
    const next = new Set(down);
    if (pressed) {
      next.add(id);
    } else {
      next.delete(id);
    }
    setDown(next);
    onButton(id, pressed);
  };

  const deviceKey = (event: KeyboardEvent<HTMLElement>, pressed: boolean) => {
    if (event.target !== event.currentTarget || event.repeat) {
      return;
    }
    const id = buttonForKey(event.key, ids);
    if (id !== null) {
      event.preventDefault();
      set(id, pressed);
    }
  };

  const buttonKey = (event: KeyboardEvent<HTMLButtonElement>, id: string, pressed: boolean) => {
    if ((event.key === " " || event.key === "Enter") && !event.repeat) {
      event.preventDefault();
      set(id, pressed);
    }
  };

  const halt = status?.halt ?? null;
  const asleep = status?.machine?.deepSleep != null;

  return (
    <div className="device-wrap">
      {look.panel.origin === "testPattern" ? (
        <p className="banner" role="note">
          {t("device.testPattern", { name: deviceName })}
        </p>
      ) : null}
      <div
        className="device"
        role="application"
        aria-label={t("device.label", { name: deviceName })}
        aria-describedby="device-keys"
        // biome-ignore lint/a11y/noNoninteractiveTabindex: the device takes its own keys while it has the focus (keyboard.ts), as an application does.
        tabIndex={0}
        onKeyDown={(event) => deviceKey(event, true)}
        onKeyUp={(event) => deviceKey(event, false)}
      >
        {EDGES.map((edge) => (
          <div key={edge} className={`device-edge device-edge-${edge}`}>
            {onEdge(look.buttons, edge).map((button) => (
              <button
                key={button.id}
                type="button"
                className="device-button"
                aria-pressed={down.has(button.id)}
                onPointerDown={() => set(button.id, true)}
                onPointerUp={() => set(button.id, false)}
                onPointerLeave={() => set(button.id, false)}
                onKeyDown={(event) => buttonKey(event, button.id, true)}
                onKeyUp={(event) => buttonKey(event, button.id, false)}
                onBlur={() => set(button.id, false)}
              >
                {labelOf(t, "button", button.id)}
              </button>
            ))}
          </div>
        ))}
        <div className="device-screen">
          <PanelCanvas panel={look.panel} signal={signal} />
          {asleep ? (
            <p className="device-badge">{t("device.asleep")}</p>
          ) : halt !== null ? (
            <p className="device-badge">{t("device.waiting")}</p>
          ) : null}
        </div>
      </div>
      <p id="device-keys" className="hint">
        {t("device.keys")}
      </p>
    </div>
  );
}
