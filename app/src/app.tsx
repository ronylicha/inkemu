import { useEffect, useReducer, useState } from "react";
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
import type { Command, Setup, Speed, Update } from "./ipc/types";
import { initialState, reducer } from "./state/reducer";
import { useTheme } from "./use-theme";

/**
 * The window: the controls, the device, its status, its consoles and its journal. It keeps what
 * the host sends and calls the host's commands; the host decides everything about the device.
 */
export function App() {
  const host = useHost();
  const { t } = useI18n();
  const [state, dispatch] = useReducer(reducer, undefined, initialState);
  const [theme, setTheme] = useTheme();
  const [speed, setSpeed] = useState<Speed>("realTime");
  const [starting, setStarting] = useState(false);
  const [about, setAbout] = useState(false);

  useEffect(() => {
    host
      .devices()
      .then((devices) => dispatch({ type: "devices", devices }))
      .catch((reason: unknown) => dispatch({ type: "failed", error: asHostError(reason) }));
  }, [host]);

  const failed = (reason: unknown) => dispatch({ type: "failed", error: asHostError(reason) });
  const onUpdate = (update: Update) => dispatch({ type: "update", update });
  const command = (next: Command) => {
    host.command(next).catch(failed);
  };

  const powerOn = (setup: Setup) => {
    setStarting(false);
    dispatch({ type: "starting" });
    host
      .powerOn(setup, speed, onUpdate)
      .then((started) => dispatch({ type: "started", started }))
      .catch(failed);
  };

  const replay = async () => {
    const path = await host.pickFile("session");
    if (path === null) {
      return;
    }
    dispatch({ type: "starting" });
    host
      .openReplay(path, null, speed, onUpdate)
      .then((started) => dispatch({ type: "started", started }))
      .catch(failed);
  };

  const record = async () => {
    const path = await host.pickSavePath("session");
    if (path !== null) {
      command({ kind: "record", path });
    }
  };

  const capture = async () => {
    const path = await host.pickSavePath("capture");
    if (path !== null) {
      host.saveCapture(path).catch(failed);
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
          status={status}
          speed={status?.speed ?? speed}
          hasImage={state.frame !== null}
          theme={theme}
          onPowerOn={() => setStarting(true)}
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
      {starting ? (
        <StartDialog
          devices={state.devices}
          device={state.device}
          onDevice={(device) => dispatch({ type: "selectDevice", device })}
          onStart={powerOn}
          onClose={() => setStarting(false)}
        />
      ) : null}
      {about ? <AboutDialog onClose={() => setAbout(false)} /> : null}
    </div>
  );
}
