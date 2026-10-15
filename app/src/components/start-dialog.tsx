import { type FormEvent, useState } from "react";
import { useI18n } from "../i18n/i18n";
import { type FileKind, type SaveKind, useHost } from "../ipc/host";
import { type DeviceView, type Setup, USB_HOSTS, type UsbHost } from "../ipc/types";
import { labelOf } from "./labels";
import { Modal } from "./modal";

type Source = "erased" | "image" | "firmware";

interface Props {
  devices: readonly DeviceView[];
  device: string;
  onDevice(device: string): void;
  onStart(setup: Setup): void;
  onClose(): void;
}

/** A path picked with the system's dialog, shown by its file name, and removable. */
function PathField(props: {
  label: string;
  path: string | null;
  pick(): Promise<string | null>;
  onPath(path: string | null): void;
}) {
  const { t } = useI18n();
  const name = props.path === null ? t("start.none") : props.path.split(/[/\\]/).at(-1);
  return (
    <div className="path-field">
      <span>{props.label}</span>
      <output title={props.path ?? undefined}>{name}</output>
      <button
        type="button"
        onClick={async () => {
          const picked = await props.pick();
          if (picked !== null) {
            props.onPath(picked);
          }
        }}
      >
        {t("start.choose")}
      </button>
      {props.path === null ? null : (
        <button type="button" onClick={() => props.onPath(null)}>
          {t("start.clear")}
        </button>
      )}
    </div>
  );
}

/** What `inkemu run` takes, as a form: the model, the flash, its file, the USB host, a session. */
export function StartDialog({ devices, device, onDevice, onStart, onClose }: Props) {
  const { t } = useI18n();
  const host = useHost();
  const [source, setSource] = useState<Source>("firmware");
  const [image, setImage] = useState<string | null>(null);
  const [firmware, setFirmware] = useState<string | null>(null);
  const [bootloader, setBootloader] = useState<string | null>(null);
  const [flashFile, setFlashFile] = useState<string | null>(null);
  const [resetFlash, setResetFlash] = useState(false);
  const [usbHost, setUsbHost] = useState<UsbHost>("open");
  const [record, setRecord] = useState<string | null>(null);
  const [maxTime, setMaxTime] = useState("");
  const [maxCycles, setMaxCycles] = useState("");
  const [strictMmio, setStrictMmio] = useState(false);
  const [romDir, setRomDir] = useState<string | null>(null);
  const chosen = devices.find((one) => one.id === device);
  const pickFile = (kind: FileKind) => () => host.pickFile(kind);
  const pickSave = (kind: SaveKind) => () => host.pickSavePath(kind);

  const ready =
    source === "erased" ||
    (source === "image" && image !== null) ||
    (source === "firmware" && firmware !== null);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    onStart({
      device,
      image: source === "image" ? image : null,
      firmware: source === "firmware" ? firmware : null,
      bootloader: source === "firmware" ? bootloader : null,
      flashFile,
      resetFlash: flashFile !== null && resetFlash,
      romDir,
      usbHost,
      strictMmio,
      record,
      maxTime: maxTime.trim(),
      maxCycles: maxCycles.trim(),
      console: null,
    });
  };

  return (
    <Modal title={t("start.title")} onClose={onClose}>
      <form onSubmit={submit} className="start-form">
        <label>
          {t("start.device")}{" "}
          <select value={device} onChange={(event) => onDevice(event.target.value)}>
            {devices.map((one) => (
              <option key={one.id} value={one.id}>
                {one.name}
              </option>
            ))}
          </select>
        </label>
        <fieldset>
          <legend>{t("start.source")}</legend>
          {(["erased", "image", "firmware"] as const).map((one) => (
            <label key={one}>
              <input
                type="radio"
                name="source"
                value={one}
                checked={source === one}
                onChange={() => setSource(one)}
              />{" "}
              {labelOf(t, "start.source", one)}
            </label>
          ))}
          {source === "image" ? (
            <PathField
              label={t("start.image")}
              path={image}
              pick={pickFile("flash")}
              onPath={setImage}
            />
          ) : null}
          {source === "firmware" ? (
            <>
              <PathField
                label={t("start.firmware")}
                path={firmware}
                pick={pickFile("firmware")}
                onPath={setFirmware}
              />
              <PathField
                label={t("start.bootloader")}
                path={bootloader}
                pick={pickFile("bootloader")}
                onPath={setBootloader}
              />
              {bootloader === null && chosen !== undefined ? (
                <p className="hint">
                  {chosen.shippedBootloader === null
                    ? t("start.bootloader.none", { reason: chosen.noBootloader ?? "" })
                    : t("start.bootloader.shipped", { id: chosen.shippedBootloader })}
                </p>
              ) : null}
            </>
          ) : null}
        </fieldset>
        <PathField
          label={t("start.flashFile")}
          path={flashFile}
          pick={pickSave("flashFile")}
          onPath={setFlashFile}
        />
        {flashFile === null ? null : (
          <label>
            <input
              type="checkbox"
              checked={resetFlash}
              onChange={(event) => setResetFlash(event.target.checked)}
            />{" "}
            {t("start.resetFlash")}
          </label>
        )}
        <label>
          {t("start.usbHost")}{" "}
          <select value={usbHost} onChange={(event) => setUsbHost(event.target.value as UsbHost)}>
            {USB_HOSTS.map((one) => (
              <option key={one} value={one}>
                {labelOf(t, "usbHost", one)}
              </option>
            ))}
          </select>
        </label>
        <PathField
          label={t("start.record")}
          path={record}
          pick={pickSave("session")}
          onPath={setRecord}
        />
        <details>
          <summary>{t("start.advanced")}</summary>
          <label>
            {t("start.maxTime")}{" "}
            <input
              type="text"
              inputMode="decimal"
              value={maxTime}
              onChange={(event) => setMaxTime(event.target.value)}
            />
          </label>
          <label>
            {t("start.maxCycles")}{" "}
            <input
              type="text"
              inputMode="numeric"
              value={maxCycles}
              onChange={(event) => setMaxCycles(event.target.value)}
            />
          </label>
          <label>
            <input
              type="checkbox"
              checked={strictMmio}
              onChange={(event) => setStrictMmio(event.target.checked)}
            />{" "}
            {t("start.strictMmio")}
          </label>
          <PathField
            label={t("start.romDir")}
            path={romDir}
            pick={pickFile("romDir")}
            onPath={setRomDir}
          />
        </details>
        <div className="actions">
          <button type="button" onClick={onClose}>
            {t("start.cancel")}
          </button>
          <button type="submit" disabled={!ready}>
            {t("start.submit")}
          </button>
        </div>
      </form>
    </Modal>
  );
}
