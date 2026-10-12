import { onCleanup, onMount, type JSX } from "solid-js";
import { Portal } from "solid-js/web";

/** Open dialogs, innermost last: only that one keeps the focus. */
const open: HTMLElement[] = [];

const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** Dialog with backdrop; Escape closes it wherever the focus is, Tab stays inside (aria-modal). */
export function Modal(props: { label: string; onClose: () => void; class?: string; children: JSX.Element }) {
  let dialog: HTMLDivElement | undefined;
  let previous: Element | null = null;
  const trapTab = (e: KeyboardEvent) => {
    if (!dialog || open[open.length - 1] !== dialog) return;
    const items = [...dialog.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => el.getClientRects().length > 0);
    const active = document.activeElement;
    if (items.length === 0) {
      e.preventDefault();
      dialog.focus();
      return;
    }
    const first = items[0]!;
    const last = items[items.length - 1]!;
    const inside = dialog.contains(active);
    // Only at the edges: inside, Tab keeps its normal order (and QuickCapture its completion).
    if (e.shiftKey && (!inside || active === first || active === dialog)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && (!inside || active === last)) {
      e.preventDefault();
      first.focus();
    }
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Tab") {
      trapTab(e);
      return;
    }
    if (e.key !== "Escape") return;
    e.preventDefault();
    e.stopPropagation();
    props.onClose();
  };
  onMount(() => {
    if (dialog) open.push(dialog);
    previous = document.activeElement;
    window.addEventListener("keydown", onKey, { capture: true });
    const auto = dialog?.querySelector<HTMLElement>("[autofocus]");
    if (auto) auto.focus();
    else if (dialog && !dialog.contains(document.activeElement)) dialog.focus();
  });
  onCleanup(() => {
    const i = dialog ? open.lastIndexOf(dialog) : -1;
    if (i >= 0) open.splice(i, 1);
    window.removeEventListener("keydown", onKey, { capture: true });
    if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
  });
  return (
    <Portal>
      <div
        class="backdrop"
        onMouseDown={(e) => {
          if (e.target === e.currentTarget) props.onClose();
        }}
      >
        <div ref={dialog} class={`modal ${props.class ?? ""}`} role="dialog" aria-modal="true" aria-label={props.label} tabIndex={-1}>
          {props.children}
        </div>
      </div>
    </Portal>
  );
}
