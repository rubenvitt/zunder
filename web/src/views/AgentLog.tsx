// Agent log: what agents (MCP, e.g. Claude) changed, newest first. Deliberately
// tucked away: reachable from the settings and the command palette only.

import { createMemo, createSignal, For, onMount, Show } from "solid-js";
import { describeChange } from "../agent-log";
import { Icon } from "../components/Icon";
import { sourceLabel } from "../labels";
import { api, now } from "../state/store";
import { formatInstant } from "../sync/dates";
import type { AgentAction } from "../sync/types";

export function AgentLogView() {
  const [actions, setActions] = createSignal<AgentAction[]>([]);
  const [nextBefore, setNextBefore] = createSignal<number | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [failed, setFailed] = createSignal<string | null>(null);

  async function load(before?: number) {
    setLoading(true);
    setFailed(null);
    try {
      const page = await api().agentLog(before);
      setActions((prev) => (before === undefined ? page.actions : [...prev, ...page.actions]));
      setNextBefore(page.next_before);
    } catch (err) {
      setFailed(err instanceof Error ? err.message : "Das Protokoll konnte nicht geladen werden.");
    } finally {
      setLoading(false);
    }
  }

  onMount(() => void load());

  return (
    <div class="view">
      <header class="view-head">
        <h1>KI-Protokoll</h1>
        <button class="btn btn-ghost btn-small" type="button" disabled={loading()} onClick={() => void load()}>
          <Icon name="sync" size={14} /> Neu laden
        </button>
      </header>
      <p class="muted">
        Was Agenten über MCP (z. B. Claude) in Zunder geändert haben, neueste zuerst. Ein Eintrag ist ein Werkzeugaufruf. Was sie nur gelesen haben, steht hier nicht. Die Änderungen einer einzelnen Aufgabe zeigt auch ihr Änderungsprotokoll.
      </p>
      <Show when={failed()}>{(message) => <p class="notice notice-error">{message()}</p>}</Show>
      <ul class="agent-log" data-testid="agent-log">
        <For each={actions()} fallback={<li class="muted">{loading() ? "Lädt …" : failed() ? "" : "Noch keine Änderungen durch Agenten."}</li>}>
          {(action) => (
            <li class="agent-action">
              <div class="agent-action-head">
                <span class="history-time">{formatInstant(action.received_at, new Date(now()))}</span>
                <span class="history-source" title={action.source}>
                  {sourceLabel(action.source)}
                </span>
              </div>
              <ul class="agent-changes">
                <For each={action.changes}>
                  {(change) => {
                    // Names come from the store, which may still be loading.
                    const s = createMemo(() => describeChange(change));
                    return (
                      <li>
                        <Icon name={s().icon} size={14} />
                        <span>
                          {s().lead}{" "}
                          <Show when={s().href} fallback={<strong>„{s().name}“</strong>}>
                            {(href) => (
                              <a href={href()}>
                                <strong>„{s().name}“</strong>
                              </a>
                            )}
                          </Show>{" "}
                          {s().verb}
                          <Show when={s().details.length > 0}>
                            <span class="agent-details">{s().details.join(" · ")}</span>
                          </Show>
                        </span>
                      </li>
                    );
                  }}
                </For>
              </ul>
            </li>
          )}
        </For>
      </ul>
      <Show when={nextBefore()}>
        {(before) => (
          <button class="btn" type="button" disabled={loading()} onClick={() => void load(before())}>
            Ältere laden
          </button>
        )}
      </Show>
    </div>
  );
}
