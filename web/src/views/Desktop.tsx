// UI of the Tauri app, loaded on demand (App.tsx): start-up without OIDC
// login, the settings (connection: login in the system browser or server URL +
// API token, setup screen on first start; the app itself: version, start at
// login, updates, test notification) and the status of the app's own sync.

import { createEffect, createSignal, Match, on, onCleanup, onMount, Show, Switch, type Component } from "solid-js";
import { Icon } from "../components/Icon";
import { Logo } from "../components/Logo";
import {
  appConfig,
  appInfo,
  appSyncing,
  checkUpdate,
  defaultDeviceName,
  desktopSync,
  initDesktop,
  isConfigured,
  loadAppInfo,
  loginState,
  reloadConfig,
  restartApp,
  saveConnection,
  setAutostart,
  startLogin,
  syncNow,
  testNotification,
  type ActionResult,
} from "../state/desktop";
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

/** Problems with the app's background sync (offline, rejected token, …); links to the settings. Hidden while all is well. */
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
      default:
        return "Sync-Fehler";
    }
  };
  return (
    <Show when={status().state !== "ok"}>
      <a href="#/verbindung" class="sync" data-sync={status().state} title={status().message ?? label()}>
        <Icon name="offline" size={14} />
        <span class="sync-label">{label()}</span>
      </a>
    </Show>
  );
}

export function ConnectionView(props: { setup?: boolean; onConnected?: () => void }) {
  const [serverUrl, setServerUrl] = createSignal(appConfig()?.server_url ?? "");
  const [deviceName, setDeviceName] = createSignal(defaultDeviceName());
  const [token, setToken] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [result, setResult] = createSignal<ActionResult | null>(null);
  const [problem, setProblem] = createSignal("");
  // Which action "Änderungen verwerfen und …" repeats.
  const [retry, setRetry] = createSignal<"login" | "token">("login");

  const hasToken = () => !!appConfig()?.has_api_token;

  function urlOk(): boolean {
    if (URL_PATTERN.test(serverUrl().trim())) return true;
    setProblem("Bitte die Adresse des Servers mit http:// oder https:// angeben.");
    return false;
  }

  async function signIn(discardPending = false) {
    setProblem("");
    setResult(null);
    setRetry("login");
    if (!urlOk()) return;
    if (!deviceName().trim()) {
      setProblem("Bitte einen Gerätenamen angeben.");
      return;
    }
    setBusy(true);
    try {
      const outcome = await startLogin({ serverUrl: serverUrl(), deviceName: deviceName(), discardPending });
      if (!outcome.ok) setResult(outcome);
    } finally {
      setBusy(false);
    }
  }

  const loginErrorMessage = () => {
    const s = loginState();
    return s.state === "error" ? s.message : "";
  };

  // The browser came back: the app stored the device token and syncs.
  createEffect(
    on(
      loginState,
      (s) => {
        if (s.state !== "done") return;
        toast("Angemeldet.");
        props.onConnected?.();
      },
      { defer: true },
    ),
  );
  // An event sent while the page was in the background can be lost: re-read
  // the configuration when the app is visible again.
  onMount(() => {
    const visible = async () => {
      if (document.visibilityState !== "visible") return;
      const config = await reloadConfig();
      if (props.setup && loginState().state === "waiting" && isConfigured(config)) props.onConnected?.();
    };
    document.addEventListener("visibilitychange", visible);
    onCleanup(() => document.removeEventListener("visibilitychange", visible));
  });

  async function save(discardPending = false) {
    setProblem("");
    setRetry("token");
    if (!urlOk()) return;
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
    <div class="form connection-form">
      <form
        class="form"
        onSubmit={(e) => {
          e.preventDefault();
          void signIn();
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
          Gerätename
          <input class="field" autocomplete="off" maxLength={100} value={deviceName()} onInput={(e) => setDeviceName(e.currentTarget.value)} />
        </label>
        <p class="muted">Unter diesem Namen erscheint das Gerät in der Web-Oberfläche unter „Geräte“; dort lässt es sich abmelden.</p>
        <button class="btn btn-primary" type="submit" disabled={busy()}>
          {props.setup || !hasToken() ? "Im Browser anmelden" : "Neu anmelden"}
        </button>
      </form>
      <Show when={loginState().state === "waiting"}>
        <p class="notice notice-warning" role="status">
          Melde dich im Browser an und erlaube den Zugriff. Danach geht es hier von selbst weiter.
        </p>
      </Show>
      <Show when={loginErrorMessage()}>
        <p class="notice notice-error" role="alert">
          {loginErrorMessage()}
        </p>
      </Show>
      <form
        class="form"
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        <p class="muted">Oder mit einem API-Token aus der Web-Oberfläche verbinden:</p>
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
        <button class="btn" type="submit" disabled={busy()}>
          {busy() ? "Verbinde …" : "Speichern und verbinden"}
        </button>
      </form>
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
                <button type="button" class="btn btn-danger btn-small" disabled={busy()} onClick={() => void (retry() === "login" ? signIn(true) : save(true))}>
                  Änderungen verwerfen und wechseln
                </button>
              </div>
            </Show>
          </Show>
        )}
      </Show>
    </div>
  );

  return (
    <Show
      when={!props.setup}
      fallback={
        <main class="login">
          <div class="login-card setup-card">
            <Logo size={64} />
            <h1>Zunder einrichten</h1>
            <p class="muted">
              Die App speichert deine Aufgaben auf diesem Gerät und gleicht sie im Hintergrund mit deinem Zunder-Server ab. Gib die Adresse deines Servers an und melde dich im Browser an.
            </p>
            {form}
          </div>
        </main>
      }
    >
      <div class="view">
        <header class="view-head">
          <h1>Einstellungen</h1>
        </header>
        <section class="detail-section" aria-label="Verbindung">
          <h3>Verbindung</h3>
          <p class="muted">Server und Anmeldung dieser App. Änderungen werden lokal gespeichert und im Hintergrund abgeglichen.</p>
          {form}
        </section>
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
        <AppSettings />
      </div>
    </Show>
  );
}

/** The app itself: version, start at login, updates, notifications. */
function AppSettings() {
  const [busy, setBusy] = createSignal<"" | "autostart" | "update" | "restart" | "notification">("");
  onMount(() => void loadAppInfo());

  async function run(what: "autostart" | "update" | "notification", action: () => Promise<ActionResult>, done?: string) {
    setBusy(what);
    try {
      const outcome = await action();
      if (!outcome.ok) toast(outcome.message ?? "Unbekannter Fehler.", "error");
      else if (done) toast(done);
    } finally {
      setBusy("");
    }
  }

  async function restart() {
    setBusy("restart");
    try {
      await restartApp();
    } catch (err) {
      toast(err instanceof Error ? err.message : String(err), "error");
      setBusy("");
    }
  }

  const update = () => appInfo()?.update ?? null;
  const updateText = () => {
    const u = update();
    if (!u) return "";
    if (u.installed) return `Version ${u.installed} ist installiert und wird nach einem Neustart aktiv.`;
    const last = u.last_check;
    if (!last) return "Zunder sucht beim Start und danach alle 6 Stunden selbst nach Updates.";
    const when = formatInstant(new Date(last.at_ms).toISOString(), new Date(now()));
    return last.ok ? `Zunder ist aktuell (geprüft: ${when}).` : `Letzte Prüfung ${when} fehlgeschlagen: ${last.error ?? "unbekannter Fehler"}`;
  };

  return (
    <section class="detail-section app-settings" aria-label="App">
      <h3>App</h3>
      <p class="muted" data-testid="app-version">
        Zunder {appInfo()?.version ?? "…"}
      </p>
      <Show when={appInfo()?.desktop}>
        <label class="toggle">
          <input
            type="checkbox"
            checked={!!appInfo()?.autostart}
            disabled={busy() === "autostart" || appInfo()?.autostart == null}
            onChange={(e) => {
              const enabled = e.currentTarget.checked;
              void run("autostart", () => setAutostart(enabled), enabled ? "Zunder startet ab jetzt beim Anmelden." : "Autostart ist aus.");
            }}
          />
          Beim Anmelden am Rechner starten
        </label>
        <p class="muted">
          Zunder startet dann unsichtbar in der Menüleiste. Das Fenster zu schließen beendet Zunder nicht; Erinnerungen kommen weiter. Beenden geht über das Zunder-Symbol in der Menüleiste.
        </p>
        <p data-testid="update-status">{updateText()}</p>
        <div class="app-actions">
          <Show
            when={update()?.installed}
            fallback={
              <button class="btn" disabled={busy() === "update"} onClick={() => void run("update", checkUpdate)}>
                <Icon name="sync" size={14} /> {busy() === "update" ? "Sucht …" : "Nach Updates suchen"}
              </button>
            }
          >
            <button class="btn btn-primary" disabled={busy() === "restart"} onClick={() => void restart()}>
              Jetzt neu starten
            </button>
          </Show>
        </div>
      </Show>
      <div class="app-actions">
        <button class="btn" disabled={busy() === "notification"} onClick={() => void run("notification", testNotification, "Testbenachrichtigung gesendet.")}>
          Testbenachrichtigung senden
        </button>
      </div>
      <p class="muted">Kommt keine an: in den Systemeinstellungen unter „Mitteilungen“ Zunder erlauben.</p>
    </section>
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
