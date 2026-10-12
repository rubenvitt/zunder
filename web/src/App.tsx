import { createSignal, For, lazy, Match, onCleanup, onMount, Show, Switch } from "solid-js";
import { CommandPalette } from "./components/CommandPalette";
import { Icon, type IconName } from "./components/Icon";
import { ShortcutHelp, Toasts } from "./components/Overlays";
import { QuickCapture } from "./components/QuickCapture";
import { StructureForm } from "./components/StructureForm";
import { TaskDetail } from "./components/TaskDetail";
import { installKeyboard } from "./keys";
import { installDetailHistory, NARROW_LAYOUT } from "./state/back";
import { isTauri } from "./platform/runtime";
import { isUnauthorized } from "./sync/transport";
import { navigate, parseRoute, route, type Route, type ViewName } from "./state/router";
import { inboxCount } from "./state/selectors";
import { api, data, getEngine, pendingCount, ready, setNotifier, startSync, syncStatus, taskIndex } from "./state/store";
import { captureOpen, detailId, detailWide, detailWidth, helpOpen, openSearch, palette, setCaptureOpen, setDetailId, setHelpOpen, setSelectedId, structureEdit, toast } from "./state/ui";
import { Logo } from "./components/Logo";
import { preloadEditor } from "./components/MarkdownField";
import { InboxView, LooseView, TodayView } from "./views/Lists";
import { DevView, LoginView, PhoenixView, TokensView } from "./views/Other";
import { SettingsView } from "./views/Settings";
import { StructureView } from "./views/Structure";

interface NavItem {
  view: ViewName;
  label: string;
  icon: IconName;
  keys: string;
}

const NAV: NavItem[] = [
  { view: "heute", label: "Heute", icon: "sun", keys: "g h" },
  { view: "inbox", label: "Inbox", icon: "inbox", keys: "g i" },
  { view: "lose", label: "Lose", icon: "loose", keys: "g l" },
  { view: "bereiche", label: "Bereiche", icon: "folder", keys: "g b" },
  { view: "dev", label: "Dev", icon: "code", keys: "g d" },
  { view: "phoenix", label: "Phönix", icon: "flame", keys: "g p" },
];

setNotifier({
  rejected(rejections) {
    const messages = [...new Set(rejections.map((r) => r.message))];
    toast(`Änderung abgelehnt: ${messages.join(" ")}`, "error");
  },
  warnings(warnings) {
    // open_subtasks is already shown locally at the moment of the change (warnIfOpenChildren),
    // also offline; push warnings only describe this client's own ops.
    const shown = warnings.filter((w) => w.code !== "open_subtasks");
    for (const message of new Set(shown.map((w) => w.message))) toast(message, "warning");
  },
});

type AuthState = "checking" | "ok" | "login";

export function App() {
  return isTauri() ? <DesktopApp shell={Shell} onReady={markReady} /> : <BrowserApp />;
}

// Tauri app only: loaded on demand, so the browser bundle stays lean.
const desktopViews = () => import("./views/Desktop");
const DesktopApp = lazy(() => desktopViews().then((m) => ({ default: m.DesktopApp })));
const ConnectionView = lazy(() => desktopViews().then((m) => ({ default: m.ConnectionView })));
const DesktopSyncIndicator = lazy(() => desktopViews().then((m) => ({ default: m.DesktopSyncIndicator })));

function BrowserApp() {
  const [auth, setAuth] = createSignal<AuthState>("checking");

  onMount(async () => {
    try {
      await api().me();
    } catch (err) {
      if (isUnauthorized(err)) {
        setAuth("login");
        markReady();
        return;
      }
      // Server unreachable: the sync engine keeps retrying with backoff.
    }
    setAuth("ok");
    const ok = await startSync();
    if (!ok && syncStatus() === "unauthorized") setAuth("login");
    markReady();
  });

  return (
    <Switch fallback={<div class="splash" aria-busy="true" />}>
      <Match when={auth() === "login" || syncStatus() === "unauthorized"}>
        <LoginView />
      </Match>
      <Match when={auth() === "ok" && ready()}>
        <Shell />
      </Match>
      <Match when={auth() === "ok" && syncStatus() === "offline"}>
        <div class="splash">
          <p class="muted">Server nicht erreichbar. Neuer Versuch läuft …</p>
        </div>
      </Match>
    </Switch>
  );
}

function markReady(): void {
  requestAnimationFrame(() => {
    document.documentElement.dataset.ready = "true";
    performance.mark("zunder-ready");
  });
}

function Shell() {
  const desktop = isTauri();
  onMount(() => onCleanup(installKeyboard()));
  installDetailHistory();
  // The Markdown editor is a separate chunk: fetch it once the app is idle, so the first task opens instantly.
  onMount(() => {
    const idle = window.requestIdleCallback ?? ((cb: () => void) => setTimeout(cb, 1500));
    idle(() => void preloadEditor());
  });
  const active = (view: ViewName) => route().view === view;
  // On phones the detail panel covers everything: keep focus and screen readers out of what is behind it.
  const narrowQuery = window.matchMedia(NARROW_LAYOUT);
  const [narrow, setNarrow] = createSignal(narrowQuery.matches);
  const onNarrow = () => setNarrow(narrowQuery.matches);
  narrowQuery.addEventListener("change", onNarrow);
  onCleanup(() => narrowQuery.removeEventListener("change", onNarrow));
  const covered = () => narrow() && !!detailId();
  const mobileTitle = () => NAV.find((n) => n.view === route().view)?.label ?? "Zunder";

  return (
    <div class="app" classList={{ "detail-open": !!detailId(), "detail-wide": !!detailId() && detailWide() }} style={{ "--detail": `${detailWidth()}px` }}>
      <nav class="sidebar" aria-label="Sichten">
        <div class="brand">
          <Logo size={26} />
          <span>Zunder</span>
        </div>
        <button class="search-button" onClick={openSearch} title="Suche (⌘K)">
          <Icon name="search" size={16} />
          <span>Suchen</span>
          <kbd>⌘K</kbd>
        </button>
        <ul class="nav">
          <For each={NAV}>
            {(item) => (
              <li>
                <a href={`#/${item.view}`} class="nav-item" classList={{ active: active(item.view) }} title={`${item.label} (${item.keys})`} aria-current={active(item.view) ? "page" : undefined}>
                  <Icon name={item.icon} />
                  <span>{item.label}</span>
                  <Show when={item.view === "inbox" && inboxCount() > 0}>
                    <span class="nav-count">{inboxCount()}</span>
                  </Show>
                </a>
              </li>
            )}
          </For>
        </ul>
        <div class="sidebar-foot">
          <button class="btn btn-primary capture-button" onClick={() => setCaptureOpen(true)} title="Neue Aufgabe (n)">
            <Icon name="plus" size={16} /> Neue Aufgabe
          </button>
          <Show
            when={desktop}
            fallback={
              <>
                <a href="#/tokens" class="nav-item small" classList={{ active: active("tokens") }}>
                  <Icon name="key" size={16} />
                  <span>API-Tokens</span>
                </a>
                <a href="#/einstellungen" class="nav-item small" classList={{ active: active("einstellungen") }}>
                  <Icon name="settings" size={16} />
                  <span>Einstellungen</span>
                </a>
              </>
            }
          >
            <a href="#/verbindung" class="nav-item small" classList={{ active: active("verbindung") }}>
              <Icon name="settings" size={16} />
              <span>Einstellungen</span>
            </a>
          </Show>
          <button class="nav-item small" onClick={() => setHelpOpen(true)}>
            <Icon name="keyboard" size={16} />
            <span>Tastenkürzel</span>
          </button>
          <SyncIndicator />
        </div>
      </nav>

      <header class="topbar" inert={covered()}>
        <span class="brand">
          <Logo size={24} /> {mobileTitle()}
        </span>
        <SyncIndicator />
        <button class="btn btn-ghost btn-icon" aria-label="Suchen" onClick={openSearch}>
          <Icon name="search" />
        </button>
        <Show
          when={desktop}
          fallback={
            <>
              <a class="btn btn-ghost btn-icon" aria-label="API-Tokens" href="#/tokens">
                <Icon name="key" />
              </a>
              <a class="btn btn-ghost btn-icon" aria-label="Einstellungen" href="#/einstellungen">
                <Icon name="settings" />
              </a>
            </>
          }
        >
          <a class="btn btn-ghost btn-icon" aria-label="Einstellungen" href="#/verbindung">
            <Icon name="settings" />
          </a>
        </Show>
      </header>

      <main class="main" id="main" inert={covered()}>
        <Switch>
          <Match when={route().view === "heute"}>
            <TodayView />
          </Match>
          <Match when={route().view === "inbox"}>
            <InboxView />
          </Match>
          <Match when={route().view === "lose"}>
            <LooseView />
          </Match>
          <Match when={route().view === "bereiche"}>
            <StructureView />
          </Match>
          <Match when={route().view === "dev"}>
            <DevView />
          </Match>
          <Match when={route().view === "phoenix"}>
            <PhoenixView />
          </Match>
          <Match when={route().view === "tokens"}>
            <TokensView />
          </Match>
          <Match when={route().view === "einstellungen"}>
            <SettingsView />
          </Match>
          <Match when={route().view === "verbindung"}>
            <Show when={desktop} fallback={<TokensView />}>
              <ConnectionView />
            </Show>
          </Match>
          <Match when={route().view === "aufgabe" || route().view === "clickup"}>
            <TaskLink />
          </Match>
        </Switch>
      </main>

      <Show when={detailId()}>{(id) => <TaskDetail id={id()} />}</Show>

      <button class="fab" aria-label="Neue Aufgabe" inert={covered()} onClick={() => setCaptureOpen(true)}>
        <Icon name="plus" size={28} />
      </button>

      <nav class="tabbar" aria-label="Sichten" inert={covered()}>
        <For each={NAV}>
          {(item) => (
            <button class="tab" classList={{ active: active(item.view) }} onClick={() => navigate(parseRoute(`#/${item.view}`))} aria-current={active(item.view) ? "page" : undefined}>
              <Icon name={item.icon} size={20} />
              <span>{item.label}</span>
              <Show when={item.view === "inbox" && inboxCount() > 0}>
                <span class="tab-count">{inboxCount()}</span>
              </Show>
            </button>
          )}
        </For>
      </nav>

      <Show when={captureOpen()}>
        <QuickCapture />
      </Show>
      <Show when={palette()}>{(req) => <CommandPalette request={req()} />}</Show>
      <Show when={structureEdit()}>{(edit) => <StructureForm edit={edit()} />}</Show>
      <Show when={helpOpen()}>
        <ShortcutHelp />
      </Show>
      <Toasts />
    </div>
  );
}

/** "#/aufgabe/<id or ZUN-42>" or "#/clickup/<clickup_id>": open the task over its home view. */
function TaskLink() {
  onMount(async () => {
    const r = route();
    if (r.view !== "aufgabe" && r.view !== "clickup") return;
    const reference = r.id.toUpperCase();
    const find = () =>
      r.view === "aufgabe"
        ? (data.tasks[r.id] ?? Object.values(data.tasks).find((t) => t.ref === reference))
        : Object.values(data.tasks).find((t) => t.clickup_id === r.id);
    let task = find();
    if (!task) {
      // A running app may not have pulled a task created a moment ago.
      await getEngine().pull();
      if (route() !== r) return;
      task = find();
    }
    if (!task || taskIndex().isDeleted(task)) {
      toast("Aufgabe nicht gefunden oder gelöscht.", "error");
      navigate({ view: "heute" }, { replace: true });
      return;
    }
    const loc = taskIndex().location(task);
    const home: Route =
      loc.kind === "inbox" ? { view: "inbox" } : loc.kind === "loose" ? { view: "lose" } : { view: "bereiche", kind: loc.kind, id: loc.id };
    setSelectedId(task.id);
    navigate(home, { replace: true });
    setDetailId(task.id);
  });
  return null;
}

function SyncIndicator() {
  return isTauri() ? <DesktopSyncIndicator /> : <BrowserSyncIndicator />;
}

function BrowserSyncIndicator() {
  const label = () => {
    if (syncStatus() === "offline") return pendingCount() > 0 ? `Offline · ${pendingCount()} ausstehend` : "Offline";
    if (pendingCount() > 0) return "Synchronisiert …";
    return "Synchron";
  };
  return (
    <span class="sync" classList={{ offline: syncStatus() === "offline", pending: pendingCount() > 0 }} data-sync={syncStatus()} title={label()}>
      <Show when={syncStatus() === "offline"} fallback={<span class="sync-dot" />}>
        <Icon name="offline" size={14} />
      </Show>
      <span class="sync-label">{label()}</span>
    </span>
  );
}
