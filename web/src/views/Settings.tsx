// Server settings (browser session only): the Timing bridge for flames. Stored
// settings override the server's environment variables field by field.

import { createResource, createSignal, Show } from "solid-js";
import { Icon } from "../components/Icon";
import { isTauri } from "../platform/runtime";
import type { SettingSource, TimingSettings, TimingSettingsUpdate } from "../sync/types";
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

function TimingSection() {
  const [settings, { mutate }] = createResource(async (): Promise<TimingSettings | null> => {
    try {
      return await api().timingSettings();
    } catch (err) {
      toast(err instanceof Error ? err.message : "Timing-Einstellungen konnten nicht geladen werden.", "error");
      return null;
    }
  });
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
                        , Projekt <code>{p()}</code> ({sourceText(s().project_source, "ZUNDER_TIMING_PROJECT")}).
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
                <label>
                  Timing-Projekt
                  <input
                    class="field"
                    name="timing-project"
                    placeholder="/projects/1 oder Projekttitel"
                    value={projectValue()}
                    onInput={(e) => setProject(e.currentTarget.value)}
                  />
                  <span class="hint">Self-Link (z. B. /projects/1) oder Titel eines Timing-Projekts; leer = ohne Projekt.</span>
                </label>
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
