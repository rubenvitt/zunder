// Bereiche → Projekte: tree on the left, tasks (list or board) on the right.

import { createEffect, createMemo, createSignal, For, on, onMount, Show } from "solid-js";
import { Board } from "../components/Board";
import { EmptyState } from "../components/EmptyState";
import { Icon } from "../components/Icon";
import { TaskList } from "../components/TaskList";
import type { Area, Project } from "../sync/types";
import { PROJECT_STATUS_LABELS } from "../labels";
import { createTask, updateProject } from "../state/actions";
import { navigate, route } from "../state/router";
import { areaList, data, projectList } from "../state/store";
import { archivedAt, tasksAt } from "../state/selectors";
import { registerList, setStructureEdit, toggleTree, treeHidden } from "../state/ui";

export function StructureView() {
  const [showArchived, setShowArchived] = createSignal(false);
  // Phones: with a selection the tree folds into a picker line above the tasks.
  const [treeOpen, setTreeOpen] = createSignal(false);
  const current = () => {
    const r = route();
    return r.view === "bereiche" ? r : null;
  };
  const areas = () => areaList().filter((a) => showArchived() || a.state === "active");
  const projectsOf = (area: Area) => projectList().filter((p) => p.area_id === area.id && (showArchived() || p.status !== "completed"));
  const isActive = (kind: "area" | "project", id: string) => current()?.kind === kind && current()?.id === id;
  const selectionLabel = () => {
    const c = current();
    if (!c?.id) return "";
    if (c.kind === "project") {
      const p = data.projects[c.id];
      return p ? `${data.areas[p.area_id]?.name ?? ""} › ${p.name}` : "";
    }
    return data.areas[c.id]?.name ?? "";
  };
  createEffect(on(current, () => setTreeOpen(false), { defer: true }));

  return (
    <div class="view view-split" classList={{ "has-selection": !!current()?.id, "tree-open": treeOpen(), "tree-hidden": treeHidden() && !!current()?.id }}>
      <button class="tree-picker" aria-expanded={treeOpen()} onClick={() => setTreeOpen((o) => !o)}>
        <Icon name="folder" size={16} />
        <span>{selectionLabel()}</span>
        <Icon name={treeOpen() ? "up" : "down"} size={14} />
      </button>
      <nav class="tree" aria-label="Bereiche und Projekte">
        <header class="tree-head">
          <h1>Bereiche</h1>
          <span class="spacer" />
          <button class="btn btn-ghost btn-icon" title="Neuer Bereich" aria-label="Neuer Bereich" onClick={() => setStructureEdit({ kind: "area" })}>
            <Icon name="plus" />
          </button>
          <Show when={current()?.id}>
            <button class="btn btn-ghost btn-icon tree-toggle" title="Bereiche ausblenden" aria-label="Bereiche ausblenden" onClick={toggleTree}>
              <Icon name="left" />
            </button>
          </Show>
        </header>
        <ul>
          <For each={areas()} fallback={<li class="empty">Noch keine Bereiche.</li>}>
            {(area) => (
              <li>
                <a
                  href={`#/bereiche/area/${area.id}`}
                  class="tree-item"
                  classList={{ active: isActive("area", area.id), dim: area.state === "archived" }}
                >
                  <Icon name="folder" size={16} />
                  {area.name}
                </a>
                <ul>
                  <For each={projectsOf(area)}>
                    {(project) => (
                      <li>
                        <a
                          href={`#/bereiche/project/${project.id}`}
                          class="tree-item tree-project"
                          classList={{ active: isActive("project", project.id), dim: project.status !== "active" }}
                        >
                          <Show when={project.kind === "dev"} fallback={<span class="dot" />}>
                            <Icon name="code" size={14} />
                          </Show>
                          {project.name}
                        </a>
                      </li>
                    )}
                  </For>
                </ul>
              </li>
            )}
          </For>
        </ul>
        <label class="toggle small">
          <input type="checkbox" checked={showArchived()} onChange={(e) => setShowArchived(e.currentTarget.checked)} /> Archivierte zeigen
        </label>
      </nav>
      <div class="split-main">
        <Show
          when={current()?.kind && current()?.id}
          fallback={
            <Show
              when={areaList().length > 0}
              fallback={
                <EmptyState icon="folder" title="Noch keine Bereiche." hint="Bereiche bündeln Projekte und Aufgaben, etwa Arbeit, Zuhause oder Gesundheit.">
                  <button class="btn btn-small" onClick={() => setStructureEdit({ kind: "area" })}>
                    Bereich anlegen
                  </button>
                </EmptyState>
              }
            >
              <EmptyState icon="folderOpen" title="Bereich oder Projekt links auswählen." />
            </Show>
          }
        >
          <Show
            when={current()!.kind === "project" ? data.projects[current()!.id!] : undefined}
            fallback={
              <Show when={data.areas[current()!.id!]} fallback={<EmptyState icon="search" title="Nicht gefunden." hint="Der Bereich oder das Projekt existiert nicht mehr." />}>
                {(area) => <AreaPanel area={area()} />}
              </Show>
            }
          >
            {(project) => <ProjectPanel project={project()} />}
          </Show>
        </Show>
      </div>
    </div>
  );
}

/** Shown in the panel head while the tree is hidden, to bring it back. */
function ShowTreeButton() {
  return (
    <Show when={treeHidden()}>
      <button class="btn btn-ghost btn-icon tree-toggle" title="Bereiche einblenden" aria-label="Bereiche einblenden" onClick={toggleTree}>
        <Icon name="right" />
      </button>
    </Show>
  );
}

function AddTaskInline(props: { onAdd: (title: string) => void; label: string }) {
  const [title, setTitle] = createSignal("");
  return (
    <form
      class="add-row add-task"
      onSubmit={(e) => {
        e.preventDefault();
        if (!title().trim()) return;
        props.onAdd(title());
        setTitle("");
      }}
    >
      <Icon name="plus" size={16} />
      <input class="add-input" placeholder={props.label} aria-label={props.label} value={title()} onInput={(e) => setTitle(e.currentTarget.value)} />
      <Show when={title().trim()}>
        <button class="btn btn-primary btn-small" type="submit" aria-label="Aufgabe anlegen">
          Anlegen
        </button>
      </Show>
    </form>
  );
}

function AreaPanel(props: { area: Area }) {
  const tasks = createMemo(() => tasksAt({ kind: "area", id: props.area.id }));
  const projects = () => projectList().filter((p) => p.area_id === props.area.id);
  onMount(() => registerList(tasks));
  return (
    <>
      <header class="view-head">
        <ShowTreeButton />
        <h1>{props.area.name}</h1>
        <Show when={props.area.state === "archived"}>
          <span class="chip">Archiviert</span>
        </Show>
        <span class="spacer" />
        <button class="btn btn-ghost btn-small" onClick={() => setStructureEdit({ kind: "project", areaId: props.area.id })}>
          <Icon name="plus" size={14} /> Projekt
        </button>
        <button class="btn btn-ghost btn-small" onClick={() => setStructureEdit({ kind: "area", id: props.area.id })}>
          <Icon name="pencil" size={14} /> Bearbeiten
        </button>
      </header>
      <Show when={projects().length > 0}>
        <div class="project-chips">
          <For each={projects()}>
            {(p) => (
              <button class="chip chip-button" onClick={() => navigate({ view: "bereiche", kind: "project", id: p.id })}>
                {p.name}
                <Show when={p.status !== "active"}>
                  <span class="muted"> · {PROJECT_STATUS_LABELS[p.status]}</span>
                </Show>
              </button>
            )}
          </For>
        </div>
      </Show>
      <AddTaskInline label="Aufgabe im Bereich anlegen …" onAdd={(title) => createTask({ title, location: { kind: "area", id: props.area.id } })} />
      <TaskList
        tasks={tasks()}
        label={`Aufgaben in ${props.area.name}`}
        empty={<EmptyState compact icon="check" title="Keine laufenden Aufgaben ohne Projekt." />}
      />
    </>
  );
}

function ProjectPanel(props: { project: Project }) {
  const tasks = createMemo(() => tasksAt({ kind: "project", id: props.project.id }));
  const boardTasks = createMemo(() => tasks().filter((t) => t.status !== "dropped"));
  const dev = () => props.project.kind === "dev";
  const toggleView = () => updateProject(props.project.id, { view: props.project.view === "board" ? "list" : "board" });

  return (
    <>
      <header class="view-head">
        <ShowTreeButton />
        <h1>{props.project.name}</h1>
        <span class="muted">{data.areas[props.project.area_id]?.name}</span>
        <Show when={dev()}>
          <span class="chip chip-type-feature">
            <Icon name="code" size={12} />
            Dev{props.project.repo ? ` · ${props.project.repo}` : ""}
          </span>
        </Show>
        <Show when={props.project.status !== "active"}>
          <span class="chip">{PROJECT_STATUS_LABELS[props.project.status]}</span>
        </Show>
        <span class="spacer" />
        <div class="segmented" role="group" aria-label="Ansicht">
          <button classList={{ active: props.project.view === "list" }} aria-pressed={props.project.view === "list"} onClick={() => props.project.view !== "list" && toggleView()} title="Liste (v)">
            <Icon name="list" size={16} /> Liste
          </button>
          <button classList={{ active: props.project.view === "board" }} aria-pressed={props.project.view === "board"} onClick={() => props.project.view !== "board" && toggleView()} title="Board (v)">
            <Icon name="board" size={16} /> Board
          </button>
        </div>
        <button class="btn btn-ghost btn-small" onClick={() => setStructureEdit({ kind: "project", id: props.project.id })}>
          <Icon name="pencil" size={14} /> Bearbeiten
        </button>
      </header>
      <AddTaskInline label="Aufgabe im Projekt anlegen …" onAdd={(title) => createTask({ title, location: { kind: "project", id: props.project.id } })} />
      <Show when={props.project.view === "board"} fallback={<ProjectList tasks={tasks()} name={props.project.name} />}>
        <Board tasks={boardTasks()} dev={dev()} label={`Board ${props.project.name}`} archive={() => archivedAt({ kind: "project", id: props.project.id })} />
      </Show>
    </>
  );
}

function ProjectList(props: { tasks: ReturnType<typeof tasksAt>; name: string }) {
  onMount(() => registerList(() => props.tasks));
  return <TaskList tasks={props.tasks} label={`Aufgaben in ${props.name}`} empty={<EmptyState compact icon="list" title="Noch keine Aufgaben." hint="Oben eine Aufgabe anlegen." />} />;
}

export function toggleCurrentProjectView(): boolean {
  const r = route();
  if (r.view !== "bereiche" || r.kind !== "project" || !r.id) return false;
  const project = data.projects[r.id];
  if (!project) return false;
  return updateProject(project.id, { view: project.view === "board" ? "list" : "board" });
}
