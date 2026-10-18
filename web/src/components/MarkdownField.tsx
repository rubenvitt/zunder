// Markdown field: rendered like a document, edited in place (WYSIWYG), or as source.
//
// Task references such as "ZUN-42" become links to the task (with a hover card)
// as long as such a task exists; the stored Markdown stays plain text.
//
// Same rules as TextField: only real edits are committed (on blur or Cmd/Ctrl+Enter),
// Escape reverts, and while nothing was typed the field follows the stored value,
// so a plan an agent writes meanwhile is never overwritten with stale text.

import { createEffect, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import { isClosed } from "../sync/derived";
import { taskIndex } from "../state/store";
import { openTask } from "../state/ui";
import type { Editor, TaskRefs, ToolbarCommand } from "./editor/tiptap";
import { Icon, type IconName } from "./Icon";
import { TextField } from "./Fields";

type EditorModule = typeof import("./editor/tiptap");

let modulePromise: Promise<EditorModule> | null = null;
/** Starts loading the editor early (e.g. when a task is selected), so opening it feels instant. */
export function preloadEditor(): Promise<EditorModule> {
  modulePromise ??= import("./editor/tiptap");
  return modulePromise;
}

const TOOLBAR: { command: ToolbarCommand; icon: IconName; label: string }[] = [
  { command: "heading", icon: "heading", label: "Überschrift" },
  { command: "bold", icon: "bold", label: "Fett (⌘B)" },
  { command: "italic", icon: "italic", label: "Kursiv (⌘I)" },
  { command: "strike", icon: "strike", label: "Durchgestrichen" },
  { command: "code", icon: "code", label: "Code" },
  { command: "link", icon: "link", label: "Link" },
  { command: "bulletList", icon: "bulletList", label: "Liste" },
  { command: "orderedList", icon: "orderedList", label: "Nummerierte Liste" },
  { command: "taskList", icon: "checklist", label: "Checkliste" },
  { command: "blockquote", icon: "quote", label: "Zitat" },
  { command: "codeBlock", icon: "codeBlock", label: "Codeblock" },
];

const taskRefs: TaskRefs = {
  resolve(ref) {
    const task = taskIndex().byRef(ref);
    return task ? { closed: isClosed(task.status) } : null;
  },
  open(ref) {
    const task = taskIndex().byRef(ref);
    if (task) openTask(task.id);
  },
};

export interface MarkdownFieldProps {
  value: string | null;
  onCommit: (value: string) => unknown;
  label: string;
  placeholder?: string;
  class?: string;
}

export function MarkdownField(props: MarkdownFieldProps) {
  const stored = () => props.value ?? "";
  const [source, setSource] = createSignal(false);
  const [focused, setFocused] = createSignal(false);
  const [dirty, setDirty] = createSignal(false);
  const [tick, setTick] = createSignal(0);
  const [editor, setEditor] = createSignal<Editor | null>(null);
  let mod: EditorModule | undefined;
  let host: HTMLDivElement | undefined;
  let wrapper: HTMLDivElement | undefined;

  const commit = () => {
    const ed = editor();
    if (!ed || !mod) return;
    const edited = dirty();
    setDirty(false);
    if (edited) {
      const value = mod.getMarkdown(ed);
      if (value !== stored()) props.onCommit(value);
    }
    // Show what is stored now (normalized, refused or changed meanwhile).
    if (mod.getMarkdown(ed) !== stored()) mod.setMarkdown(ed, stored());
  };

  const revert = () => {
    const ed = editor();
    if (!ed || !mod) return;
    setDirty(false);
    if (mod.getMarkdown(ed) !== stored()) mod.setMarkdown(ed, stored());
    ed.commands.blur();
    (document.activeElement as HTMLElement | null)?.blur?.();
  };

  onMount(() => {
    void preloadEditor().then((m) => {
      if (!host) return;
      mod = m;
      const ed = m.createMarkdownEditor({
        element: host,
        content: stored(),
        label: props.label,
        placeholder: props.placeholder ?? "Schreiben …",
        onUserChange: () => setDirty(true),
        onStateChange: () => setTick((n) => n + 1),
        onFocus: () => setFocused(true),
        onBlur: (related) => {
          // Clicking the toolbar keeps the editing session.
          if (related instanceof Node && wrapper?.contains(related)) return;
          setFocused(false);
          commit();
        },
        onEscape: revert,
        taskRefs,
        onSubmit: () => {
          commit();
          ed.commands.blur();
        },
      });
      setEditor(ed);
    });
  });
  onCleanup(() => {
    const ed = editor();
    if (ed && dirty()) commit();
    ed?.destroy();
  });

  // Follow the stored value while nothing was typed.
  createEffect(
    on(stored, (value) => {
      const ed = editor();
      if (!ed || !mod || dirty()) return;
      if (mod.getMarkdown(ed) !== value) mod.setMarkdown(ed, value);
    }),
  );

  // Mentions follow the tasks: a new, renamed or closed task changes how they look.
  createEffect(
    on(taskIndex, () => {
      const ed = editor();
      if (ed && mod) mod.refreshTaskRefs(ed);
    }, { defer: true }),
  );

  const active = (command: ToolbarCommand) => {
    tick();
    const ed = editor();
    return !!ed && !!mod && mod.isActive(ed, command);
  };

  const toggleSource = () => {
    const ed = editor();
    if (!source() && ed && dirty()) commit();
    setSource((s) => !s);
  };

  return (
    <div ref={wrapper} class={`md-field ${props.class ?? ""}`} classList={{ focused: focused(), source: source(), empty: !stored() && !focused() }}>
      <Show when={source()}>
        <TextField multiline autosize class="md-source mono" label={props.label} value={props.value} placeholder={props.placeholder} onCommit={props.onCommit} />
      </Show>
      {/* The editor owns the host's children: nothing else may render into it. */}
      <div
        ref={host}
        class="md-host"
        hidden={source() || !editor()}
        onClick={(e) => {
          // A click on the empty area below the text starts writing at the end.
          const ed = editor();
          if (ed && e.target === host) ed.commands.focus("end");
        }}
      />
      <Show when={!editor() && !source()}>
        <div class="md-host">
          <div class="md-content md-loading">{stored() || props.placeholder}</div>
        </div>
      </Show>
      {/* Below the text: appearing on focus must not move the text under the pointer. */}
      <div class="md-toolbar" role="toolbar" aria-label={`${props.label}: Formatierung`}>
        <Show when={!source()}>
          <For each={TOOLBAR}>
            {(item) => (
              <button
                type="button"
                class="md-tool"
                classList={{ active: active(item.command) }}
                title={item.label}
                aria-label={item.label}
                aria-pressed={active(item.command)}
                tabIndex={-1}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => {
                  const ed = editor();
                  if (ed && mod) mod.runCommand(ed, item.command);
                }}
              >
                <Icon name={item.icon} size={16} />
              </button>
            )}
          </For>
        </Show>
        <span class="spacer" />
        <button
          type="button"
          class="md-tool md-source-toggle"
          classList={{ active: source() }}
          title={source() ? "Formatiert bearbeiten" : "Markdown-Quelltext bearbeiten"}
          aria-pressed={source()}
          onMouseDown={(e) => e.preventDefault()}
          onClick={toggleSource}
        >
          <Icon name={source() ? "eye" : "document"} size={16} />
          <span>{source() ? "Ansicht" : "Markdown"}</span>
        </button>
      </div>
    </div>
  );
}
