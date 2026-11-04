// Shared empty state: a line figure or an icon in a soft accent disc, a short title, an optional
// hint and actions.

import { Show, type JSX } from "solid-js";
import { Figure, type FigureName } from "./Figure";
import { Icon, type IconName } from "./Icon";

export interface EmptyStateProps {
  icon: IconName;
  /** A hairline figure in place of the icon; only in the large state, compact ones keep the icon. */
  figure?: { name: FigureName; label: string; play?: boolean };
  title: string;
  hint?: JSX.Element;
  /** Smaller and left-aligned, for empty sections inside a view. */
  compact?: boolean;
  children?: JSX.Element;
}

export function EmptyState(props: EmptyStateProps) {
  return (
    <div class="empty-state" classList={{ compact: props.compact }}>
      <Show
        when={!props.compact && props.figure}
        fallback={
          <span class="empty-icon" aria-hidden="true">
            <Icon name={props.icon} size={props.compact ? 18 : 28} />
          </span>
        }
      >
        {(figure) => <Figure name={figure().name} label={figure().label} play={figure().play} />}
      </Show>
      <div class="empty-text">
        <p class="empty-title">{props.title}</p>
        <Show when={props.hint}>
          <p class="empty-hint">{props.hint}</p>
        </Show>
        <Show when={props.children}>
          <div class="empty-actions">{props.children}</div>
        </Show>
      </div>
    </div>
  );
}
