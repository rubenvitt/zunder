// Server settings (browser session only): the Timing bridge for flames. Stored
// settings override the server's environment variables field by field.

import { createMemo, createResource, createSignal, For, Show } from "solid-js";
import { Icon } from "../components/Icon";
import { isTauri } from "../platform/runtime";
import type { SettingSource, TimingProject, TimingSettings, TimingSettingsUpdate } from "../sync/types";
import { api } from "../state/store";
import { toast } from "../state/ui";

export function SettingsView() {
  return isTauri() ? <SettingsInApp /> : <ServerSettings />;
}

/** Tauri app: server settings need a browser session on the server. */
function SettingsInApp() {
  return (
    <div class="view">
      <header class="view-head">
        <h1>Einstellungen</h1>
      </header>
      <p class="muted">Die Einstellungen des Servers (z. B. Timing) änderst du in der Web-Oberfläche deines Zunder-Servers; dafür ist die Anmeldung im Browser nötig.</p>
      <a class="btn" href="#/verbindung">
        Zur Verbindung
      </a>
    </div>
  );
}

function ServerSettings() {
  return (
    <div class="view">
      <header class="view-head">
        <h1>Einstellungen</h1>
      </header>
      <p class="muted">Einstellungen dieses Zunder-Servers. Sie gelten sofort und haben Vorrang vor den Umgebungsvariablen des Servers.</p>
      <TimingSection />
    </div>
  );
}

function sourceText(source: SettingSource | null, variable: string): string {
  if (source === "settings") return "aus den Einstellungen";
  if (source === "env") return `aus ${variable}`;
  return "";
}

type ProjectList = { projects: TimingProject[] } | { error: string };

/** "Arbeit › Zunder": the project with its parents. */
function projectLabel(p: TimingProject): string {
  return (p.title_chain.length ? p.title_chain : [p.title]).join(" › ");
}

function TimingSection() {
  const [settings, { mutate }] = createResource(async (): Promise<TimingSettings | null> => {
    try {
      return await api().timingSettings();
    } catch (err) {
      toast(err instanceof Error ? err.message : "Timing-Einstellungen konnten nicht geladen werden.", "error");
      return null;
    }
  });
  // Timing's projects for the picker; loaded again when the token in effect changes.
  const [projectList] = createResource(
    () => {
      const s = settings();
      return s?.configured ? `${s.token_source}:${s.token_hint}` : false;
    },
    async (): Promise<ProjectList> => {
      try {
        const projects = await api().timingProjects();
        return { projects: [...projects].sort((a, b) => projectLabel(a).localeCompare(projectLabel(b), "de")) };
      } catch (err) {
        return { error: err instanceof Error ? err.message : "Timing-Projekte konnten nicht geladen werden." };
      }
    },
  );
  const projects = () => {
    const list = projectList();
    return list && "projects" in list ? list.projects : [];
  };
  const projectsError = () => {
    const list = projectList();
    return list && "error" in list ? list.error : null;
  };
  /** Title chain for a self link of a known project, otherwise the value itself. */
  const describe = (value: string) => {
    const p = projects().find((p) => p.self === value);
    return p ? projectLabel(p) : value;
  };
  const [token, setToken] = createSignal("");
  // null: the project field is untouched and shows the project in effect.
  const [project, setProject] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);

  const projectValue = () => project() ?? settings()?.project ?? "";
  const update = (): TimingSettingsUpdate => {
    const next: TimingSettingsUpdate = {};
    if (token().trim()) next.token = token().trim();
    const p = project();
    if (p !== null && p.trim() !== (settings()?.project ?? "")) next.project = p.trim();
    return next;
  };
  const dirty = () => Object.keys(update()).length > 0;

  function loaded(next: TimingSettings) {
    mutate(next);
    setToken("");
    setProject(null);
  }

  async function save(e: Event) {
    e.preventDefault();
    if (!dirty() || busy()) return;
    setBusy(true);
    try {
      loaded(await api().updateTimingSettings(update()));
      toast("Timing-Einstellungen gespeichert.");
    } catch (err) {
      toast(err instanceof Error ? err.message : "Speichern fehlgeschlagen.", "error");
    } finally {
      setBusy(false);
    }
  }

  async function check() {
    setBusy(true);
    try {
      const result = await api().checkTiming();
      if (result.ok) toast("Timing ist erreichbar und akzeptiert den Token.");
      else toast(result.error ?? "Timing lehnt die Verbindung ab.", "error");
    } catch (err) {
      toast(err instanceof Error ? err.message : "Prüfen fehlgeschlagen.", "error");
    } finally {
      setBusy(false);
    }
  }

  async function reset() {
    const s = settings();
    if (!s) return;
    const after = s.env_token ? "Danach gelten wieder die Umgebungsvariablen des Servers." : "Danach ist Timing aus.";
    if (!confirm(`Gespeicherte Timing-Einstellungen entfernen? ${after}`)) return;
    setBusy(true);
    try {
      loaded(await api().resetTimingSettings());
      toast("Timing-Einstellungen zurückgesetzt.");
    } catch (err) {
      toast(err instanceof Error ? err.message : "Zurücksetzen fehlgeschlagen.", "error");
    } finally {
      setBusy(false);
    }
  }

  return (
    <section class="group settings-section" aria-label="Timing">
      <h2>Timing</h2>
      <p class="muted">
        Flammen starten und stoppen einen Timer in Timing. Ohne Token speichern sie nur Start- und Endzeit. Den API-Token erstellst du in der Timing-Web-App.
      </p>
      <Show when={settings() !== null} fallback={<p class="muted">Timing-Einstellungen nicht verfügbar.</p>}>
        <Show when={settings()} fallback={<p class="muted">Lädt …</p>}>
          {(s) => (
            <>
              <p class="notice" classList={{ "notice-success": s().configured }} data-testid="timing-status">
                <Show when={s().configured} fallback="Nicht eingerichtet: Flammen nutzen Timing nicht.">
                  Eingerichtet mit Token <code>{s().token_hint}</code> ({sourceText(s().token_source, "ZUNDER_TIMING_TOKEN")})
                  <Show when={s().project} fallback=", ohne Projekt.">
                    {(p) => (
                      <>
                        , Projekt{" "}
                        <Show when={describe(p()) !== p()} fallback={<code>{p()}</code>}>
                          <strong>{describe(p())}</strong>
                        </Show>{" "}
                        ({sourceText(s().project_source, "ZUNDER_TIMING_PROJECT")}).
                      </>
                    )}
                  </Show>
                </Show>
              </p>
              <form class="form settings-form" onSubmit={save}>
                <label>
                  Timing-Token
                  <input
                    class="field"
                    type="password"
                    name="timing-token"
                    autocomplete="off"
                    spellcheck={false}
                    placeholder={s().configured ? `Leer lassen, um den Token ${s().token_hint ?? ""} zu behalten` : "Token aus der Timing-Web-App"}
                    value={token()}
                    onInput={(e) => setToken(e.currentTarget.value)}
                  />
                </label>
                {/* Not a wrapping label: clicks on the list must not reach the field. */}
                <div class="form-field">
                  <label for="timing-project">Timing-Projekt</label>
                  <ProjectCombo id="timing-project" value={projectValue()} projects={projects()} describe={describe} onChange={setProject} />
                  <span class="hint" data-testid="timing-project-hint">
                    <Show
                      when={projects().length > 0}
                      fallback={
                        <>
                          <Show when={projectsError()}>{(e) => <>Projektliste nicht geladen: {e()} </>}</Show>
                          Self-Link (z. B. /projects/1) oder Titel eines Timing-Projekts; leer = ohne Projekt.
                        </>
                      }
                    >
                      Aus der Liste wählen oder Titel eintippen; leer = ohne Projekt.
                      <Show when={projectValue() !== describe(projectValue())}>
                        {" "}
                        Gespeichert wird <code>{projectValue()}</code>.
                      </Show>
                    </Show>
                  </span>
                </div>
                <div class="inline-group">
                  <button class="btn btn-primary" type="submit" disabled={!dirty() || busy()}>
                    Speichern
                  </button>
                  <Show when={s().configured}>
                    <button class="btn" type="button" disabled={busy() || dirty()} onClick={() => void check()}>
                      <Icon name="sync" size={14} /> Verbindung prüfen
                    </button>
                  </Show>
                  <Show when={s().overridden}>
                    <button class="btn btn-ghost" type="button" disabled={busy()} onClick={() => void reset()}>
                      Zurücksetzen
                    </button>
                  </Show>
                </div>
              </form>
            </>
          )}
        </Show>
      </Show>
    </section>
  );
}

/**
 * Combo box for the Timing project: typing filters Timing's projects, choosing
 * one stores its self link, anything else typed is kept as a project title.
 */
function ProjectCombo(props: {
  id: string;
  value: string;
  projects: TimingProject[];
  describe: (value: string) => string;
  onChange: (value: string) => void;
}) {
  const id = props.id;
  // null: the field shows the value (a known project by its title chain).
  const [text, setText] = createSignal<string | null>(null);
  const [open, setOpen] = createSignal(false);
  // Highlighted option; -1: none (Enter then submits the form).
  const [active, setActive] = createSignal(-1);
  let listEl: HTMLUListElement | undefined;

  const shown = () => text() ?? props.describe(props.value);
  const matches = createMemo(() => {
    const q = (text() ?? "").trim().toLowerCase();
    if (!q) return props.projects;
    return props.projects.filter((p) => projectLabel(p).toLowerCase().includes(q));
  });
  const expanded = () => open() && props.projects.length > 0;

  function choose(p: TimingProject) {
    props.onChange(p.self);
    setText(null);
    setOpen(false);
  }

  function show() {
    if (!open()) setActive(-1);
    setOpen(true);
  }

  function move(delta: number) {
    show();
    const n = matches().length;
    if (n === 0) return;
    setActive((i) => (i < 0 ? (delta > 0 ? 0 : n - 1) : (i + delta + n) % n));
    listEl?.querySelector(`[data-index="${active()}"]`)?.scrollIntoView({ block: "nearest" });
  }

  return (
    <div class="combo">
      <input
        id={id}
        class="field"
        name="timing-project"
        role="combobox"
        autocomplete="off"
        spellcheck={false}
        aria-autocomplete="list"
        aria-expanded={expanded()}
        aria-controls={`${id}-list`}
        aria-activedescendant={expanded() && matches()[active()] ? `${id}-${active()}` : undefined}
        placeholder={props.projects.length ? "Projekt suchen …" : "/projects/1 oder Projekttitel"}
        value={shown()}
        onFocus={show}
        onClick={show}
        onBlur={() => {
          setOpen(false);
          setText(null);
        }}
        onInput={(e) => {
          const typed = e.currentTarget.value;
          setText(typed);
          setOpen(true);
          // While typing, Enter takes the best match.
          setActive(typed.trim() ? 0 : -1);
          // Typing a project's full title chain picks that project.
          const wanted = typed.trim().toLowerCase();
          const exact = props.projects.find((p) => projectLabel(p).toLowerCase() === wanted);
          props.onChange(exact ? exact.self : typed.trim());
        }}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") {
            e.preventDefault();
            move(1);
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            move(-1);
          } else if (e.key === "Enter" && expanded() && matches()[active()]) {
            e.preventDefault();
            choose(matches()[active()]!);
          } else if (e.key === "Escape" && expanded()) {
            e.preventDefault();
            e.stopPropagation();
            setOpen(false);
          }
        }}
      />
      <Show when={expanded()}>
        <ul class="combo-list" id={`${id}-list`} role="listbox" aria-label="Projekte aus Timing" ref={listEl}>
          <For each={matches()}>
            {(p, i) => (
              <li
                id={`${id}-${i()}`}
                data-index={i()}
                role="option"
                aria-selected={p.self === props.value}
                class="combo-option"
                classList={{ active: i() === active() }}
                // Keep the focus in the field, so blur does not close the list first.
                onMouseDown={(e) => e.preventDefault()}
                onMouseMove={() => setActive(i())}
                onClick={() => choose(p)}
              >
                <span class="combo-dot" style={{ background: p.color ?? "var(--border)" }} aria-hidden="true" />
                <span class="combo-label">{projectLabel(p)}</span>
                <Show when={p.self === props.value}>
                  <Icon name="check" size={14} />
                </Show>
              </li>
            )}
          </For>
          <Show when={matches().length === 0}>
            <li class="combo-empty" role="presentation">
              Kein Timing-Projekt passt; der Text wird als Projekttitel gespeichert.
            </li>
          </Show>
        </ul>
      </Show>
    </div>
  );
}
