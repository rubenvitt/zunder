// WYSIWYG Markdown editor (TipTap/ProseMirror). Loaded on demand by MarkdownField,
// so the editor stays out of the start bundle.

import { Editor, Extension } from "@tiptap/core";
import { TaskItem, TaskList } from "@tiptap/extension-list";
import { Placeholder } from "@tiptap/extensions";
import { Markdown } from "@tiptap/markdown";
import type { Node as PmNode } from "@tiptap/pm/model";
import { Plugin, PluginKey } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";
import StarterKit from "@tiptap/starter-kit";
import { openExternal } from "../../platform/runtime";
import { findTaskRefs } from "./task-refs";

export type { Editor };

export interface MarkdownEditorOptions {
  element: HTMLElement;
  content: string;
  label: string;
  placeholder: string;
  /** A change typed by the user (not one set from outside). */
  onUserChange: () => void;
  onStateChange: () => void;
  onFocus: () => void;
  onBlur: (related: EventTarget | null) => void;
  onEscape: () => void;
  onSubmit: () => void;
  /** Task references ("ZUN-42") in the text become links when they resolve. */
  taskRefs?: TaskRefs;
}

/** How mentions of tasks are resolved and opened. */
export interface TaskRefs {
  /** The task behind a reference, or null when there is none (then the text stays plain). */
  resolve(ref: string): { closed: boolean } | null;
  open(ref: string): void;
}

const mentionKey = new PluginKey<DecorationSet>("taskMentions");

/** Mentions as decorations: the Markdown stays plain text, only the view links them. */
function mentionDecorations(doc: PmNode, refs: TaskRefs): DecorationSet {
  const decorations: Decoration[] = [];
  doc.descendants((node, pos, parent) => {
    if (parent?.type.spec.code) return false;
    if (!node.isText || !node.text) return true;
    if (node.marks.some((m) => m.type.name === "code" || m.type.name === "link")) return false;
    for (const m of findTaskRefs(node.text)) {
      const task = refs.resolve(m.ref);
      if (!task) continue;
      decorations.push(
        Decoration.inline(pos + m.start, pos + m.end, {
          nodeName: "a",
          href: `#/aufgabe/${m.ref}`,
          class: task.closed ? "task-mention closed" : "task-mention",
          "data-task-ref": m.ref,
        }),
      );
    }
    return false;
  });
  return DecorationSet.create(doc, decorations);
}

function taskMentions(refs: TaskRefs) {
  return Extension.create({
    name: "taskMentions",
    addProseMirrorPlugins() {
      return [
        new Plugin<DecorationSet>({
          key: mentionKey,
          state: {
            init: (_, state) => mentionDecorations(state.doc, refs),
            apply: (tr, old, _oldState, state) => (tr.docChanged || tr.getMeta(mentionKey) ? mentionDecorations(state.doc, refs) : old),
          },
          props: {
            decorations: (state) => mentionKey.getState(state),
          },
        }),
      ];
    },
  });
}

/** Re-resolves the mentions, e.g. after tasks were created, renamed or closed. */
export function refreshTaskRefs(editor: Editor): void {
  if (!editor.isDestroyed) editor.view.dispatch(editor.state.tr.setMeta(mentionKey, true).setMeta("addToHistory", false));
}

/** The mention under a primary click that should open its task rather than edit. */
function mentionToOpen(focused: boolean, event: MouseEvent): string | null {
  if (event.button !== 0 || (focused && !event.metaKey && !event.ctrlKey)) return null;
  return (event.target as Element | null)?.closest?.("a[data-task-ref]")?.getAttribute("data-task-ref") ?? null;
}

const SAFE_PROTOCOLS = ["http:", "https:", "mailto:"];

function safeUrl(url: string): string | null {
  try {
    return SAFE_PROTOCOLS.includes(new URL(url).protocol) ? url : null;
  } catch {
    return null;
  }
}

/** The safe link under a primary click that should open rather than edit. */
function linkToOpen(focused: boolean, event: MouseEvent): string | null {
  if (event.button !== 0 || (focused && !event.metaKey && !event.ctrlKey)) return null;
  const anchor = (event.target as Element | null)?.closest?.("a");
  return anchor ? safeUrl(anchor.getAttribute("href") ?? "") : null;
}

export function createMarkdownEditor(options: MarkdownEditorOptions): Editor {
  const editor = new Editor({
    element: options.element,
    content: options.content,
    contentType: "markdown",
    extensions: [
      StarterKit.configure({
        heading: { levels: [1, 2, 3] },
        link: {
          openOnClick: false,
          autolink: true,
          defaultProtocol: "https",
          isAllowedUri: (url, ctx) => !!safeUrl(url) || ctx.defaultValidate(url),
          HTMLAttributes: { target: "_blank", rel: "noopener noreferrer nofollow" },
        },
      }),
      TaskList,
      TaskItem.configure({ nested: true }),
      Placeholder.configure({ placeholder: options.placeholder }),
      Markdown.configure({ indentation: { style: "space", size: 2 } }),
      ...(options.taskRefs ? [taskMentions(options.taskRefs)] : []),
    ],
    editorProps: {
      attributes: {
        class: "md-content",
        role: "textbox",
        "aria-multiline": "true",
        "aria-label": options.label,
        spellcheck: "true",
      },
      handleKeyDown(_view, event) {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          options.onEscape();
          return true;
        }
        if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
          event.preventDefault();
          options.onSubmit();
          return true;
        }
        return false;
      },
      handleDOMEvents: {
        // Reading mode (editor not focused): a click on a link opens it instead of
        // starting to edit; while writing, Cmd/Ctrl+click does. Decided on mousedown,
        // because the click would focus the editor first.
        mousedown(view, event) {
          if (!linkToOpen(view.hasFocus(), event) && !mentionToOpen(view.hasFocus(), event)) return false;
          event.preventDefault();
          return true;
        },
        click(view, event) {
          const ref = mentionToOpen(view.hasFocus(), event);
          if (ref && options.taskRefs) {
            event.preventDefault();
            options.taskRefs.open(ref);
            return true;
          }
          const href = linkToOpen(view.hasFocus(), event);
          if (!href) return false;
          event.preventDefault();
          openExternal(href);
          return true;
        },
      },
    },
    onUpdate: ({ transaction }) => {
      if (transaction.getMeta("external") !== true) options.onUserChange();
    },
    onTransaction: () => options.onStateChange(),
    onFocus: () => options.onFocus(),
    onBlur: ({ event }) => options.onBlur(event.relatedTarget),
  });
  return editor;
}

/** Replaces the content from outside (stored value) without counting as a user edit. */
export function setMarkdown(editor: Editor, markdown: string): void {
  editor.chain().setMeta("external", true).setMeta("addToHistory", false).setContent(markdown, { contentType: "markdown", emitUpdate: true }).run();
}

export function getMarkdown(editor: Editor): string {
  return editor.isEmpty ? "" : editor.getMarkdown().replace(/\s+$/, "");
}

export type ToolbarCommand =
  | "bold"
  | "italic"
  | "strike"
  | "code"
  | "heading"
  | "bulletList"
  | "orderedList"
  | "taskList"
  | "blockquote"
  | "codeBlock"
  | "link";

export function isActive(editor: Editor, command: ToolbarCommand): boolean {
  switch (command) {
    case "heading":
      return editor.isActive("heading");
    case "taskList":
      return editor.isActive("taskList");
    default:
      return editor.isActive(command);
  }
}

export function runCommand(editor: Editor, command: ToolbarCommand): void {
  const chain = editor.chain().focus();
  switch (command) {
    case "bold":
      chain.toggleBold().run();
      break;
    case "italic":
      chain.toggleItalic().run();
      break;
    case "strike":
      chain.toggleStrike().run();
      break;
    case "code":
      chain.toggleCode().run();
      break;
    case "heading": {
      const level = editor.getAttributes("heading").level as number | undefined;
      if (!level) chain.setHeading({ level: 2 }).run();
      else if (level === 2) chain.setHeading({ level: 3 }).run();
      else chain.setParagraph().run();
      break;
    }
    case "bulletList":
      chain.toggleBulletList().run();
      break;
    case "orderedList":
      chain.toggleOrderedList().run();
      break;
    case "taskList":
      chain.toggleTaskList().run();
      break;
    case "blockquote":
      chain.toggleBlockquote().run();
      break;
    case "codeBlock":
      chain.toggleCodeBlock().run();
      break;
    case "link": {
      if (editor.isActive("link")) {
        chain.extendMarkRange("link").unsetLink().run();
        break;
      }
      const url = window.prompt("Link-Adresse", "https://");
      if (!url || url === "https://") {
        editor.commands.focus();
        break;
      }
      const href = safeUrl(url) ?? safeUrl(`https://${url}`);
      if (!href) {
        editor.commands.focus();
        break;
      }
      if (editor.state.selection.empty) chain.insertContent({ type: "text", text: url, marks: [{ type: "link", attrs: { href } }] }).run();
      else chain.extendMarkRange("link").setLink({ href }).run();
      break;
    }
  }
}
