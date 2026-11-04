// Line figures from hairline by Lucas Marques (https://hairline.lucasmarkes.com, MIT license,
// notice in THIRD_PARTY_NOTICES.md). Used as empty-state pictures: their rest pose is the
// picture, the pointer is a bonus. Colors come from our tokens (see `.figure` in styles.css).

import { onCleanup, onMount } from "solid-js";
import {
  basket,
  branches,
  drawer,
  elevator,
  hub,
  padlock,
  query,
  rail,
  slow,
  type Figure as HairlineFigure,
  type HairlineOptions,
} from "@lucasmarkes/hairline";

// Only the figures we use, so the bundle carries only these.
const FIGURES = { basket, branches, drawer, elevator, hub, padlock, query, rail, slow } satisfies Record<
  string,
  (el: HTMLElement, options?: HairlineOptions) => HairlineFigure
>;

export type FigureName = keyof typeof FIGURES;

export function Figure(props: { name: FigureName; label: string; play?: boolean }) {
  let el!: HTMLDivElement;
  onMount(() => {
    // Without a DOM that can draw (tests), the empty state still has its text.
    try {
      const figure = FIGURES[props.name](el, { label: props.label, play: props.play ?? false, intensity: 0.5 });
      onCleanup(() => figure.destroy());
    } catch {
      el.remove();
    }
  });
  // A figure that plays on its own is a small reward (Nichts fällig): it gets the accent.
  return <div ref={el} class="figure" classList={{ reward: props.play }} />;
}
