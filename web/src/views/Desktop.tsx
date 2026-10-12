// UI of the Tauri app, loaded on demand (App.tsx): start-up without OIDC
// login, the connection settings (server URL + API token; setup screen on
// first start and "Verbindung" view) and the status of the app's own sync.

import { createSignal, Match, onMount, Show, Switch, type Component } from "solid-js";
import { Icon } from "../components/Icon";
import { appConfig, appSyncing, desktopSync, initDesktop, isConfigured, saveConnection, syncNow, type ActionResult } from "../state/desktop";
import { now, pendingCount, ready, startSync } from "../state/store";
import { toast } from "../state/ui";
import { formatInstant } from "../sync/dates";

const URL_PATTERN = /^https?:\/\/[^\s/?#]+/i;

type DesktopPhase = "loading" | "setup" | "ready" | "error";

/** Start-up inside the app: data comes from the app's local store, no login. */
export function DesktopApp(props: { shell: Component; onReady: () => void }) {
  const [phase, setPhase] = createSignal<DesktopPhase>("loading");
  const [failure, setFailure] = createSignal("");
  const Shell = props.shell;

  async function open() {
    await startSync();
    setPhase("ready");
  }

  onMount(async () => {
    try {
      const config = await initDesktop();
      if (isConfigured(config)) await open();
      else setPhase("setup");
    } catch (err) {
      setFailure(err instanceof Error ? err.message : String(err));
      setPhase("error");
    }
    props.onReady();
  });

  return (
    <Switch fallback={<div class="splash" aria-busy="true" />}>
      <Match when={phase() === "setup"}>
        <ConnectionView setup onConnected={() => void open()} />
      </Match>
      <Match when={phase() === "ready" && ready()}>
        <Shell />
      </Match>
      <Match when={phase() === "error"}>
        <div class="splash">
          <p class="notice notice-error">Die App konnte ihre lokalen Daten nicht laden: {failure()}</p>
        </div>
      </Match>
    </Switch>
  );
}

/** Status of the app's background sync with the server; links to the settings. */
export function DesktopSyncIndicator() {
  const status = () => desktopSync();
  const label = () => {
    const pending = pendingCount();
    switch (status().state) {
      case "offline":
        return pending > 0 ? `Offline · ${pending} ausstehend` : "Offline";
      case "unauthorized":
        return "Token abgelehnt";
      case "unconfigured":
        return "Nicht verbunden";
      case "error":
        return "Sync-Fehler";
      default:
        return pending > 0 ? "Synchronisiert …" : "Synchron";
    }
  };
  const problem = () => status().state !== "ok";
  return (
    <a href="#/verbindung" class="sync" classList={{ offline: problem(), pending: pendingCount() > 0 }} data-sync={status().state} title={status().message ?? label()}>
      <Show when={problem()} fallback={<span class="sync-dot" />}>
        <Icon name="offline" size={14} />
      </Show>
      <span class="sync-label">{label()}</span>
    </a>
  );
}

export function ConnectionView(props: { setup?: boolean; onConnected?: () => void }) {
  const [serverUrl, setServerUrl] = createSignal(appConfig()?.server_url ?? "");
  const [token, setToken] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [result, setResult] = createSignal<ActionResult | null>(null);
  const [problem, setProblem] = createSignal("");

  const hasToken = () => !!appConfig()?.has_api_token;

  async function save(discardPending = false) {
    setProblem("");
    if (!URL_PATTERN.test(serverUrl().trim())) {
      setProblem("Bitte die Adresse des Servers mit http:// oder https:// angeben.");
      return;
    }
    if (!token().trim() && !hasToken()) {
      setProblem("Bitte einen API-Token eintragen.");
      return;
    }
    setBusy(true);
    try {
      const outcome = await saveConnection({ serverUrl: serverUrl(), token: token(), discardPending });
      setResult(outcome);
      if (outcome.saved !== false) setToken("");
      if (outcome.ok) {
        toast("Verbunden und abgeglichen.");
        props.onConnected?.();
      }
    } finally {
      setBusy(false);
    }
  }

  async function syncNowClicked() {
    const outcome = await syncNow();
    setResult(outcome);
    if (outcome.ok) toast("Abgeglichen.");
  }

  const lastSyncText = () => {
    const last = appConfig()?.last_sync;
    if (!last) return "Noch nie abgeglichen.";
    const when = formatInstant(new Date(last.at_ms).toISOString(), new Date(now()));
    return last.ok ? `Zuletzt abgeglichen: ${when}` : `Letzter Versuch ${when} fehlgeschlagen: ${last.error ?? "unbekannter Fehler"}`;
  };

  const form = (
    <form
      class="form connection-form"
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <label>
        Server-Adresse
        <input
          class="field"
          type="url"
          inputmode="url"
          autocomplete="url"
          placeholder="https://zunder.example.org"
          value={serverUrl()}
          onInput={(e) => setServerUrl(e.currentTarget.value)}
        />
      </label>
      <label>
        API-Token
        <input
          class="field"
          type="password"
          autocomplete="off"
          placeholder={hasToken() ? "Gespeichert – leer lassen, um ihn zu behalten" : "zt_…"}
          value={token()}
          onInput={(e) => setToken(e.currentTarget.value)}
        />
      </label>
      <Show when={problem()}>
        <p class="notice notice-error" role="alert">
          {problem()}
        </p>
      </Show>
      <Show when={result()}>
        {(r) => (
          <Show when={!r().ok}>
            <Show
              when={r().code === "pending_ops"}
              fallback={
                <div class="notice notice-error" role="alert">
                  <p>{errorText(r())}</p>
                  <Show when={props.setup && r().saved}>
                    <button type="button" class="btn btn-small" onClick={() => props.onConnected?.()}>
                      Trotzdem öffnen
                    </button>
                  </Show>
                </div>
              }
            >
              <div class="notice notice-warning" role="alert">
                <p>{r().message} Beim Wechsel des Servers gehen sie verloren.</p>
                <button type="button" class="btn btn-danger btn-small" disabled={busy()} onClick={() => void save(true)}>
                  Änderungen verwerfen und wechseln
                </button>
              </div>
            </Show>
          </Show>
        )}
      </Show>
      <button class="btn btn-primary" type="submit" disabled={busy()}>
        {busy() ? "Verbinde …" : "Speichern und verbinden"}
      </button>
    </form>
  );

  return (
    <Show
      when={!props.setup}
      fallback={
        <main class="login">
          <div class="login-card setup-card">
            <div class="logo" aria-hidden="true">
              <Icon name="flame" size={40} />
            </div>
            <h1>Zunder einrichten</h1>
            <p class="muted">
              Die App speichert deine Aufgaben auf diesem Gerät und gleicht sie im Hintergrund mit deinem Zunder-Server ab. Einen API-Token erstellst du in der Web-Oberfläche unter „API-Tokens“.
            </p>
            {form}
          </div>
        </main>
      }
    >
      <div class="view">
        <header class="view-head">
          <h1>Verbindung</h1>
        </header>
        <p class="muted">Server und API-Token dieser App. Änderungen werden lokal gespeichert und im Hintergrund abgeglichen.</p>
        {form}
        <section class="detail-section connection-status" aria-label="Abgleich">
          <h3>Abgleich</h3>
          <p data-testid="last-sync">{lastSyncText()}</p>
          <Show when={desktopSync().state !== "ok" && desktopSync().message}>
            <p class="notice notice-warning">{desktopSync().message}</p>
          </Show>
          <p class="muted">
            {pendingCount() > 0 ? `${pendingCount()} Änderungen warten auf die Übertragung.` : "Alle Änderungen sind übertragen."} Gerät: <code>{appConfig()?.node_id}</code>
          </p>
          <Show when={isConfigured()}>
            <button class="btn" disabled={appSyncing()} onClick={() => void syncNowClicked()}>
              <Icon name="sync" size={14} /> {appSyncing() ? "Gleicht ab …" : "Jetzt abgleichen"}
            </button>
          </Show>
        </section>
      </div>
    </Show>
  );
}

function errorText(result: ActionResult): string {
  switch (result.code) {
    case "unauthorized":
      return "Der Server lehnt den API-Token ab. Bitte einen gültigen Token eintragen.";
    case "network":
      return `${result.message ?? "Server nicht erreichbar."}${result.saved ? " Die Einstellungen sind gespeichert; die App versucht es weiter." : ""}`;
    default:
      return result.message ?? "Unbekannter Fehler.";
  }
}
