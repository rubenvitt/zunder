// ⌘K command palette: tasks (incl. closed), projects, areas and actions.
// Also serves as picker for locations and parent tasks.

import { createMemo, createSignal, For, onMount, Show } from "solid-js";
import { isClosed } from "../sync/derived";
import type { Location } from "../sync/types";
import { locationLabel, STATUS_LABELS } from "../labels";
import { isTauri } from "../platform/runtime";
import { navigate } from "../state/router";
import { api, areaList, data, projectList, taskIndex } from "../state/store";
import { setCaptureOpen, setDetailId, setHelpOpen, setPalette, setSelectedId, setStructureEdit, toast, type PaletteRequest } from "../state/ui";
import { Modal } from "./Modal";

interface Item {
  key: string;
  group: string;
  label: string;
  detail?: string;
  hay: string;
  rank: number;
  dim?: boolean;
  run: () => void;
}

function normalize(text: string): string {
  return text
    .toLowerCase()
    .replace(/ä/g, "ae")
    .replace(/ö/g, "oe")
    .replace(/ü/g, "ue")
    .replace(/ß/g, "ss")
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "");
}

function score(item: Item, terms: string[], query: string): number | null {
  for (const t of terms) if (!item.hay.includes(t)) return null;
  const label = normalize(item.label);
  let s = item.rank * 10;
  if (label.startsWith(query)) s -= 5;
  else if (label.includes(` ${terms[0]}`)) s -= 3;
  if (item.dim) s += 4;
  return s;
}

const LIMIT = 50;

export function CommandPalette(props: { request: PaletteRequest }) {
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal(0);
  let input: HTMLInputElement | undefined;
  let listEl: HTMLUListElement | undefined;
  onMount(() => input?.focus());

  const close = () => setPalette(null);

  const actions = (): Item[] => {
    const go = (label: string, run: () => void, detail?: string): Item => ({
      key: `action:${label}`,
      group: "Aktionen",
      label,
      detail,
      hay: normalize(`${label} ${detail ?? ""}`),
      rank: 0,
      run,
    });
    // In the Tauri app there is no session: tokens and logout give way to the connection settings.
    const account = isTauri()
      ? [
          go("Einstellungen", () => navigate({ view: "verbindung" }), "Verbindung, Autostart, Updates"),
          go("Jetzt abgleichen", () => {
            void import("../state/desktop")
              .then((m) => m.syncNow())
              .then((r) => toast(r.ok ? "Abgeglichen." : (r.message ?? "Abgleich fehlgeschlagen."), r.ok ? "info" : "error"));
          }),
        ]
      : [
          go("API-Tokens verwalten", () => navigate({ view: "tokens" })),
          go("Einstellungen", () => navigate({ view: "einstellungen" }), "Timing"),
          go("Abmelden", () => {
            void api()
              .logout()
              .catch(() => undefined)
              .finally(() => location.reload());
          }),
        ];
    return [
      go("Neue Aufgabe", () => setCaptureOpen(true), "n"),
      go("Gehe zu Heute", () => navigate({ view: "heute" }), "g h"),
      go("Gehe zu Inbox", () => navigate({ view: "inbox" }), "g i"),
      go("Gehe zu Lose", () => navigate({ view: "lose" }), "g l"),
      go("Gehe zu Bereiche", () => navigate({ view: "bereiche" }), "g b"),
      go("Gehe zu Dev", () => navigate({ view: "dev" }), "g d"),
      go("Gehe zu Phönix", () => navigate({ view: "phoenix" }), "g p"),
      go("Neuer Bereich", () => setStructureEdit({ kind: "area" })),
      go("Neues Projekt", () => setStructureEdit({ kind: "project" })),
      go("Tastenkürzel anzeigen", () => setHelpOpen(true), "?"),
      ...account,
    ];
  };

  const structureItems = (): Item[] => [
    ...areaList().map<Item>((a) => ({
      key: `area:${a.id}`,
      group: "Bereiche",
      label: a.name,
      detail: a.state === "archived" ? "archiviert" : undefined,
      hay: normalize(a.name),
      rank: 1,
      dim: a.state === "archived",
      run: () => navigate({ view: "bereiche", kind: "area", id: a.id }),
    })),
    ...projectList().map<Item>((p) => ({
      key: `project:${p.id}`,
      group: "Projekte",
      label: p.name,
      detail: data.areas[p.area_id]?.name,
      hay: normalize(`${p.name} ${data.areas[p.area_id]?.name ?? ""}`),
      rank: 2,
      dim: p.status === "completed",
      run: () => navigate({ view: "bereiche", kind: "project", id: p.id }),
    })),
  ];

  const taskItems = createMemo((): Item[] => {
    const index = taskIndex();
    const exclude = props.request.mode === "task" ? props.request.excludeIds : null;
    const out: Item[] = [];
    for (const t of index.byId.values()) {
      if (index.isDeleted(t) || exclude?.has(t.id)) continue;
      const where = locationLabel(index.location(t));
      const parent = t.parent_id ? data.tasks[t.parent_id]?.title : undefined;
      const detail = [t.ref, parent ? `↳ ${parent}` : where, isClosed(t.status) ? STATUS_LABELS[t.status] : ""].filter(Boolean).join(" · ");
      out.push({
        key: `task:${t.id}`,
        group: "Aufgaben",
        label: t.title,
        detail,
        hay: normalize(`${t.ref ?? ""} ${t.title} ${where} ${t.description ?? ""} ${t.plan ?? ""}`),
        rank: t.ref && normalize(query().trim()) === normalize(t.ref) ? -1 : 3,
        dim: isClosed(t.status),
        run:
          props.request.mode === "task"
            ? () => (props.request as Extract<PaletteRequest, { mode: "task" }>).onPick(t.id)
            : () => {
                setSelectedId(t.id);
                setDetailId(t.id);
              },
      });
    }
    return out;
  });

  const locationItems = (onPick: (l: Location) => void): Item[] => {
    const item = (key: string, label: string, location: Location, group: string, rank: number, detail?: string): Item => ({
      key,
      group,
      label,
      detail,
      hay: normalize(`${label} ${detail ?? ""}`),
      rank,
      run: () => onPick(location),
    });
    return [
      item("inbox", "Inbox", { kind: "inbox" }, "Stellen", 0),
      item("loose", "Lose", { kind: "loose" }, "Stellen", 0),
      ...areaList()
        .filter((a) => a.state === "active")
        .map((a) => item(`area:${a.id}`, a.name, { kind: "area", id: a.id }, "Bereiche", 1)),
      ...projectList()
        .filter((p) => p.status !== "completed")
        .map((p) => item(`project:${p.id}`, p.name, { kind: "project", id: p.id }, "Projekte", 2, data.areas[p.area_id]?.name)),
    ];
  };

  const results = createMemo((): Item[] => {
    const req = props.request;
    const q = normalize(query().trim());
    let pool: Item[];
    if (req.mode === "location") pool = locationItems(req.onPick);
    else if (req.mode === "task") pool = taskItems();
    else pool = q ? [...actions(), ...structureItems(), ...taskItems()] : actions();
    if (!q) return pool.slice(0, LIMIT);
    const terms = q.split(/\s+/);
    const scored: { item: Item; s: number }[] = [];
    for (const item of pool) {
      const s = score(item, terms, q);
      if (s !== null) scored.push({ item, s });
    }
    scored.sort((a, b) => a.s - b.s || a.item.label.localeCompare(b.item.label, "de"));
    return scored.slice(0, LIMIT).map((x) => x.item);
  });

  function choose(item: Item | undefined) {
    if (!item) return;
    close();
    item.run();
  }

  function moveActive(delta: number) {
    const n = results().length;
    if (n === 0) return;
    setActive((i) => (i + delta + n) % n);
    listEl?.querySelector(`[data-index="${active()}"]`)?.scrollIntoView({ block: "nearest" });
  }

  const title = () => (props.request.mode === "search" ? "Suche" : props.request.title);

  return (
    <Modal label={title()} onClose={close} class="palette">
      <Show when={props.request.mode !== "search"}>
        <div class="palette-title">{title()}</div>
      </Show>
      <input
        ref={input}
        class="palette-input"
        placeholder={props.request.mode === "search" ? "Aufgaben, Projekte, Bereiche, Aktionen …" : `${title()} …`}
        value={query()}
        onInput={(e) => {
          setQuery(e.currentTarget.value);
          setActive(0);
        }}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) {
            e.preventDefault();
            moveActive(1);
          } else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p")) {
            e.preventDefault();
            moveActive(-1);
          } else if (e.key === "Enter") {
            e.preventDefault();
            choose(results()[active()]);
          }
        }}
        aria-label={title()}
        aria-controls="palette-results"
        aria-activedescendant={results()[active()] ? `palette-item-${active()}` : undefined}
        role="combobox"
        aria-expanded="true"
      />
      <ul class="palette-results" id="palette-results" role="listbox" ref={listEl}>
        <For each={results()}>
          {(item, i) => (
            <>
              <Show when={i() === 0 || results()[i() - 1]!.group !== item.group}>
                <li class="palette-group" role="presentation">
                  {item.group}
                </li>
              </Show>
              <li
                id={`palette-item-${i()}`}
                data-index={i()}
                role="option"
                aria-selected={i() === active()}
                class="palette-item"
                classList={{ active: i() === active(), dim: item.dim }}
                onMouseMove={() => setActive(i())}
                onClick={() => choose(item)}
              >
                <span class="palette-label">{item.label}</span>
                <Show when={item.detail}>
                  <span class="palette-detail">{item.detail}</span>
                </Show>
              </li>
            </>
          )}
        </For>
        <Show when={results().length === 0}>
          <li class="empty">Nichts gefunden.</li>
        </Show>
      </ul>
    </Modal>
  );
}

