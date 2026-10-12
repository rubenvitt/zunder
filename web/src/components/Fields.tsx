// Small inline editors that commit on blur/Enter and revert on Escape.
//
// Only real edits are committed: focusing a field and leaving it without typing
// writes nothing, so a value that changed meanwhile (pull from an agent or another
// device) is not overwritten with the stale text.

import { createEffect, createSignal, onCleanup, onMount, Show, type JSX } from "solid-js";

interface TextFieldProps {
  value: string | null;
  onCommit: (value: string) => unknown;
  placeholder?: string;
  multiline?: boolean;
  label: string;
  class?: string;
  type?: "text" | "url";
  rows?: number;
  /** Textarea that grows with its content instead of scrolling. */
  autosize?: boolean;
  /** A single-line value shown wrapped over several lines (e.g. a long title); Enter commits. */
  wrap?: boolean;
}

/** Grows a textarea to fit its content. */
export function fitHeight(el: HTMLTextAreaElement): void {
  el.style.height = "auto";
  el.style.height = `${el.scrollHeight + (el.offsetHeight - el.clientHeight)}px`;
}

export function TextField(props: TextFieldProps) {
  const stored = () => props.value ?? "";
  const [draft, setDraft] = createSignal(stored());
  // Set by typing; until then the field follows the stored value even while focused.
  const [dirty, setDirty] = createSignal(false);
  createEffect(() => {
    const v = stored();
    if (!dirty()) setDraft(v);
  });

  const commit = () => {
    const value = draft();
    const edited = dirty();
    setDirty(false);
    if (edited && value !== stored()) props.onCommit(value);
    // Show what is stored now: the committed value as normalized by the caller, the
    // old value when the commit was refused, or a remote change that arrived meanwhile.
    setDraft(stored());
  };
  const onInput = (value: string) => {
    setDraft(value);
    setDirty(true);
  };
  const onKeyDown: JSX.EventHandler<HTMLInputElement | HTMLTextAreaElement, KeyboardEvent> = (e) => {
    if (e.key === "Escape") {
      setDirty(false);
      setDraft(stored());
      e.currentTarget.blur();
      e.stopPropagation();
    } else if (e.key === "Enter" && (!props.multiline || props.wrap || e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      e.currentTarget.blur();
    }
  };
  const common = {
    class: `field ${props.class ?? ""}`,
    placeholder: props.placeholder,
    "aria-label": props.label,
    onBlur: commit,
    onKeyDown,
  };
  let area: HTMLTextAreaElement | undefined;
  const grows = () => props.autosize || props.wrap;
  createEffect(() => {
    draft();
    if (area && grows()) fitHeight(area);
  });
  onMount(() => {
    if (!area || !grows()) return;
    // Fonts and the panel width settle after mounting.
    const observer = new ResizeObserver(() => area && fitHeight(area));
    observer.observe(area);
    onCleanup(() => observer.disconnect());
  });
  return (
    <Show
      when={props.multiline || props.wrap}
      fallback={<input {...common} type={props.type ?? "text"} value={draft()} onInput={(e) => onInput(e.currentTarget.value)} />}
    >
      <textarea
        ref={area}
        {...common}
        class={`${common.class}${grows() ? " autosize" : ""}`}
        rows={props.wrap ? 1 : (props.rows ?? 4)}
        value={draft()}
        onInput={(e) => onInput(props.wrap ? e.currentTarget.value.replace(/\n/g, " ") : e.currentTarget.value)}
      />
    </Show>
  );
}
