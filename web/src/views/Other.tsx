// Dev, Phönix, API tokens and login.

import { createMemo, createResource, createSignal, For, onMount, Show } from "solid-js";
import { Board } from "../components/Board";
import { Icon } from "../components/Icon";
import { Logo } from "../components/Logo";
import { TaskList } from "../components/TaskList";
import { isTauri } from "../platform/runtime";
import { fireOf, formatInstant, isValidFire, shiftFire } from "../sync/dates";
import { isFlameRunning } from "../sync/derived";
import type { ApiToken, CreatedToken, Device, OAuthClient, Task } from "../sync/types";
import { flame } from "../state/actions";
import { navigate, route } from "../state/router";
import { api, now } from "../state/store";
import { devProjectIds, devTasks, phoenixTasks } from "../state/selectors";
import { copyText, registerList, toast } from "../state/ui";

export function DevView() {
  const tasks = createMemo(() => devTasks().filter((t) => t.status !== "dropped"));
  return (
    <div class="view view-wide">
      <header class="view-head">
        <h1>Dev</h1>
        <span class="muted">{devProjectIds().size} Dev-Projekte</span>
      </header>
      <Show when={devProjectIds().size > 0} fallback={<p class="empty big">Noch keine Dev-Projekte. Ein Projekt mit der Art „Dev“ anlegen.</p>}>
        <Board tasks={tasks()} dev devDetails label="Dev-Board" />
      </Show>
    </div>
  );
}

function FlameButton(props: { task: Task }) {
  const running = () => isFlameRunning(props.task);
  return (
    <button
      class="btn btn-small flame-button"
      classList={{ "btn-primary": !running(), burning: running() }}
      data-flame-for={props.task.id}
      onClick={() => void flame(props.task.id, running() ? "stop" : "start")}
    >
      <Icon name={running() ? "stop" : "flame"} size={14} />
      {running() ? "Flamme beenden" : "Flamme starten"}
    </button>
  );
}

export function PhoenixView() {
  const fire = () => {
    const r = route();
    return r.view === "phoenix" && r.fire && isValidFire(r.fire) ? r.fire : fireOf(new Date(now()));
  };
  const groups = createMemo(() => phoenixTasks(fire()));
  const current = () => fireOf(new Date(now()));
  onMount(() => registerList(() => [...groups().steps, ...groups().flames]));
  const go = (weeks: number) => navigate({ view: "phoenix", fire: shiftFire(fire(), weeks) });

  return (
    <div class="view">
      <header class="view-head">
        <h1>Phönix</h1>
        <div class="fire-nav">
          <button class="btn btn-ghost btn-icon" aria-label="Vorige Woche" onClick={() => go(-1)}>
            <Icon name="left" />
          </button>
          <span class="fire-label mono" data-fire={fire()}>
            Feuer {fire()}
          </span>
          <button class="btn btn-ghost btn-icon" aria-label="Nächste Woche" onClick={() => go(1)}>
            <Icon name="right" />
          </button>
          <Show when={fire() !== current()}>
            <button class="btn btn-ghost btn-small" onClick={() => navigate({ view: "phoenix" })}>
              Aktuelles Feuer
            </button>
          </Show>
        </div>
      </header>
      <section class="group">
        <h2>Feuer-Schritte</h2>
        <TaskList tasks={groups().steps} showLocation label="Feuer-Schritte" empty="Keine Feuer-Schritte in dieser Woche." />
      </section>
      <section class="group">
        <h2>Flammen</h2>
        <Show when={groups().flames.length > 0} fallback={<p class="empty">Keine Flammen in dieser Woche.</p>}>
          <ul class="flame-list">
            <For each={groups().flames}>
              {(task) => (
                <li class="flame-item" classList={{ burning: isFlameRunning(task) }}>
                  <TaskList tasks={[task]} showLocation />
                  <FlameButton task={task} />
                </li>
              )}
            </For>
          </ul>
        </Show>
      </section>
    </div>
  );
}

export function TokensView() {
  return isTauri() ? <TokensInApp /> : <TokenManager />;
}

/** Tauri app: token management needs a browser session on the server. */
function TokensInApp() {
  return (
    <div class="view">
      <header class="view-head">
        <h1>API-Tokens</h1>
      </header>
      <p class="muted">API-Tokens verwaltest du in der Web-Oberfläche deines Zunder-Servers; dafür ist die Anmeldung im Browser nötig. Die App selbst nutzt den Token aus der Verbindung.</p>
      <a class="btn" href="#/verbindung">
        Zur Verbindung
      </a>
    </div>
  );
}

function TokenManager() {
  const [tokens, { refetch }] = createResource(async (): Promise<ApiToken[] | null> => {
    try {
      return await api().listTokens();
    } catch (err) {
      toast(err instanceof Error ? err.message : "Tokens konnten nicht geladen werden.", "error");
      return null;
    }
  });
  const [name, setName] = createSignal("");
  const [created, setCreated] = createSignal<CreatedToken | null>(null);

  async function create(e: Event) {
    e.preventDefault();
    if (!name().trim()) return;
    try {
      setCreated(await api().createToken(name().trim()));
      setName("");
      void refetch();
    } catch (err) {
      toast(err instanceof Error ? err.message : "Token konnte nicht erstellt werden.", "error");
    }
  }

  async function revoke(token: ApiToken) {
    if (!confirm(`Token „${token.name}“ widerrufen? Agenten mit diesem Token verlieren sofort den Zugriff.`)) return;
    try {
      await api().revokeToken(token.id);
      toast(`Token „${token.name}“ widerrufen.`);
      void refetch();
    } catch (err) {
      toast(err instanceof Error ? err.message : "Widerrufen fehlgeschlagen.", "error");
    }
  }

  const [clients, { refetch: refetchClients }] = createResource(async (): Promise<OAuthClient[] | null> => {
    try {
      return await api().listOAuthClients();
    } catch (err) {
      toast(err instanceof Error ? err.message : "Verbundene Apps konnten nicht geladen werden.", "error");
      return null;
    }
  });

  async function disconnect(client: OAuthClient) {
    if (!confirm(`Verbindung „${client.name}“ trennen? Die App verliert sofort den Zugriff.`)) return;
    try {
      await api().revokeOAuthClient(client.id);
      toast(`Verbindung „${client.name}“ getrennt.`);
      void refetchClients();
    } catch (err) {
      toast(err instanceof Error ? err.message : "Trennen fehlgeschlagen.", "error");
    }
  }

  const [devices, { refetch: refetchDevices }] = createResource(async (): Promise<Device[] | null> => {
    try {
      return await api().listDevices();
    } catch (err) {
      toast(err instanceof Error ? err.message : "Geräte konnten nicht geladen werden.", "error");
      return null;
    }
  });

  async function revokeDevice(device: Device) {
    if (!confirm(`Gerät „${device.name}“ abmelden? Die App dort gleicht sofort nicht mehr ab.`)) return;
    try {
      await api().revokeDevice(device.id);
      toast(`Gerät „${device.name}“ abgemeldet.`);
      void refetchDevices();
    } catch (err) {
      toast(err instanceof Error ? err.message : "Abmelden fehlgeschlagen.", "error");
    }
  }

  return (
    <div class="view">
      <header class="view-head">
        <h1>API-Tokens</h1>
      </header>
      <p class="muted">Tokens für Agenten und den MCP-Zugang. Jeder Token ist einzeln widerrufbar.</p>
      <form class="inline-form" onSubmit={create}>
        <input class="field" placeholder="Name, z. B. claude-code" aria-label="Token-Name" value={name()} onInput={(e) => setName(e.currentTarget.value)} />
        <button class="btn btn-primary" type="submit" disabled={!name().trim()}>
          <Icon name="key" size={14} /> Token erstellen
        </button>
      </form>
      <Show when={created()}>
        {(c) => (
          <div class="notice notice-success new-token">
            <p>
              Neuer Token <strong>{c().name}</strong>. Er wird nur jetzt angezeigt:
            </p>
            <code class="token-value" data-testid="new-token">
              {c().token}
            </code>
            <div class="inline-group">
              <button
                class="btn btn-small"
                onClick={() => copyText(c().token, "Token kopiert.")}
              >
                Kopieren
              </button>
              <button class="btn btn-ghost btn-small" onClick={() => setCreated(null)}>
                Ausblenden
              </button>
            </div>
          </div>
        )}
      </Show>
      <Show when={tokens() !== null} fallback={<p class="muted">Tokens nicht verfügbar.</p>}>
        <table class="table">
          <thead>
            <tr>
              <th>Name</th>
              <th>Erstellt</th>
              <th>Zuletzt benutzt</th>
              <th />
            </tr>
          </thead>
          <tbody>
            <For each={tokens() ?? []} fallback={<tr><td colSpan={4} class="muted">{tokens.loading ? "Lädt …" : "Noch keine Tokens."}</td></tr>}>
              {(token) => (
                <tr data-token-id={token.id}>
                  <td>{token.name}</td>
                  <td>{token.created_at ? formatInstant(token.created_at, new Date(now())) : "—"}</td>
                  <td>{token.last_used_at ? formatInstant(token.last_used_at, new Date(now())) : "nie"}</td>
                  <td class="right">
                    <button class="btn btn-danger btn-small" onClick={() => void revoke(token)}>
                      Widerrufen
                    </button>
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Show>
      <section class="group">
        <h2>Geräte</h2>
        <p class="muted">Geräte, auf denen du dich in der Zunder-App angemeldet hast. Abmelden widerruft den Zugang sofort; die App fragt dann nach einer neuen Anmeldung.</p>
        <Show when={devices() !== null} fallback={<p class="muted">Geräte nicht verfügbar.</p>}>
          <table class="table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Angemeldet seit</th>
                <th>Zuletzt abgeglichen</th>
                <th />
              </tr>
            </thead>
            <tbody>
              <For each={devices() ?? []} fallback={<tr><td colSpan={4} class="muted">{devices.loading ? "Lädt …" : "Keine Geräte angemeldet."}</td></tr>}>
                {(device) => (
                  <tr data-device-id={device.id}>
                    <td>{device.name}</td>
                    <td>{device.created_at ? formatInstant(device.created_at, new Date(now())) : "—"}</td>
                    <td>{device.last_used_at ? formatInstant(device.last_used_at, new Date(now())) : "nie"}</td>
                    <td class="right">
                      <button class="btn btn-danger btn-small" onClick={() => void revokeDevice(device)}>
                        Abmelden
                      </button>
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Show>
      </section>
      <section class="group">
        <h2>Verbundene Apps</h2>
        <p class="muted">
          Apps, die sich per OAuth mit Zunder verbunden haben, z. B. claude.ai als Connector. Trennen widerruft den Zugriff sofort; beim nächsten
          Verbinden fragt Zunder wieder nach der Freigabe.
        </p>
        <Show when={clients() !== null} fallback={<p class="muted">Verbundene Apps nicht verfügbar.</p>}>
          <table class="table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Weiterleitung</th>
                <th>Verbunden seit</th>
                <th>Zuletzt benutzt</th>
                <th />
              </tr>
            </thead>
            <tbody>
              <For
                each={clients() ?? []}
                fallback={<tr><td colSpan={5} class="muted">{clients.loading ? "Lädt …" : "Keine verbundenen Apps."}</td></tr>}
              >
                {(client) => (
                  <tr data-oauth-client-id={client.id}>
                    <td>{client.name}</td>
                    <td>{redirectHosts(client.redirect_uris)}</td>
                    <td>{client.created_at ? formatInstant(client.created_at, new Date(now())) : "—"}</td>
                    <td>{client.last_used_at ? formatInstant(client.last_used_at, new Date(now())) : "nie"}</td>
                    <td class="right">
                      <button class="btn btn-danger btn-small" onClick={() => void disconnect(client)}>
                        Trennen
                      </button>
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Show>
      </section>
    </div>
  );
}

/** Hosts of the redirect URIs of an OAuth client, e.g. `claude.ai`. */
function redirectHosts(uris: string[]): string {
  const hosts = new Set<string>();
  for (const uri of uris) {
    try {
      hosts.add(new URL(uri).hostname);
    } catch {
      hosts.add(uri);
    }
  }
  return [...hosts].join(", ") || "—";
}

export function LoginView() {
  return (
    <main class="login">
      <div class="login-card">
        <Logo size={64} />
        <h1>Zunder</h1>
        <p class="muted">Bitte anmelden, um fortzufahren.</p>
        <a class="btn btn-primary btn-large" href="/auth/login">
          Anmelden
        </a>
      </div>
    </main>
  );
}
