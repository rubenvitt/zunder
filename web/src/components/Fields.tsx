// Small inline editors that commit on blur/Enter and revert on Escape.

import { createEffect, createSignal, Show, type JSX } from "solid-js";

interface TextFieldProps {
  value: string | null;
  onCommit: (value: string) => void;
  placeholder?: string;
  multiline?: boolean;
  label: string;
  class?: string;
  type?: "text" | "url";
  rows?: number;
}

export function TextField(props: TextFieldProps) {
  const [draft, setDraft] = createSignal(props.value ?? "");
  let focused = false;
  createEffect(() => {
    const v = props.value ?? "";
    if (!focused) setDraft(v);
  });

  const commit = () => {
    if (draft() !== (props.value ?? "")) props.onCommit(draft());
  };
  const onKeyDown: JSX.EventHandler<HTMLInputElement | HTMLTextAreaElement, KeyboardEvent> = (e) => {
    if (e.key === "Escape") {
      setDraft(props.value ?? "");
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
    onFocus: () => (focused = true),
    onBlur: () => {
      focused = false;
      commit();
    },
    onKeyDown,
  };
  return (
    <Show
      when={props.multiline}
      fallback={<input {...common} type={props.type ?? "text"} value={draft()} onInput={(e) => setDraft(e.currentTarget.value)} />}
    >
      <textarea {...common} rows={props.rows ?? 4} value={draft()} onInput={(e) => setDraft(e.currentTarget.value)} />
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
