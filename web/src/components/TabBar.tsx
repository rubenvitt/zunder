// The phone's tab bar: four fixed views and "Mehr" for the rest, as on iOS. A lens of glass marks
// the current tab; a thumb can slide over the bar, the lens follows, and letting go switches.

import { createSignal, For, onCleanup, Show } from "solid-js";
import { Icon, type IconName } from "./Icon";
import type { ViewName } from "../state/router";

export interface TabItem {
  view: ViewName;
  label: string;
  short?: string;
  icon: IconName;
}

/** Inner padding of the bar (matches `.tabbar` in glass.css): the lens and the hit test start there. */
const PAD = 5;
/** A thumb must travel this far sideways before a press becomes a slide. */
const SLOP = 8;

export function TabBar(props: {
  primary: TabItem[];
  more: TabItem[];
  current: ViewName;
  inert: boolean;
  countOf: (view: ViewName) => number;
  badgeOf: (view: ViewName) => number;
  onSelect: (view: ViewName) => void;
}) {
  let bar!: HTMLElement;
  const [moreOpen, setMoreOpen] = createSignal(false);
  // While sliding: the lens' left edge in px and the tab beneath the thumb.
  const [dragX, setDragX] = createSignal<number | null>(null);
  const [dragIndex, setDragIndex] = createSignal<number | null>(null);

  const slots = () => props.primary.length + 1;
  const moreIndex = () => props.primary.length;
  const inMore = () => props.more.some((m) => m.view === props.current);
  const activeIndex = () => {
    const i = props.primary.findIndex((p) => p.view === props.current);
    return i >= 0 ? i : inMore() ? moreIndex() : -1;
  };
  const lensIndex = () => dragIndex() ?? (moreOpen() ? moreIndex() : activeIndex());
  const moreBadge = () => props.more.reduce((n, m) => n + props.badgeOf(m.view), 0);

  const select = (index: number) => {
    if (index === moreIndex()) {
      setMoreOpen((open) => !open);
      return;
    }
    setMoreOpen(false);
    const item = props.primary[index];
    if (item) props.onSelect(item.view);
  };

  // ---- Sliding over the bar ----
  let start: { id: number; x: number } | null = null;
  let sliding = false;
  // A slide ends in a click on whatever lies under the thumb: that one is already handled.
  let swallowClick = false;
  const geometry = () => {
    const rect = bar.getBoundingClientRect();
    const width = (rect.width - 2 * PAD) / slots();
    return { left: rect.left + PAD, width };
  };
  const indexAt = (clientX: number) => {
    const { left, width } = geometry();
    return Math.max(0, Math.min(slots() - 1, Math.floor((clientX - left) / width)));
  };
  const follow = (clientX: number) => {
    const { left, width } = geometry();
    const x = Math.max(0, Math.min((slots() - 1) * width, clientX - left - width / 2));
    setDragX(x);
    const index = indexAt(clientX);
    if (index !== dragIndex()) {
      if (dragIndex() !== null) navigator.vibrate?.(6);
      setDragIndex(index);
    }
  };
  const onPointerDown = (e: PointerEvent) => {
    if (e.button !== 0) return;
    start = { id: e.pointerId, x: e.clientX };
    sliding = false;
  };
  const onPointerMove = (e: PointerEvent) => {
    if (!start || e.pointerId !== start.id) return;
    if (!sliding) {
      if (Math.abs(e.clientX - start.x) < SLOP) return;
      sliding = true;
      setMoreOpen(false);
      bar.setPointerCapture(e.pointerId);
    }
    follow(e.clientX);
  };
  const finish = (commit: boolean, e: PointerEvent) => {
    if (!start || e.pointerId !== start.id) return;
    const index = dragIndex();
    start = null;
    if (sliding) {
      sliding = false;
      swallowClick = true;
      setTimeout(() => (swallowClick = false), 0);
      setDragX(null);
      setDragIndex(null);
      if (commit && index !== null) select(index);
    }
  };
  const onTab = (index: number) => {
    if (swallowClick) return;
    select(index);
  };

  // ---- "Mehr" closes on a tap elsewhere and on Escape ----
  let sheet: HTMLDivElement | undefined;
  const onOutside = (e: PointerEvent) => {
    if (!moreOpen()) return;
    const target = e.target as Node;
    if (sheet?.contains(target) || bar.contains(target)) return;
    setMoreOpen(false);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && moreOpen()) {
      e.stopPropagation();
      setMoreOpen(false);
    }
  };
  document.addEventListener("pointerdown", onOutside, true);
  document.addEventListener("keydown", onKey, true);
  onCleanup(() => {
    document.removeEventListener("pointerdown", onOutside, true);
    document.removeEventListener("keydown", onKey, true);
  });

  return (
    <>
      <nav
        ref={bar}
        class="tabbar"
        classList={{ sliding: dragX() !== null }}
        aria-label="Sichten"
        inert={props.inert}
        style={{ "--tabs": slots() }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={(e) => finish(true, e)}
        onPointerCancel={(e) => finish(false, e)}
      >
        <span
          class="tab-lens"
          aria-hidden="true"
          classList={{ hidden: lensIndex() < 0 }}
          style={{ transform: dragX() !== null ? `translateX(${dragX()}px) scale(1.08)` : `translateX(${Math.max(0, lensIndex()) * 100}%)` }}
        />
        <For each={props.primary}>
          {(item, i) => (
            <button
              class="tab"
              classList={{ active: props.current === item.view, hover: dragIndex() === i() }}
              onClick={() => onTab(i())}
              aria-current={props.current === item.view ? "page" : undefined}
            >
              <Icon name={item.icon} size={20} />
              <span>{item.short ?? item.label}</span>
              <Show when={props.badgeOf(item.view) > 0}>
                <span class="tab-count">{props.badgeOf(item.view)}</span>
              </Show>
            </button>
          )}
        </For>
        <button
          class="tab"
          classList={{ active: inMore(), hover: dragIndex() === moreIndex() }}
          onClick={() => onTab(moreIndex())}
          aria-haspopup="menu"
          aria-expanded={moreOpen()}
          aria-current={inMore() ? "page" : undefined}
        >
          <Icon name="more" size={20} />
          <span>Mehr</span>
          <Show when={moreBadge() > 0}>
            <span class="tab-count">{moreBadge()}</span>
          </Show>
        </button>
      </nav>
      <Show when={moreOpen()}>
        <div ref={sheet} class="tab-more" role="menu" aria-label="Weitere Sichten">
          <For each={props.more}>
            {(item) => (
              <button
                class="tab-more-item"
                classList={{ active: props.current === item.view }}
                role="menuitem"
                onClick={() => {
                  setMoreOpen(false);
                  props.onSelect(item.view);
                }}
              >
                <Icon name={item.icon} size={20} />
                <span>{item.label}</span>
                <Show when={props.countOf(item.view) > 0}>
                  <span class="tab-more-count">{props.countOf(item.view)}</span>
                </Show>
              </button>
            )}
          </For>
        </div>
      </Show>
    </>
  );
}
