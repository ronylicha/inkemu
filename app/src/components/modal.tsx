import { type ReactNode, useEffect, useId, useRef } from "react";

interface Props {
  title: string;
  onClose(): void;
  children: ReactNode;
}

/**
 * A modal dialog: labelled by its title, focused when it opens, closed by Escape, and the focus
 * given back to where it was when it closes.
 */
export function Modal({ title, onClose, children }: Props) {
  const titleId = useId();
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const first = box.current?.querySelector<HTMLElement>(
      "input, select, textarea, button, [tabindex]:not([tabindex='-1'])",
    );
    (first ?? box.current)?.focus();
    return () => before?.focus();
  }, []);
  return (
    <div className="modal-backdrop">
      <div
        ref={box}
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.stopPropagation();
            onClose();
          }
        }}
      >
        <h2 id={titleId}>{title}</h2>
        {children}
      </div>
    </div>
  );
}
