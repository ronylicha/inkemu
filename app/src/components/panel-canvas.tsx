import { useEffect, useRef, useState } from "react";
import { useI18n } from "../i18n/i18n";
import { FrameError } from "../ipc/frame";
import { asHostError, useHost } from "../ipc/host";
import type { HostError, PanelDescription } from "../ipc/types";
import { type FlashStep, flashSteps } from "../panel/refresh";
import { fitScale, toPicture, turnedSize } from "../panel/render";

interface Props {
  panel: PanelDescription;
  /** The last refresh the host announced; a change fetches its image. */
  signal: { generation: number; life: number } | null;
  /** An image that could not be fetched or read: the panel keeps the last one. */
  onError(error: HostError): void;
}

function prefersReducedMotion(): boolean {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

/**
 * The screen of the device: the panel's pixels, turned as it is mounted, scaled by a whole
 * number so that nothing blurs, and the flash of a full refresh before its image.
 */
export function PanelCanvas({ panel, signal, onError }: Props) {
  const host = useHost();
  const { t } = useI18n();
  // The latest handler, read when a fetch fails: a new handler does not fetch the image again.
  const report = useRef(onError);
  report.current = onError;
  const room = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const size = turnedSize(panel.width, panel.height, panel.rotation);
  const [scale, setScale] = useState(1);
  const [flash, setFlash] = useState<FlashStep["show"] | null>(null);
  const [shown, setShown] = useState(false);

  const { width, height } = size;
  useEffect(() => {
    const element = room.current;
    if (element === null || typeof ResizeObserver === "undefined") {
      return;
    }
    const observer = new ResizeObserver(([entry]) => {
      if (entry !== undefined) {
        setScale(fitScale(entry.contentRect, { width, height }));
      }
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [width, height]);

  useEffect(() => {
    if (signal === null) {
      setShown(false);
      return;
    }
    let cancelled = false;
    const timers: number[] = [];
    host
      .frame()
      .then((frame) => {
        if (cancelled || frame === null) {
          return;
        }
        const draw = () => {
          setFlash(null);
          const context = canvas.current?.getContext("2d") ?? null;
          if (context !== null) {
            const picture = toPicture(frame, panel.rotation);
            context.putImageData(new ImageData(picture.data, picture.width, picture.height), 0, 0);
          }
          setShown(true);
        };
        let at = 0;
        for (const step of flashSteps(frame.refresh, frame.durationNanos, prefersReducedMotion())) {
          timers.push(window.setTimeout(() => setFlash(step.show), at));
          at += step.ms;
        }
        timers.push(window.setTimeout(draw, at));
      })
      .catch((reason: unknown) => {
        // The panel keeps the last image, and the next refresh asks again; the user is told.
        if (!cancelled) {
          report.current(
            reason instanceof FrameError
              ? { kind: "frame", message: reason.message }
              : asHostError(reason),
          );
        }
      });
    return () => {
      cancelled = true;
      for (const timer of timers) {
        window.clearTimeout(timer);
      }
    };
  }, [host, signal, panel.rotation]);

  return (
    <div className="panel-room" ref={room}>
      <div className="panel" style={{ width: size.width * scale, height: size.height * scale }}>
        <canvas
          ref={canvas}
          width={size.width}
          height={size.height}
          role="img"
          aria-label={shown ? t("device.panel") : t("device.noImage")}
        />
        {flash === null ? null : <div className={`panel-flash panel-flash-${flash}`} />}
      </div>
    </div>
  );
}
