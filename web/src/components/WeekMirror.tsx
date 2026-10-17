// Wochenspiegel in Heute (contract 5.1): what got done this ISO week, by weight and for whom.
// It mirrors, it does not judge: plain counts, two slim bars, no scores and no targets.

import { createMemo, For, Show } from "solid-js";
import { FOR_WHOM_KEYS, WEIGHT_KEYS, type WeekBalance } from "../sync/balance";
import { fireOf } from "../sync/dates";
import { BALANCE_FOR_WHOM_LABELS, BALANCE_WEIGHT_LABELS, doneCountLabel, weekForWhomLine, weekWeightLine } from "../labels";
import { weekBalanceOf } from "../state/selectors";
import { now } from "../state/store";
import { setWeekMirrorOpen, weekMirrorOpen } from "../state/ui";

/**
 * One bar: a segment per non-empty group, as wide as its share. The line above says the same
 * in words, so screen readers skip the bar; segments name their group on hover.
 */
function BalanceBar<K extends string>(props: { kind: string; counts: Record<K, number>; keys: readonly K[]; labels: Record<K, string> }) {
  const parts = () => props.keys.filter((k) => props.counts[k] > 0);
  return (
    <div class={`balance-bar balance-bar-${props.kind}`} aria-hidden="true">
      <For each={parts()}>
        {(key) => (
          <span
            class={`balance-seg balance-${props.kind}-${key}`}
            style={{ "flex-grow": props.counts[key] }}
            data-key={key}
            data-count={props.counts[key]}
            title={`${props.counts[key]} ${props.labels[key]}`}
          />
        )}
      </For>
    </div>
  );
}

export function WeekMirror() {
  // The week changes at most once a week; the balance only when tasks change.
  const week = createMemo(() => fireOf(new Date(now())));
  const balance = createMemo<WeekBalance>(() => weekBalanceOf(week()));
  return (
    <section class="week-mirror" aria-label="Wochenspiegel">
      <details open={weekMirrorOpen()} onToggle={(e) => setWeekMirrorOpen(e.currentTarget.open)}>
        <summary>
          <span class="week-mirror-title">Wochenspiegel</span>
          <span class="muted week-mirror-total">
            KW {Number(week().slice(6))} · {doneCountLabel(balance().total)}
          </span>
        </summary>
        <div class="week-mirror-body">
          <p class="week-mirror-line" data-line="weight">
            {weekWeightLine(balance())}
          </p>
          <Show when={balance().total > 0}>
            <BalanceBar kind="weight" counts={balance().by_weight} keys={WEIGHT_KEYS} labels={BALANCE_WEIGHT_LABELS} />
            <p class="week-mirror-line" data-line="for-whom">
              {weekForWhomLine(balance())}
            </p>
            <BalanceBar kind="for" counts={balance().by_for_whom} keys={FOR_WHOM_KEYS} labels={BALANCE_FOR_WHOM_LABELS} />
          </Show>
          <p class="muted week-mirror-hint">
            Erledigt seit Montag, nach Gewicht und Für wen. Dev-Projekte zählen nicht mit.
          </p>
        </div>
      </details>
    </section>
  );
}
