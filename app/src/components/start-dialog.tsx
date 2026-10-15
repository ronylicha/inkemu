import { type FormEvent, useState } from "react";
import { useI18n } from "../i18n/i18n";
import { asHostError, type FileKind, type SaveKind, useHost } from "../ipc/host";
import {
  type DeviceView,
  type SdCard,
  type SdSlotView,
  type Setup,
  USB_HOSTS,
  type UsbHost,
} from "../ipc/types";
import { labelOf } from "./labels";
import { Modal } from "./modal";

type Source = "erased" | "image" | "firmware";

/** The card of the form: none, an image that exists, or a blank one made first. */
export type SdKind = "none" | "existing" | "new";
const SD_KINDS: readonly SdKind[] = ["none", "existing", "new"];

/** Whether `text` is a whole number of MiB within the sizes the host makes a new card at. */
function isCardSize(text: string, slot: SdSlotView): boolean {
  const mib = Number(text.trim());
  return /^\d+$/.test(text.trim()) && mib >= slot.minMib && mib <= slot.maxMib;
}

/**
 * The card the form describes: `null` for an empty slot, `undefined` while the form misses
 * something (no image chosen, a size that is not a whole number of MiB within the bounds the host
 * gives, `slot`).
 */
export function sdCardOf(
  kind: SdKind,
  path: string | null,
  readOnly: boolean,
  sizeMib: string,
  slot: SdSlotView,
): SdCard | null | undefined {
  if (kind === "none") {
    return null;
  }
  if (path === null) {
    return undefined;
  }
  if (kind === "existing") {
    return { path, readOnly, createMib: null };
  }
  if (!isCardSize(sizeMib, slot)) {
    return undefined;
  }
  // A card made blank for this run is written: write-protecting it would keep it empty.
  return { path, readOnly: false, createMib: Number(sizeMib.trim()) };
}

/** What the form still misses before the device can start, said under the button (H9). */
export type Missing =
  | { key: "start.missing.image" }
  | { key: "start.missing.firmware" }
  | { key: "start.missing.sdImage" }
  | { key: "start.missing.sdSize"; min: number; max: number };

/** The first thing the form misses, `null` when it is complete. */
export function missingOf(form: {
  source: Source;
  image: string | null;
  firmware: string | null;
  sdKind: SdKind;
  sdPath: string | null;
  sdSizeMib: string;
  slot: SdSlotView | null;
}): Missing | null {
  if (form.source === "image" && form.image === null) {
    return { key: "start.missing.image" };
  }
  if (form.source === "firmware" && form.firmware === null) {
    return { key: "start.missing.firmware" };
  }
  if (form.slot === null || form.sdKind === "none") {
    return null;
  }
  if (form.sdPath === null) {
    return { key: "start.missing.sdImage" };
  }
  if (form.sdKind === "new" && !isCardSize(form.sdSizeMib, form.slot)) {
    return { key: "start.missing.sdSize", min: form.slot.minMib, max: form.slot.maxMib };
  }
  return null;
}

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
  const [usbPower, setUsbPower] = useState(false);
  const [sdKind, setSdKind] = useState<SdKind>("none");
  const [sdPath, setSdPath] = useState<string | null>(null);
  const [sdReadOnly, setSdReadOnly] = useState(false);
  // `null` until the user types one: the size the host offers first.
  const [sdSizeTyped, setSdSizeTyped] = useState<string | null>(null);
  const [pickError, setPickError] = useState<string | null>(null);
  const chosen = devices.find((one) => one.id === device);
  const slot = chosen?.sdSlot ?? null;
  const sdSizeMib = sdSizeTyped ?? String(slot?.defaultMib ?? "");
  const sd = slot === null ? null : sdCardOf(sdKind, sdPath, sdReadOnly, sdSizeMib, slot);
  /** A path from the system's dialog; a dialog that fails is said in the form, and picks nothing. */
  const picked = async (pick: Promise<string | null>) => {
    try {
      const path = await pick;
      setPickError(null);
      return path;
    } catch (reason) {
      setPickError(t("start.pickFailed", { detail: asHostError(reason).message }));
      return null;
    }
  };
  const pickFile = (kind: FileKind) => () => picked(host.pickFile(kind));
  const pickSave = (kind: SaveKind) => () => picked(host.pickSavePath(kind));

  const missing = missingOf({ source, image, firmware, sdKind, sdPath, sdSizeMib, slot });
  const ready = missing === null;
  // The size field is described by what is missing only when the size is what is missing (R6).
  const sizeMissing = missing?.key === "start.missing.sdSize";
  const missingText =
    missing === null
      ? null
      : missing.key === "start.missing.sdSize"
        ? t(missing.key, { min: missing.min, max: missing.max })
        : t(missing.key);

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
      usbPower: (chosen?.usbPower ?? false) && usbPower,
      sd: sd ?? null,
    });
  };

  return (
    <Modal title={t("start.title")} onClose={onClose}>
      <form onSubmit={submit} className="start-form">
        {pickError === null ? null : (
          <p className="error" role="alert">
            {pickError}
          </p>
        )}
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
        {chosen?.usbPower ? (
          <label>
            <input
              type="checkbox"
              checked={usbPower}
              onChange={(event) => setUsbPower(event.target.checked)}
            />{" "}
            {t("start.usbPower")}
          </label>
        ) : null}
        {slot !== null ? (
          <fieldset>
            <legend>{t("start.sd")}</legend>
            {SD_KINDS.map((one) => (
              <label key={one}>
                <input
                  type="radio"
                  name="sd"
                  value={one}
                  checked={sdKind === one}
                  onChange={() => {
                    setSdKind(one);
                    // An image picked to open is not a path to create, nor the reverse.
                    setSdPath(null);
                  }}
                />{" "}
                {labelOf(t, "start.sd", one)}
              </label>
            ))}
            {sdKind === "existing" ? (
              <>
                <PathField
                  label={t("start.sd.image")}
                  path={sdPath}
                  pick={pickFile("sdImage")}
                  onPath={setSdPath}
                />
                <label>
                  <input
                    type="checkbox"
                    checked={sdReadOnly}
                    onChange={(event) => setSdReadOnly(event.target.checked)}
                  />{" "}
                  {t("start.sd.readOnly")}
                </label>
              </>
            ) : null}
            {sdKind === "new" ? (
              <>
                <PathField
                  label={t("start.sd.newImage")}
                  path={sdPath}
                  pick={pickSave("sdImage")}
                  onPath={setSdPath}
                />
                <label>
                  {t("start.sd.size", { min: slot.minMib, max: slot.maxMib })}{" "}
                  <input
                    type="text"
                    inputMode="numeric"
                    value={sdSizeMib}
                    aria-invalid={sizeMissing}
                    aria-describedby={sizeMissing ? "start-missing" : undefined}
                    onChange={(event) => setSdSizeTyped(event.target.value)}
                  />
                </label>
              </>
            ) : null}
          </fieldset>
        ) : null}
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
          <button
            type="submit"
            disabled={!ready}
            aria-describedby={missingText === null ? undefined : "start-missing"}
          >
            {t("start.submit")}
          </button>
        </div>
        {missingText === null ? null : (
          <p id="start-missing" className="hint" role="status">
            {missingText}
          </p>
        )}
      </form>
    </Modal>
  );
}
