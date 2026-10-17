import { createSignal, createUniqueId, For, Show } from "solid-js";
import { deferralsLabel, describeRecurrence } from "../labels";
import { cutAnew, dropTask, dropInstance, endSeries, handBack, handBackAlternative, letRest, writeDeferral } from "../state/actions";
import { data, now, taskIndex } from "../state/store";
import { formatInstant } from "../sync/dates";
import { isRecurring } from "../sync/derived";
import { handbackRemindAt, handbackSentence, HANDBACK_WORKDAYS, waitingNames } from "../sync/handback";
import {
  copyText,
  dismissToast,
  setDeferRequest,
  setDropRequest,
  setHandBackRequest,
  setHelpOpen,
  toasts,
  type DeferRequest,
  type DropRequest,
  type HandBackRequest,
} from "../state/ui";
import { Icon } from "./Icon";
import { Modal } from "./Modal";

export function Toasts() {
  return (
    <div class="toasts" role="status" aria-live="polite">
      <For each={toasts()}>
        {(t) => (
          <div class={`toast toast-${t.kind}`} classList={{ "toast-ask": !!t.actions?.length }} data-kind={t.kind}>
            <span>{t.message}</span>
            <Show when={t.actions?.length}>
              <div class="toast-actions">
                <For each={t.actions}>
                  {(action) => (
                    <button
                      class="btn btn-small toast-action"
                      onClick={() => {
                        dismissToast(t.id);
                        action.run();
                      }}
                    >
                      {action.label}
                    </button>
                  )}
                </For>
              </div>
            </Show>
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
  ["g w", "Wartet auf andere"],
  ["g z", "Offene Zusagen"],
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

/**
 * "Zurückgeben" (contract 5.1): Rauch goes back to whoever asked for it. The sentence to send
 * (Zunder copies it, the owner sends it), whom the task now waits on (required) and a reminder
 * in three working days (preselected). "Zurückgeben" copies the sentence and writes
 * `waiting_on`, `blocked` and `remind_at` in one action; Esc cancels.
 */
export function HandBackDialog(props: { request: HandBackRequest }) {
  const nameId = createUniqueId();
  const listId = createUniqueId();
  const sentenceId = createUniqueId();
  // Fixed when the dialog opens, so the text does not change while it is edited.
  const opened = data.tasks[props.request.taskId];
  const alternative = opened ? handBackAlternative(opened) : null;
  const [sentence, setSentence] = createSignal(opened ? handbackSentence(opened.title, alternative?.title ?? null) : "");
  const [name, setName] = createSignal("");
  const [remind, setRemind] = createSignal(true);
  const names = waitingNames([...taskIndex().byId.values()].filter((t) => !taskIndex().isDeleted(t)));
  const remindLabel = formatInstant(handbackRemindAt(new Date(now())), new Date(now()));
  const close = () => setHandBackRequest(null);
  const submit = (e: SubmitEvent) => {
    e.preventDefault();
    if (!name().trim()) return;
    // Read before closing: the dialog (and its props) go away with it.
    const { taskId } = props.request;
    const text = sentence().trim();
    close();
    handBack(taskId, name(), remind(), text);
  };
  return (
    <Modal label="Zurückgeben" onClose={close} class="handback">
      <form onSubmit={submit}>
        <h2>Zurückgeben</h2>
        <p class="muted">Rauch drängt, ist dir selbst aber nicht wichtig. Gib die Entscheidung an die Person zurück, die gefragt hat.</p>
        <label class="handback-label" for={sentenceId}>
          Satz zum Verschicken
        </label>
        <textarea id={sentenceId} class="field handback-sentence" rows={3} value={sentence()} onInput={(e) => setSentence(e.currentTarget.value)} />
        <div class="handback-row">
          <button type="button" class="btn btn-small" onClick={() => copyText(sentence().trim(), "Satz kopiert.")}>
            <Icon name="copy" size={14} />
            Kopieren
          </button>
          <span class="muted small">Zunder verschickt nichts.</span>
        </div>
        <label class="handback-label" for={nameId}>
          Wer entscheidet?
        </label>
        <input id={nameId} class="field" list={listId} required autofocus placeholder="Name" value={name()} onInput={(e) => setName(e.currentTarget.value)} />
        <datalist id={listId}>
          <For each={names}>{(n) => <option value={n} />}</For>
        </datalist>
        <label class="handback-check">
          <input type="checkbox" checked={remind()} onChange={(e) => setRemind(e.currentTarget.checked)} />
          In {HANDBACK_WORKDAYS} Werktagen erinnern
          <span class="muted small">({remindLabel})</span>
        </label>
        <div class="modal-actions">
          <button type="button" class="btn btn-ghost" onClick={close}>
            Abbrechen
          </button>
          <span class="spacer" />
          <button type="submit" class="btn btn-primary" disabled={!name().trim()} title="Kopiert den Satz und wartet auf die Antwort">
            Zurückgeben
          </button>
        </div>
      </form>
    </Modal>
  );
}
