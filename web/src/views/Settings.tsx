// Server settings (browser session only): the Timing bridge for flames (stored
// settings override the server's environment variables field by field) and the
// calendar feed of flames and timed tasks, Jev for inbox proposals, plus the
// link to the agent log.

import { createMemo, createResource, createSignal, For, Show } from "solid-js";
import { Icon } from "../components/Icon";
import { isTauri } from "../platform/runtime";
import { formatInstant } from "../sync/dates";
import type { CalendarSettings, CreatedCalendarFeed, JevSettings, JevSettingsUpdate, SettingSource, SuggestionTally, TimingProject, TimingSettings, TimingSettingsUpdate } from "../sync/types";
import { setJevConfigured } from "../state/jev";
import { api, areaList, now } from "../state/store";
import { copyText, toast } from "../state/ui";

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
      <JevSection />
      <CalendarSection />
      <section class="group settings-section" aria-label="KI-Protokoll">
        <h2>KI-Protokoll</h2>
        <p class="muted">Was Agenten über MCP (z. B. Claude) in Zunder geändert haben: angelegte, geänderte und gelöschte Aufgaben, Projekte, Bereiche und Kommentare.</p>
        <a class="btn" href="#/ki-protokoll">
          <Icon name="history" size={14} /> Protokoll öffnen
        </a>
      </section>
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

function tallyText(label: string, t: SuggestionTally): string {
  if (t.decided === 0) return `${label}: noch nichts entschieden`;
  return `${label}: ${t.followed} von ${t.decided} übernommen`;
}

/** Jev: key, pinned version and the areas that never leave the server. */
function JevSection() {
  const [settings, { mutate }] = createResource(async (): Promise<JevSettings | null> => {
    try {
      return await api().jevSettings();
    } catch (err) {
      toast(err instanceof Error ? err.message : "Jev-Einstellungen konnten nicht geladen werden.", "error");
      return null;
    }
  });
  const [token, setToken] = createSignal("");
  // null: untouched, the value in effect applies.
  const [model, setModel] = createSignal<string | null>(null);
  const [privateIds, setPrivateIds] = createSignal<string[] | null>(null);
  const [busy, setBusy] = createSignal(false);

  const activeAreas = createMemo(() => areaList().filter((a) => a.state === "active"));
  const privateValue = () => privateIds() ?? settings()?.private_area_ids ?? [];
  const samePrivate = (a: string[], b: string[]) => a.length === b.length && a.every((id) => b.includes(id));
  const update = (): JevSettingsUpdate => {
    const next: JevSettingsUpdate = {};
    const s = settings();
    if (token().trim()) next.token = token().trim();
    const m = model();
    if (m !== null && m.trim() !== (s?.model ?? "")) next.model = m.trim();
    const p = privateIds();
    if (p !== null && !samePrivate(p, s?.private_area_ids ?? [])) next.private_area_ids = p;
    return next;
  };
  const dirty = () => Object.keys(update()).length > 0;

  function loaded(next: JevSettings) {
    mutate(next);
    setJevConfigured(next.configured);
    setToken("");
    setModel(null);
    setPrivateIds(null);
  }

  function togglePrivate(id: string, on: boolean) {
    const current = privateValue().filter((x) => x !== id);
    setPrivateIds(on ? [...current, id] : current);
  }

  async function save(e: Event) {
    e.preventDefault();
    if (!dirty() || busy()) return;
    setBusy(true);
    try {
      loaded(await api().updateJevSettings(update()));
      toast("Jev-Einstellungen gespeichert.");
    } catch (err) {
      toast(err instanceof Error ? err.message : "Speichern fehlgeschlagen.", "error");
    } finally {
      setBusy(false);
    }
  }

  async function reset() {
    if (!confirm("Jev-Key, Version und private Bereiche entfernen? Gespeicherte Vorschläge bleiben, neue gibt es erst mit neuem Key.")) return;
    setBusy(true);
    try {
      loaded(await api().resetJevSettings());
      toast("Jev ist aus.");
    } catch (err) {
      toast(err instanceof Error ? err.message : "Zurücksetzen fehlgeschlagen.", "error");
    } finally {
      setBusy(false);
    }
  }

  return (
    <section class="group settings-section" aria-label="Jev">
      <h2>Jev (Vorschläge für die Inbox)</h2>
      <p class="muted">
        Jev ist ein Entscheidungsmodell von TypeSafe AI. In der Detailansicht einer Inbox-Aufgabe holst du auf Knopfdruck einen Vorschlag für Gewicht, „für wen“ und Ort.
        Dabei gehen Titel, Beschreibung und die Namen deiner aktiven Projekte und Bereiche an Jev; private Bereiche nie. Übernommen wird nur, was du anklickst.
      </p>
      <Show when={settings() !== null} fallback={<p class="muted">Jev-Einstellungen nicht verfügbar.</p>}>
        <Show when={settings()} fallback={<p class="muted">Lädt …</p>}>
          {(s) => (
            <>
              <p class="notice" classList={{ "notice-success": s().configured }} data-testid="jev-status">
                <Show when={s().configured} fallback="Nicht eingerichtet: keine Vorschläge.">
                  Eingerichtet mit Key <code>{s().token_hint}</code>, Version <code>{s().model}</code>.
                </Show>
              </p>
              <Show when={s().stats.asked > 0}>
                <p class="muted jev-stats" data-testid="jev-stats">
                  {s().stats.asked} Vorschläge, {s().stats.input_tokens} Eingabe-Tokens. {tallyText("Gewicht", s().stats.weight)}; {tallyText("für wen", s().stats.for_whom)};{" "}
                  {tallyText("Ort", s().stats.place)}.
                </p>
              </Show>
              <form class="form settings-form" onSubmit={save}>
                <label>
                  Jev-Key
                  <input
                    class="field"
                    type="password"
                    name="jev-token"
                    autocomplete="off"
                    spellcheck={false}
                    placeholder={s().configured ? `Leer lassen, um den Key ${s().token_hint ?? ""} zu behalten` : "API-Key von TypeSafe AI"}
                    value={token()}
                    onInput={(e) => setToken(e.currentTarget.value)}
                  />
                </label>
                <label>
                  Version
                  <input
                    class="field"
                    name="jev-model"
                    autocomplete="off"
                    spellcheck={false}
                    placeholder={s().default_model}
                    value={model() ?? (s().model === s().default_model ? "" : s().model)}
                    onInput={(e) => setModel(e.currentTarget.value)}
                  />
                  <span class="hint">Feste Version, leer = {s().default_model}. Aliase wie jev-latest lehnt der Server ab.</span>
                </label>
                <fieldset class="form-field jev-fieldset">
                  <legend>Private Bereiche (gehen nie an Jev)</legend>
                  <div class="jev-areas">
                    <For each={activeAreas()} fallback={<span class="muted">Keine aktiven Bereiche.</span>}>
                      {(a) => (
                        <label class="toggle small">
                          <input type="checkbox" checked={privateValue().includes(a.id)} onChange={(e) => togglePrivate(a.id, e.currentTarget.checked)} />
                          {a.name}
                        </label>
                      )}
                    </For>
                  </div>
                </fieldset>
                <div class="inline-group">
                  <button class="btn btn-primary" type="submit" disabled={!dirty() || busy()}>
                    Speichern
                  </button>
                  <Show when={s().configured || s().private_area_ids.length > 0 || s().model !== s().default_model}>
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

function CalendarSection() {
  const [settings, { mutate }] = createResource(async (): Promise<CalendarSettings | null> => {
    try {
      return await api().calendarSettings();
    } catch (err) {
      toast(err instanceof Error ? err.message : "Kalender-Einstellungen konnten nicht geladen werden.", "error");
      return null;
    }
  });
  // The feed URLs contain the token and come only with the answer that created it.
  const [created, setCreated] = createSignal<CreatedCalendarFeed | null>(null);
  const [busy, setBusy] = createSignal(false);

  async function create() {
    if (settings()?.enabled && !confirm("Neue Feed-Adresse erzeugen? Die bisherige Adresse funktioniert danach nicht mehr; Kalender-Abos musst du neu einrichten.")) return;
    setBusy(true);
    try {
      const feed = await api().createCalendarFeed();
      setCreated(feed);
      mutate({ enabled: feed.enabled, created_at: feed.created_at });
    } catch (err) {
      toast(err instanceof Error ? err.message : "Feed konnte nicht erzeugt werden.", "error");
    } finally {
      setBusy(false);
    }
  }

  async function disable() {
    if (!confirm("Kalender-Feed abschalten? Abonnierte Kalender erhalten danach keine Termine mehr.")) return;
    setBusy(true);
    try {
      mutate(await api().disableCalendarFeed());
      setCreated(null);
      toast("Kalender-Feed abgeschaltet.");
    } catch (err) {
      toast(err instanceof Error ? err.message : "Abschalten fehlgeschlagen.", "error");
    } finally {
      setBusy(false);
    }
  }

  return (
    <section class="group settings-section" aria-label="Kalender">
      <h2>Kalender</h2>
      <p class="muted">
        Ein Feed zum Abonnieren in Fantastical, Apple Kalender oder Google Kalender: Flammen mit Uhrzeit als Termin von zwei Stunden, auf Wunsch auch alle anderen Aufgaben mit Uhrzeit (30 Minuten, als frei markiert). Erledigte Aufgaben behalten ein ✓, verworfene fallen heraus. Die Adresse enthält einen eigenen Schlüssel, der nur den Feed liest.
      </p>
      <Show when={settings() !== null} fallback={<p class="muted">Kalender-Einstellungen nicht verfügbar.</p>}>
        <Show when={settings()} fallback={<p class="muted">Lädt …</p>}>
          {(s) => (
            <>
              <p class="notice" classList={{ "notice-success": s().enabled }} data-testid="calendar-status">
                <Show when={s().enabled} fallback="Kein Feed eingerichtet.">
                  Feed eingerichtet.
                  <Show when={s().created_at}>{(at) => <> Adresse erzeugt: {formatInstant(at(), new Date(now()))}.</>}</Show>
                </Show>
              </p>
              <Show when={created()}>
                {(c) => (
                  <div class="notice notice-success new-token">
                    <p>Die Adressen werden nur jetzt angezeigt. Zum Abonnieren in die Kalender-App kopieren:</p>
                    <FeedUrl label="Nur Flammen" url={c().url} testId="calendar-url" />
                    <FeedUrl label="Flammen und alle Aufgaben mit Uhrzeit" url={c().url_all} testId="calendar-url-all" />
                    <div class="inline-group">
                      <button class="btn btn-ghost btn-small" type="button" onClick={() => setCreated(null)}>
                        Ausblenden
                      </button>
                    </div>
                  </div>
                )}
              </Show>
              <div class="inline-group">
                <button class="btn" classList={{ "btn-primary": !s().enabled }} type="button" disabled={busy()} onClick={() => void create()}>
                  <Icon name="calendar" size={14} /> {s().enabled ? "Neue Adresse erzeugen" : "Feed einrichten"}
                </button>
                <Show when={s().enabled}>
                  <button class="btn btn-ghost" type="button" disabled={busy()} onClick={() => void disable()}>
                    Abschalten
                  </button>
                </Show>
              </div>
            </>
          )}
        </Show>
      </Show>
    </section>
  );
}

function FeedUrl(props: { label: string; url: string; testId: string }) {
  return (
    <div class="feed-url">
      <span class="hint">{props.label}</span>
      <code class="token-value" data-testid={props.testId}>
        {props.url}
      </code>
      <button class="btn btn-small" type="button" aria-label={`${props.label}: Adresse kopieren`} onClick={() => copyText(props.url, "Feed-Adresse kopiert.")}>
        <Icon name="copy" size={14} /> Kopieren
      </button>
    </div>
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
