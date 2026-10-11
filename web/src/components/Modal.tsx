import { onCleanup, onMount, type JSX } from "solid-js";
import { Portal } from "solid-js/web";

/** Dialog with backdrop; Escape closes it wherever the focus is. */
export function Modal(props: { label: string; onClose: () => void; class?: string; children: JSX.Element }) {
  let dialog: HTMLDivElement | undefined;
  let previous: Element | null = null;
  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    e.preventDefault();
    e.stopPropagation();
    props.onClose();
  };
  onMount(() => {
    previous = document.activeElement;
    window.addEventListener("keydown", onKey, { capture: true });
    const auto = dialog?.querySelector<HTMLElement>("[autofocus]");
    if (auto) auto.focus();
    else if (dialog && !dialog.contains(document.activeElement)) dialog.focus();
  });
  onCleanup(() => {
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
