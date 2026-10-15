import { type FormEvent, type KeyboardEvent, useEffect, useRef, useState } from "react";
import { useI18n } from "../i18n/i18n";
import { CONSOLE_CHANNELS, type ConsoleChannel, USB_HOSTS, type UsbHost } from "../ipc/types";
import { LINE_ENDINGS, type LineEnding, lineBytes } from "../state/console";
import { labelOf } from "./labels";

interface Props {
  /** The console of the model, shown first. */
  first: ConsoleChannel;
  consoles: Record<ConsoleChannel, string>;
  /** Whether a run takes what is typed (not off, not a replay). */
  takesInput: boolean;
  usbHost: UsbHost | null;
  usbSilenced: boolean;
  onSend(channel: ConsoleChannel, bytes: number[]): void;
  onUsbHost(host: UsbHost): void;
  onClear(channel: ConsoleChannel): void;
}

/** The three consoles of the chip, the model's first, each with what it said and a line to type. */
export function ConsolePanel(props: Props) {
  const { first, consoles, takesInput, usbHost, usbSilenced, onSend, onUsbHost, onClear } = props;
  const { t } = useI18n();
  const order = [first, ...CONSOLE_CHANNELS.filter((channel) => channel !== first)];
  const [selected, setSelected] = useState<ConsoleChannel>(first);
  const [line, setLine] = useState("");
  const [ending, setEnding] = useState<LineEnding>("crlf");
  const output = useRef<HTMLPreElement>(null);
  const tabs = useRef<Partial<Record<ConsoleChannel, HTMLButtonElement | null>>>({});

  useEffect(() => {
    setSelected(first);
  }, [first]);

  const text = consoles[selected];
  useEffect(() => {
    const element = output.current;
    if (element !== null && text.length > 0) {
      element.scrollTop = element.scrollHeight;
    }
  }, [text]);

  const select = (channel: ConsoleChannel) => {
    setSelected(channel);
    tabs.current[channel]?.focus();
  };

  const tabKey = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const step = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
    if (step !== 0) {
      event.preventDefault();
      const next = order[(index + step + order.length) % order.length];
      if (next !== undefined) {
        select(next);
      }
    }
  };

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    onSend(selected, lineBytes(line, ending));
    setLine("");
  };

  const name = labelOf(t, "console", selected);
  return (
    <section className="consoles" aria-label={t("console.label")}>
      <div role="tablist" aria-label={t("console.label")} className="tabs">
        {order.map((channel, index) => (
          <button
            key={channel}
            ref={(element) => {
              tabs.current[channel] = element;
            }}
            type="button"
            role="tab"
            id={`tab-${channel}`}
            aria-selected={channel === selected}
            aria-controls={`panel-${channel}`}
            tabIndex={channel === selected ? 0 : -1}
            onClick={() => select(channel)}
            onKeyDown={(event) => tabKey(event, index)}
          >
            {channel === first
              ? t("console.profile", { channel: labelOf(t, "console", channel) })
              : labelOf(t, "console", channel)}
          </button>
        ))}
      </div>
      <div role="tabpanel" id={`panel-${selected}`} aria-labelledby={`tab-${selected}`}>
        {selected === "usb-serial-jtag" && usbSilenced ? (
          <p className="warning">{t("console.silenced")}</p>
        ) : null}
        <pre
          ref={output}
          className="console-output"
          role="log"
          aria-live="off"
          aria-label={name}
          // biome-ignore lint/a11y/noNoninteractiveTabindex: a scrollable region must take the focus to scroll by keyboard (WCAG 2.1.1).
          tabIndex={0}
        >
          {text.length === 0 ? t("console.empty") : text}
        </pre>
        <form className="console-input" onSubmit={submit}>
          <label>
            <span className="sr-only">{t("console.input", { channel: name })}</span>
            <input
              type="text"
              value={line}
              placeholder={t("console.placeholder")}
              disabled={!takesInput}
              onChange={(event) => setLine(event.target.value)}
            />
          </label>
          <label>
            {t("console.lineEnding")}{" "}
            <select
              value={ending}
              onChange={(event) => setEnding(event.target.value as LineEnding)}
            >
              {LINE_ENDINGS.map((one) => (
                <option key={one} value={one}>
                  {labelOf(t, "console.lineEnding", one)}
                </option>
              ))}
            </select>
          </label>
          <button type="submit" disabled={!takesInput}>
            {t("console.send")}
          </button>
          <button type="button" onClick={() => onClear(selected)}>
            {t("console.clear")}
          </button>
          <label>
            {t("status.usbHost")}{" "}
            <select
              value={usbHost ?? "open"}
              disabled={!takesInput || usbHost === null}
              onChange={(event) => onUsbHost(event.target.value as UsbHost)}
            >
              {USB_HOSTS.map((host) => (
                <option key={host} value={host}>
                  {labelOf(t, "usbHost", host)}
                </option>
              ))}
            </select>
          </label>
        </form>
      </div>
    </section>
  );
}
