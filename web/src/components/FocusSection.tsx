// "Erst für dich" at the top of Heute (contract 5.1): pay yourself first, also with time. One
// Glut task for oneself; without one a quiet line and a few Glut tasks to pick for today.
// Clicking it away holds for the day (server setting, state/focus).

import { createMemo, For, Show } from "solid-js";
import { chosenFor, isDismissed } from "../sync/focus";
import { focusOf } from "../state/selectors";
import { chooseFocus, dismissFocus, focusSettings, focusToday, undismissFocus } from "../state/focus";
import { askToast, openTask } from "../state/ui";
import { FOCUS_EMPTY } from "../labels";
import { Icon } from "./Icon";
import { CheckButton, DueChip } from "./TaskBits";

export function FocusSection() {
  const today = createMemo(focusToday);
  const focus = createMemo(() => focusOf(today(), chosenFor(focusSettings(), today())));
  const dismiss = () => {
    dismissFocus();
    askToast("„Erst für dich“ ist für heute ausgeblendet.", [{ label: "Rückgängig", run: undismissFocus }], 6000);
  };
  return (
    <Show when={!isDismissed(focusSettings(), today())}>
      <section class="focus" classList={{ "focus-quiet": !focus().task }} aria-labelledby="focus-heading">
        <div class="focus-head">
          <h2 id="focus-heading">Erst für dich</h2>
          <button type="button" class="btn btn-ghost btn-icon btn-xs focus-dismiss" aria-label="Für heute ausblenden" title="Für heute ausblenden" onClick={dismiss}>
            <Icon name="x" size={12} />
          </button>
        </div>
        <Show
          when={focus().task}
          fallback={
            <div class="focus-empty">
              <p class="muted">{FOCUS_EMPTY}</p>
              <Show when={focus().choices.length > 0}>
                <div class="focus-choices" role="group" aria-label="Glut für heute wählen">
                  <span class="muted">Für heute wählen:</span>
                  <For each={focus().choices}>
                    {(task) => (
                      <button type="button" class="chip focus-choice" title="Für heute wählen" onClick={() => chooseFocus(task.id)}>
                        {task.title}
                      </button>
                    )}
                  </For>
                </div>
              </Show>
            </div>
          }
        >
          {(task) => (
            <div class="focus-task" data-task-id={task().id}>
              <CheckButton task={task()} />
              <button type="button" class="link-button focus-title" onClick={() => openTask(task().id)}>
                {task().title}
              </button>
              <span class="chip chip-weight chip-weight-glut">
                <span class="weight-dot" aria-hidden="true" />
                Glut
              </span>
              <DueChip task={task()} />
              <Show when={focus().chosen}>
                <span class="muted focus-note">für heute gewählt</span>
              </Show>
            </div>
          )}
        </Show>
      </section>
    </Show>
  );
}
