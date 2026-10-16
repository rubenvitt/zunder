import { For, Show } from "solid-js";
import { deferralsLabel, describeRecurrence } from "../labels";
import { cutAnew, dropTask, dropInstance, endSeries, letRest, writeDeferral } from "../state/actions";
import { data } from "../state/store";
import { isRecurring } from "../sync/derived";
import { dismissToast, setDeferRequest, setDropRequest, setHelpOpen, toasts, type DeferRequest, type DropRequest } from "../state/ui";
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
  ["s / Shift+S", "Heute: heute nicht, auf morgen / nächste Woche"],
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

/**
 * The third "Heute nicht" (contract 5.1, Verschiebungs-Counter of the Phönix-Progression): a
 * task put off twice is not "not done yet" but a decision. Drop it, cut it anew (opens it, the
 * counter starts over) or let it rest without a date; moving it once more stays possible but
 * is the quiet option. Esc cancels.
 */
export function DeferDecisionDialog(props: { request: DeferRequest }) {
  const task = () => data.tasks[props.request.taskId];
  const close = () => setDeferRequest(null);
  // Read the request before closing: the dialog (and its props) go away with it.
  const run = (action: (request: DeferRequest) => unknown) => {
    const request = props.request;
    close();
    action(request);
  };
  return (
    <Modal label="Entscheiden statt verschieben" onClose={close} class="defer-decision">
      <h2>Entscheiden statt verschieben</h2>
      <Show when={task()}>
        {(t) => (
          <p>
            „{t().title}“ ist schon {deferralsLabel(t().deferrals)}. Ein weiterer Aufschub ändert wenig: Was ist diese Aufgabe wirklich?
          </p>
        )}
      </Show>
      <div class="decision-options">
        <button class="decision" onClick={() => run((r) => dropTask(r.taskId, r.before))}>
          <Icon name="trash" size={18} />
          <span>
            <strong>Streichen</strong>
            <small>Sie war eine Idee, keine Zusage.</small>
          </span>
        </button>
        <button class="decision" autofocus onClick={() => run((r) => cutAnew(r.taskId))}>
          <Icon name="pencil" size={18} />
          <span>
            <strong>Neu schneiden</strong>
            <small>Kleiner oder klarer fassen, damit sie anfangbar wird. Der Zähler beginnt neu.</small>
          </span>
        </button>
        <Show when={task() && !isRecurring(task()!)}>
          <button class="decision" onClick={() =>
            run((r) => {
              r.before?.();
              letRest(r.taskId);
            })
          }>
            <Icon name="archive" size={18} />
            <span>
              <strong>Ruhen lassen</strong>
              <small>Ohne Datum, bewusst undatiert. In der Aschezeit fragen, was sie eigentlich ist.</small>
            </span>
          </button>
        </Show>
      </div>
      <div class="modal-actions">
        <button class="btn btn-ghost" onClick={close}>
          Abbrechen
        </button>
        <span class="spacer" />
        <button class="btn btn-ghost" onClick={() => run((r) => writeDeferral(r.taskId, r.target, r.before))}>
          Trotzdem verschieben
        </button>
      </div>
    </Modal>
  );
}
