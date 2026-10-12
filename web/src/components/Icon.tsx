// Icons8 "Fluency Systems Regular" as inline SVG (paths in icons8.ts): no icon font, no requests.

import { ICON_PATHS } from "./icons8";

export type IconName = keyof typeof ICON_PATHS;

export function Icon(props: { name: IconName; size?: number; title?: string }) {
  return (
    <svg
      class="icon"
      width={props.size ?? 18}
      height={props.size ?? 18}
      viewBox="0 0 48 48"
      fill="currentColor"
      aria-hidden={props.title ? undefined : "true"}
      role={props.title ? "img" : undefined}
    >
      {props.title && <title>{props.title}</title>}
      <path d={ICON_PATHS[props.name]} />
    </svg>
  );
}
