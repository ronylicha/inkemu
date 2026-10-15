import { type KeyboardEvent, type ReactNode, useEffect, useId, useRef } from "react";

interface Props {
  title: string;
  onClose(): void;
  children: ReactNode;
}

const FOCUSABLE = [
  "a[href]",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "button:not([disabled])",
  "summary",
  "[tabindex]:not([tabindex='-1'])",
].join(", ");

/** What Tab can reach in `box`, in the order of the document. */
function focusables(box: HTMLElement): HTMLElement[] {
  return [...box.querySelectorAll<HTMLElement>(FOCUSABLE)];
}

/**
 * A modal dialog: labelled by its title, focused when it opens, closed by Escape, the focus kept
 * inside while it is open (Tab past its last control comes back to its first, Shift+Tab the other
 * way), and given back to where it was when it closes.
 */
export function Modal({ title, onClose, children }: Props) {
  const titleId = useId();
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const first = box.current === null ? undefined : focusables(box.current)[0];
    (first ?? box.current)?.focus();
    return () => before?.focus();
  }, []);

  const keepFocus = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      onClose();
      return;
    }
    if (event.key !== "Tab") {
      return;
    }
    const items = focusables(event.currentTarget);
    const first = items[0];
    const last = items.at(-1);
    if (first === undefined || last === undefined) {
      event.preventDefault();
      event.currentTarget.focus();
      return;
    }
    const active = document.activeElement;
    if (event.shiftKey && (active === first || active === event.currentTarget)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && active === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <div className="modal-backdrop">
      <div
        ref={box}
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={keepFocus}
      >
        <h2 id={titleId}>{title}</h2>
        {children}
      </div>
    </div>
  );
}
