// Small inline editors that commit on blur/Enter and revert on Escape.
//
// Only real edits are committed: focusing a field and leaving it without typing
// writes nothing, so a value that changed meanwhile (pull from an agent or another
// device) is not overwritten with the stale text.

import { createEffect, createSignal, Show, type JSX } from "solid-js";

interface TextFieldProps {
  value: string | null;
  onCommit: (value: string) => unknown;
  placeholder?: string;
  multiline?: boolean;
  label: string;
  class?: string;
  type?: "text" | "url";
  rows?: number;
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
    } else if (e.key === "Enter" && (!props.multiline || e.metaKey || e.ctrlKey)) {
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
  return (
    <Show
      when={props.multiline}
      fallback={<input {...common} type={props.type ?? "text"} value={draft()} onInput={(e) => onInput(e.currentTarget.value)} />}
    >
      <textarea {...common} rows={props.rows ?? 4} value={draft()} onInput={(e) => onInput(e.currentTarget.value)} />
    </Show>
  );
}

export function Row(props: { label: string; children: JSX.Element }) {
  return (
    <div class="detail-row">
      <span class="detail-label">{props.label}</span>
      <div class="detail-value">{props.children}</div>
    </div>
  );
}
