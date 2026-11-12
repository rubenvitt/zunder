// The web UI's own context menu (browser, mobile, or when the native one fails):
// fixed at the pointer, kept inside the window, submenus to the side. Keyboard: arrows,
// Home/End, Enter/Space, → opens and ← closes a submenu, Esc and Tab close.

import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { closeContextMenu, type MenuEntry, type MenuRequest } from "../state/menu";
import { Icon } from "./Icon";

const MARGIN = 8;

function run(request: MenuRequest, action: () => void): void {
  closeContextMenu();
  const back = request.returnFocus;
  if (back && back.isConnected) back.focus({ preventScroll: true });
  action();
}

export function ContextMenu(props: { request: MenuRequest }) {
  const close = () => {
    closeContextMenu();
    const back = props.request.returnFocus;
    if (back && back.isConnected) back.focus({ preventScroll: true });
  };
  onMount(() => {
    const outside = (e: PointerEvent) => {
      if (!(e.target instanceof Element) || !e.target.closest(".context-menu")) closeContextMenu();
    };
    const dismiss = () => closeContextMenu();
    document.addEventListener("pointerdown", outside, true);
    window.addEventListener("blur", dismiss);
    window.addEventListener("resize", dismiss);
    document.addEventListener("scroll", dismiss, true);
    onCleanup(() => {
      document.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("blur", dismiss);
      window.removeEventListener("resize", dismiss);
      document.removeEventListener("scroll", dismiss, true);
    });
  });
  return <MenuPanel request={props.request} items={props.request.items} at={{ x: props.request.x, y: props.request.y }} onClose={close} />;
}

interface PanelProps {
  request: MenuRequest;
  items: MenuEntry[];
  /** Root menu: the pointer position. */
  at?: { x: number; y: number };
  /** Submenu: the item it belongs to. */
  anchor?: DOMRect;
  /** Esc: the root closes everything, a submenu only itself. */
  onClose: () => void;
}

function MenuPanel(props: PanelProps) {
  let el: HTMLUListElement | undefined;
  const buttons: (HTMLButtonElement | undefined)[] = [];
  const [openSub, setOpenSub] = createSignal<{ index: number; anchor: DOMRect } | null>(null);

  const usable = (i: number) => {
    const item = props.items[i];
    return !!item && item.kind !== "separator" && !item.disabled;
  };
  const focusAt = (i: number) => buttons[i]?.focus({ preventScroll: true });
  const step = (from: number, delta: 1 | -1) => {
    const n = props.items.length;
    for (let k = 1; k <= n; k++) {
      const i = (((from + delta * k) % n) + n) % n;
      if (usable(i)) return i;
    }
    return from;
  };
  const current = () => buttons.findIndex((b) => b === document.activeElement);

  onMount(() => {
    if (!el) return;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    let left: number;
    let top: number;
    if (props.anchor) {
      left = props.anchor.right - 4;
      if (left + w > vw - MARGIN) left = props.anchor.left - w + 4;
      top = props.anchor.top - 5;
    } else {
      left = props.at!.x;
      top = props.at!.y;
      if (left + w > vw - MARGIN) left = left - w;
    }
    left = Math.max(MARGIN, Math.min(left, vw - w - MARGIN));
    top = Math.max(MARGIN, Math.min(top, vh - h - MARGIN));
    // Placed before the first paint; set directly so the first item can take the focus right away.
    el.style.left = `${left}px`;
    el.style.top = `${top}px`;
    focusAt(step(-1, 1));
  });

  const openSubmenu = (i: number) => {
    const b = buttons[i];
    if (!b || !usable(i) || props.items[i]!.kind !== "submenu") return;
    setOpenSub({ index: i, anchor: b.getBoundingClientRect() });
  };
  const closeSubmenu = () => {
    const sub = openSub();
    setOpenSub(null);
    if (sub) focusAt(sub.index);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    // The menu owns the keyboard while it is open (no j/k/x … underneath).
    e.stopPropagation();
    // Keys from an open submenu are handled there.
    if (openSub() && !el?.contains(document.activeElement)) return;
    const i = current();
    switch (e.key) {
      case "ArrowDown":
        focusAt(step(i, 1));
        break;
      case "ArrowUp":
        focusAt(step(i < 0 ? 0 : i, -1));
        break;
      case "Home":
        focusAt(step(-1, 1));
        break;
      case "End":
        focusAt(step(0, -1));
        break;
      case "ArrowRight":
        if (i >= 0) openSubmenu(i);
        break;
      case "ArrowLeft":
        if (props.anchor) props.onClose();
        break;
      case "Escape":
        props.onClose();
        break;
      case "Tab":
        closeContextMenu();
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  return (
    <>
      <ul
        ref={el}
        class="context-menu"
        role="menu"
        style={{ left: `${props.at?.x ?? 0}px`, top: `${props.at?.y ?? 0}px` }}
        onKeyDown={onKeyDown}
        onContextMenu={(e) => e.preventDefault()}
      >
        <For each={props.items}>
          {(item, i) => (
            <Show when={item.kind !== "separator"} fallback={<li class="context-menu-separator" role="separator" />}>
              <li role="none">
                <button
                  ref={(b) => (buttons[i()] = b)}
                  type="button"
                  class="context-menu-item"
                  classList={{
                    danger: item.kind === "item" && !!item.danger,
                    open: openSub()?.index === i(),
                  }}
                  role={item.kind === "item" && item.checked !== undefined ? "menuitemcheckbox" : "menuitem"}
                  aria-checked={item.kind === "item" && item.checked !== undefined ? item.checked : undefined}
                  aria-haspopup={item.kind === "submenu" ? "menu" : undefined}
                  aria-expanded={item.kind === "submenu" ? openSub()?.index === i() : undefined}
                  disabled={item.kind !== "separator" && item.disabled}
                  tabIndex={-1}
                  onPointerEnter={() => {
                    if (!usable(i())) return;
                    focusAt(i());
                    if (item.kind === "submenu") openSubmenu(i());
                    else if (openSub()) setOpenSub(null);
                  }}
                  onClick={() => {
                    if (item.kind === "submenu") openSubmenu(i());
                    else if (item.kind === "item") run(props.request, item.run);
                  }}
                >
                  <span class="context-menu-check" aria-hidden="true">
                    <Show when={item.kind === "item" && item.checked}>
                      <Icon name="check" size={12} />
                    </Show>
                  </span>
                  <span class="context-menu-label">{item.kind !== "separator" ? item.label : ""}</span>
                  <Show when={item.kind === "item" && item.hint}>
                    <kbd aria-hidden="true">{item.kind === "item" ? item.hint : ""}</kbd>
                  </Show>
                  <Show when={item.kind === "submenu"}>
                    <Icon name="right" size={12} />
                  </Show>
                </button>
              </li>
            </Show>
          )}
        </For>
      </ul>
      <Show when={openSub()} keyed>
        {(sub) => {
          const entry = props.items[sub.index];
          return entry?.kind === "submenu" ? <MenuPanel request={props.request} items={entry.items} anchor={sub.anchor} onClose={closeSubmenu} /> : null;
        }}
      </Show>
    </>
  );
}
