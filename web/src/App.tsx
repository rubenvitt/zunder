import { createMemo, createSignal, For, lazy, Match, onCleanup, onMount, Show, Switch } from "solid-js";
import { CommandPalette } from "./components/CommandPalette";
import { ContextMenu } from "./components/ContextMenu";
import { Icon, type IconName } from "./components/Icon";
import { DeferDecisionDialog, DropSeriesDialog, HandBackDialog, ShortcutHelp, Toasts } from "./components/Overlays";
import { QuickCapture } from "./components/QuickCapture";
import { StructureForm } from "./components/StructureForm";
import { TaskDetail } from "./components/TaskDetail";
import { TaskHoverCard } from "./components/TaskLinks";
import { TabBar } from "./components/TabBar";
import { installKeyboard } from "./keys";
import { contextMenu } from "./state/menu";
import { installContextMenu } from "./state/context-menu";
import { installDetailHistory, NARROW_LAYOUT } from "./state/back";
import { isTauri } from "./platform/runtime";
import { isUnauthorized } from "./sync/transport";
import { navigate, parseRoute, route, type Route, type ViewName } from "./state/router";
import { inboxCount, todayGroups, waitingGroups } from "./state/selectors";
import { api, data, getEngine, pendingCount, ready, setNotifier, startSync, syncStatus, taskIndex } from "./state/store";
import { captureOpen, deferRequest, detailId, handBackRequest, dropRequest, detailWide, detailWidth, helpOpen, openSearch, palette, setCaptureOpen, setDetailId, setHelpOpen, setSelectedId, sidebarHidden, structureEdit, toast, toggleSidebar } from "./state/ui";
import { Logo } from "./components/Logo";
import { refreshFocus } from "./state/focus";
import { preloadEditor } from "./components/MarkdownField";
import { CommitmentsView, InboxView, LooseView, TodayView, WaitingView } from "./views/Lists";
import { DevView, LoginView, PhoenixView, TokensView } from "./views/Other";
import { AgentLogView } from "./views/AgentLog";
import { SettingsView } from "./views/Settings";
import { StructureView } from "./views/Structure";

interface NavItem {
  view: ViewName;
  label: string;
  /** Label in the phone's tab bar, where space is short. */
  short?: string;
  icon: IconName;
  keys: string;
  /** Starts a new group in the sidebar. */
  gap?: boolean;
}

const NAV: NavItem[] = [
  { view: "heute", label: "Heute", icon: "sun", keys: "g h" },
  { view: "inbox", label: "Inbox", icon: "inbox", keys: "g i" },
  { view: "lose", label: "Lose", icon: "loose", keys: "g l" },
  { view: "bereiche", label: "Bereiche", icon: "folder", keys: "g b", gap: true },
  { view: "dev", label: "Dev", icon: "code", keys: "g d" },
  { view: "phoenix", label: "Phönix", icon: "flame", keys: "g p" },
  { view: "wartet", label: "Wartet auf andere", short: "Wartet", icon: "user", keys: "g w", gap: true },
];

// The phone's tab bar holds four views, as iOS advises (at most five tabs); the rest sit behind "Mehr".
const TAB_PRIMARY: ViewName[] = ["heute", "inbox", "bereiche", "phoenix"];
const TAB_MORE: ViewName[] = ["lose", "dev", "wartet"];
const navItem = (view: ViewName): NavItem => NAV.find((n) => n.view === view)!;

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
  onMount(() => onCleanup(installContextMenu()));
  // "Erst für dich": send what was changed offline, read what other devices changed.
  onMount(() => void refreshFocus());
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
  // What each list holds, beside its name: due today (without the running flame) and who is waited on.
  const todayCount = createMemo(() => {
    const g = todayGroups();
    return new Set([...g.overdue, ...g.dueToday, ...g.reminders, ...g.followUps].map((t) => t.id)).size;
  });
  const waitingCount = createMemo(() => waitingGroups().reduce((n, g) => n + g.tasks.length, 0));
  const countOf = (view: ViewName) => (view === "inbox" ? inboxCount() : view === "heute" ? todayCount() : view === "wartet" ? waitingCount() : 0);
  const mac = /Mac|iPhone|iPad/.test(navigator.platform);

  return (
    <div class="app" classList={{ "detail-open": !!detailId(), "detail-wide": !!detailId() && detailWide(), "sidebar-hidden": sidebarHidden() }} style={{ "--detail": `${detailWidth()}px` }}>
      <nav class="sidebar" aria-label="Sichten">
        <div class="brand">
          <Logo size={26} />
          <span>Zunder</span>
          <button class="btn btn-ghost btn-icon btn-xs" title={`Seitenleiste einklappen (${mac ? "⌘B" : "Strg+B"})`} aria-label="Seitenleiste einklappen" onClick={toggleSidebar}>
            <Icon name="left" size={16} />
          </button>
        </div>
        <button class="btn btn-primary capture-button" onClick={() => setCaptureOpen(true)} title="Neue Aufgabe (n)">
          <Icon name="plus" size={16} />
          <span>Neue Aufgabe</span>
          <kbd aria-hidden="true">N</kbd>
        </button>
        <button class="search-button" onClick={openSearch} title={`Suche (${mac ? "⌘K" : "Strg+K"})`}>
          <Icon name="search" size={16} />
          <span>Suchen</span>
          <kbd aria-hidden="true">{mac ? "⌘K" : "Strg K"}</kbd>
        </button>
        <ul class="nav">
          <For each={NAV}>
            {(item) => (
              <li classList={{ "nav-gap": !!item.gap }}>
                <a href={`#/${item.view}`} class="nav-item" classList={{ active: active(item.view) }} title={`${item.label} (${item.keys})`} aria-current={active(item.view) ? "page" : undefined}>
                  <Icon name={item.icon} />
                  <span>{item.label}</span>
                  <Show when={countOf(item.view) > 0}>
                    <span class="nav-count" classList={{ hot: item.view === "inbox" }}>
                      {countOf(item.view)}
                    </span>
                  </Show>
                </a>
              </li>
            )}
          </For>
        </ul>
        <div class="sidebar-foot">
          <Show
            when={desktop}
            fallback={
              <>
                <a href="#/tokens" class="nav-item small" classList={{ active: active("tokens") }}>
                  <Icon name="key" size={16} />
                  <span>API-Tokens</span>
                </a>
                <a href="#/einstellungen" class="nav-item small" classList={{ active: active("einstellungen") || active("ki-protokoll") }}>
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

      <Show when={sidebarHidden()}>
        <button class="btn btn-ghost btn-icon btn-xs sidebar-reveal" title={`Seitenleiste ausklappen (${mac ? "⌘B" : "Strg+B"})`} aria-label="Seitenleiste ausklappen" onClick={toggleSidebar}>
          <Icon name="menu" size={16} />
        </button>
      </Show>

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
          <Match when={route().view === "wartet"}>
            <WaitingView />
          </Match>
          <Match when={route().view === "zusagen"}>
            <CommitmentsView />
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
          <Match when={route().view === "ki-protokoll"}>
            <Show when={!desktop} fallback={<SettingsView />}>
              <AgentLogView />
            </Show>
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

      <TabBar
        primary={TAB_PRIMARY.map(navItem)}
        more={TAB_MORE.map(navItem)}
        current={route().view}
        inert={covered()}
        countOf={countOf}
        badgeOf={(view) => (view === "inbox" ? inboxCount() : 0)}
        onSelect={(view) => navigate(parseRoute(`#/${view}`))}
      />

      <Show when={captureOpen()}>
        <QuickCapture />
      </Show>
      <Show when={palette()}>{(req) => <CommandPalette request={req()} />}</Show>
      <Show when={structureEdit()}>{(edit) => <StructureForm edit={edit()} />}</Show>
      <Show when={helpOpen()}>
        <ShortcutHelp />
      </Show>
      <Show when={dropRequest()}>{(req) => <DropSeriesDialog request={req()} />}</Show>
      <Show when={deferRequest()}>{(req) => <DeferDecisionDialog request={req()} />}</Show>
      <Show when={handBackRequest()}>{(req) => <HandBackDialog request={req()} />}</Show>
      <Show when={contextMenu()} keyed>
        {(req) => <ContextMenu request={req} />}
      </Show>
      <Toasts />
      <TaskHoverCard />
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

/** Shown only when something is wrong: offline, with or without pending changes. */
function BrowserSyncIndicator() {
  const label = () => (pendingCount() > 0 ? `Offline · ${pendingCount()} ausstehend` : "Offline");
  return (
    <Show when={syncStatus() === "offline"}>
      <span class="sync" data-sync={syncStatus()} title={label()}>
        <Icon name="offline" size={14} />
        <span class="sync-label">{label()}</span>
      </span>
    </Show>
  );
}
