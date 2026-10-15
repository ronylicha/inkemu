import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import { AboutDialog } from "./components/about-dialog";
import { ConsolePanel } from "./components/console-panel";
import { DeviceView } from "./components/device-view";
import { ErrorBanner } from "./components/error-banner";
import { JournalPanel } from "./components/journal-panel";
import { labelOf } from "./components/labels";
import { StartDialog } from "./components/start-dialog";
import { StatusPanel } from "./components/status-panel";
import { Toolbar } from "./components/toolbar";
import { useI18n } from "./i18n/i18n";
import { asHostError, useHost } from "./ipc/host";
import type { Command, Setup, Speed, Started, Update } from "./ipc/types";
import { updateBatcher } from "./state/batch";
import { initialState, reducer } from "./state/reducer";
import { useTheme } from "./use-theme";

/** The name of the file at `path`, as the journal shows it. */
function fileName(path: string): string {
  return path.split(/[/\\]/).at(-1) ?? path;
}

/**
 * The window: the controls, the device, its status, its consoles and its journal. It keeps what
 * the host sends and calls the host's commands; the host decides everything about the device.
 */
export function App() {
  const host = useHost();
  const { t, language } = useI18n();
  const [state, dispatch] = useReducer(reducer, undefined, initialState);
  const [theme, setTheme] = useTheme();
  const [speed, setSpeed] = useState<Speed>("realTime");
  const [asking, setAsking] = useState(false);
  const [about, setAbout] = useState(false);
  // Every start is numbered: the updates of an emulator carry its number (reducer.ts).
  const starts = useRef(0);
  const batcher = useMemo(
    () => updateBatcher((start, updates) => dispatch({ type: "updates", start, updates })),
    [],
  );
  useEffect(() => () => batcher.cancel(), [batcher]);

  // The shell speaks the interface's language in its own dialogs (the one before closing): it is
  // told at once and at every change.
  useEffect(() => {
    host
      .setLanguage(language)
      .catch((reason: unknown) => dispatch({ type: "failed", error: asHostError(reason) }));
  }, [host, language]);

  useEffect(() => {
    host
      .devices()
      .then((devices) => dispatch({ type: "devices", devices }))
      .catch((reason: unknown) => dispatch({ type: "failed", error: asHostError(reason) }));
  }, [host]);

  const failed = (reason: unknown) => dispatch({ type: "failed", error: asHostError(reason) });
  // Commands go to the emulator shown, by the number of its start: one queued behind a new start
  // is refused rather than reaching the new emulator.
  const command = (next: Command) => {
    host.command(next, state.current).catch(failed);
  };

  /** Starts what `begin` starts, numbered, its updates gathered per frame. */
  const start = (
    begin: (number: number, onUpdate: (update: Update) => void) => Promise<Started>,
  ) => {
    starts.current += 1;
    const number = starts.current;
    dispatch({ type: "starting", start: number });
    begin(number, (update) => batcher.push(number, update))
      .then((started) => {
        batcher.flush();
        dispatch({ type: "started", start: number, started });
      })
      .catch((reason: unknown) =>
        dispatch({ type: "failed", start: number, error: asHostError(reason) }),
      );
  };

  const powerOn = (setup: Setup) => {
    setAsking(false);
    start((number, onUpdate) => host.powerOn(setup, speed, number, onUpdate));
  };

  const replay = async () => {
    try {
      const path = await host.pickFile("session");
      if (path !== null) {
        start((number, onUpdate) => host.openReplay(path, null, speed, number, onUpdate));
      }
    } catch (reason) {
      failed(reason);
    }
  };

  const record = async () => {
    try {
      const path = await host.pickSavePath("session");
      if (path === null) {
        return;
      }
      await host.command({ kind: "record", path }, state.current);
      dispatch({
        type: "said",
        notice: { level: "info", text: t("said.recording", { name: fileName(path) }) },
      });
    } catch (reason) {
      failed(reason);
    }
  };

  const capture = async () => {
    try {
      const path = await host.pickSavePath("capture");
      if (path === null) {
        return;
      }
      await host.saveCapture(path, state.current);
      dispatch({
        type: "said",
        notice: { level: "info", text: t("said.captured", { name: fileName(path) }) },
      });
    } catch (reason) {
      failed(reason);
    }
  };

  const changeSpeed = (next: Speed) => {
    setSpeed(next);
    if (state.phase === "on") {
      command({ kind: "speed", speed: next });
    }
  };

  const status = state.status;
  const takesInput = state.phase === "on" && status !== null && status.replay === null;
  const announcement =
    status === null ? "" : t("announce.run", { state: labelOf(t, "status.run", status.run) });

  return (
    <div className="app">
      <a className="skip" href="#device">
        {t("app.skip")}
      </a>
      <header>
        <h1 className="sr-only">{t("app.title")}</h1>
        <Toolbar
          phase={state.phase}
          starting={state.starting !== null}
          status={status}
          speed={status?.speed ?? speed}
          hasImage={state.frame !== null}
          theme={theme}
          onPowerOn={() => setAsking(true)}
          onCommand={command}
          onSpeed={changeSpeed}
          onRecord={record}
          onReplay={replay}
          onCapture={capture}
          onAbout={() => setAbout(true)}
          onTheme={setTheme}
        />
      </header>
      {state.error === null ? null : (
        <ErrorBanner error={state.error} onDismiss={() => dispatch({ type: "dismissError" })} />
      )}
      <main>
        <div id="device" className="device-area">
          {state.look === null ? (
            <p className="hint">{t("status.off")}</p>
          ) : (
            <DeviceView
              look={state.look}
              deviceName={state.deviceName}
              signal={state.frame}
              status={status}
              onButton={(id, pressed) => command({ kind: "button", id, pressed })}
              onFrameError={failed}
            />
          )}
        </div>
        <StatusPanel status={status} lastEnd={state.lastEnd} />
      </main>
      <footer className="bottom">
        <ConsolePanel
          first={state.console}
          consoles={state.consoles}
          takesInput={takesInput}
          usbHost={status?.machine?.usbHost ?? null}
          usbSilenced={status?.usbConsoleSilenced ?? false}
          onSend={(channel, bytes) => command({ kind: "console", channel, bytes })}
          onUsbHost={(host) => command({ kind: "usbHost", host })}
          onClear={(channel) => dispatch({ type: "clearConsole", channel })}
        />
        <JournalPanel entries={state.journal} />
      </footer>
      <div className="sr-only" role="status" aria-live="polite">
        {announcement}
      </div>
      <div className="sr-only" aria-live="polite">
        {state.told}
      </div>
      {asking ? (
        <StartDialog
          devices={state.devices}
          device={state.device}
          onDevice={(device) => dispatch({ type: "selectDevice", device })}
          onStart={powerOn}
          onClose={() => setAsking(false)}
        />
      ) : null}
      {about ? <AboutDialog onClose={() => setAbout(false)} /> : null}
    </div>
  );
}
