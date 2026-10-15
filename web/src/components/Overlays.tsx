import { For, Show } from "solid-js";
import { describeRecurrence } from "../labels";
import { dropInstance, endSeries } from "../state/actions";
import { data } from "../state/store";
import { dismissToast, setDropRequest, setHelpOpen, toasts, type DropRequest } from "../state/ui";
import { Icon } from "./Icon";
import { Modal } from "./Modal";

export function Toasts() {
  return (
    <div class="toasts" role="status" aria-live="polite">
      <For each={toasts()}>
        {(t) => (
          <div class={`toast toast-${t.kind}`} data-kind={t.kind}>
            <span>{t.message}</span>
            <button class="btn btn-ghost btn-icon" aria-label="Meldung schließen" onClick={() => dismissToast(t.id)}>
              <Icon name="x" size={14} />
            </button>
          </div>
        )}
      </For>
    </div>
  );
}

const SHORTCUTS: [string, string][] = [
  ["n", "Neue Aufgabe erfassen"],
  ["⌘K / Strg+K, /", "Suche und Befehle"],
  ["⌘B / Strg+B", "Seitenleiste ein- und ausblenden"],
  ["⌘, / Strg+,", "Einstellungen öffnen"],
  ["j / k", "Nächste / vorige Aufgabe"],
  ["Enter", "Aufgabe öffnen"],
  ["x", "Erledigt umschalten"],
  ["y", "Kennung der Aufgabe (z. B. ZUN-42) kopieren, für Agenten"],
  ["Esc", "Panel oder Dialog schließen"],
  ["a / l / d", "Inbox: zuordnen / lose / verwerfen"],
  ["h / l bzw. ← / →", "Board: Spalte wechseln"],
  ["Shift+H / Shift+L", "Board: Karte in andere Spalte"],
  ["Shift+J / Shift+K", "Board: Karte nach unten / oben"],
  ["v", "Projekt: Liste/Board umschalten"],
  ["g h", "Heute"],
  ["g i", "Inbox"],
  ["g l", "Lose"],
  ["g b", "Bereiche"],
  ["g d", "Dev"],
  ["g p", "Phönix"],
  ["?", "Diese Übersicht"],
];

export function ShortcutHelp() {
  return (
    <Modal label="Tastenkürzel" onClose={() => setHelpOpen(false)} class="help">
      <h2>Tastenkürzel</h2>
      <dl class="shortcuts">
        <For each={SHORTCUTS}>
          {([keys, label]) => (
            <>
              <dt>
                <kbd>{keys}</kbd>
              </dt>
              <dd>{label}</dd>
            </>
          )}
        </For>
      </dl>
      <div class="modal-actions">
        <a class="small muted credit" href="https://icons8.com" target="_blank" rel="noopener noreferrer">
          Icons von Icons8
        </a>
        <span class="spacer" />
        <button class="btn btn-primary" onClick={() => setHelpOpen(false)}>
          Schließen
        </button>
      </div>
    </Modal>
  );
}

/**
 * Dropping a recurring task (contract 5.1): "Nur diese verwerfen" (default, Enter) lets the
 * server create the next instance, "Serie beenden" drops it and removes the rhythm, Esc cancels.
 */
export function DropSeriesDialog(props: { request: DropRequest }) {
  const task = () => data.tasks[props.request.taskId];
  const close = () => setDropRequest(null);
  const run = (action: (id: string) => boolean) => {
    // Read the request before closing: the dialog (and its props) go away with it.
    const { taskId, before } = props.request;
    close();
    before?.();
    action(taskId);
  };
  return (
    <Modal label="Wiederkehrende Aufgabe verwerfen" onClose={close} class="drop-series">
      <h2>Wiederkehrende Aufgabe verwerfen</h2>
      <Show when={task()}>
        {(t) => (
          <p>
            „{t().title}“ wiederholt sich ({t().recurrence ? describeRecurrence(t().recurrence!, t().due) : "Rhythmus"}). Nur diese Instanz verwerfen, dann folgt
            die nächste, oder die ganze Serie beenden?
          </p>
        )}
      </Show>
      <div class="modal-actions">
        <button class="btn btn-ghost" onClick={close}>
          Abbrechen
        </button>
        <span class="spacer" />
        <button class="btn" onClick={() => run(endSeries)}>
          Serie beenden
        </button>
        <button class="btn btn-primary" autofocus onClick={() => run(dropInstance)}>
          Nur diese verwerfen
        </button>
      </div>
    </Modal>
  );
}
