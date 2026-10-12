// Create/edit/archive dialog for areas and projects.

import { createSignal, For, Show } from "solid-js";
import { AREA_STATES, PROJECT_KINDS, PROJECT_STATUSES, PROJECT_VIEWS, type JsonValue, type ProjectKind } from "../sync/types";
import { AREA_STATE_LABELS, PROJECT_KIND_LABELS, PROJECT_STATUS_LABELS } from "../labels";
import { createArea, createProject, updateArea, updateProject } from "../state/actions";
import { navigate } from "../state/router";
import { areaList, data } from "../state/store";
import { setStructureEdit, type StructureEdit } from "../state/ui";
import { Modal } from "./Modal";

export function StructureForm(props: { edit: StructureEdit }) {
  const close = () => setStructureEdit(null);
  return (
    <Show when={props.edit.kind === "area"} fallback={<ProjectForm edit={props.edit as Extract<StructureEdit, { kind: "project" }>} onClose={close} />}>
      <AreaForm id={props.edit.id} onClose={close} />
    </Show>
  );
}

function AreaForm(props: { id?: string; onClose: () => void }) {
  const area = props.id ? data.areas[props.id] : undefined;
  const [name, setName] = createSignal(area?.name ?? "");
  const [state, setState] = createSignal(area?.state ?? "active");
  const [vaultPath, setVaultPath] = createSignal(area?.vault_path ?? "");

  function save(e: Event) {
    e.preventDefault();
    if (!area) {
      const id = createArea(name());
      if (id) {
        props.onClose();
        navigate({ view: "bereiche", kind: "area", id });
      }
      return;
    }
    const fields: Record<string, JsonValue> = {};
    if (name().trim() !== area.name) fields.name = name().trim();
    if (state() !== area.state) fields.state = state();
    if ((vaultPath().trim() || null) !== area.vault_path) fields.vault_path = vaultPath().trim() || null;
    if (Object.keys(fields).length === 0 || updateArea(area.id, fields)) props.onClose();
  }

  return (
    <Modal label={area ? "Bereich bearbeiten" : "Neuer Bereich"} onClose={props.onClose}>
      <form class="form" onSubmit={save}>
        <h2>{area ? "Bereich bearbeiten" : "Neuer Bereich"}</h2>
        <label>
          Name
          <input class="field" name="name" required autofocus value={name()} onInput={(e) => setName(e.currentTarget.value)} />
        </label>
        <Show when={area}>
          <label>
            Zustand
            <select class="field" value={state()} onChange={(e) => setState(e.currentTarget.value as typeof AREA_STATES[number])}>
              <For each={AREA_STATES}>{(s) => <option value={s}>{AREA_STATE_LABELS[s]}</option>}</For>
            </select>
          </label>
          <label>
            Vault-Pfad <span class="muted">(nur Info; verknüpft wird über die ID)</span>
            <input class="field mono" value={vaultPath()} onInput={(e) => setVaultPath(e.currentTarget.value)} />
          </label>
          <p class="muted mono small">zunder_id: {area!.id}</p>
        </Show>
        <div class="modal-actions">
          <Show when={area && area.state === "active"}>
            <button
              type="button"
              class="btn btn-ghost"
              onClick={() => {
                if (updateArea(area!.id, { state: "archived" })) props.onClose();
              }}
            >
              Archivieren
            </button>
          </Show>
          <span class="spacer" />
          <button type="button" class="btn btn-ghost" onClick={props.onClose}>
            Abbrechen
          </button>
          <button type="submit" class="btn btn-primary" disabled={!name().trim()}>
            Speichern
          </button>
        </div>
      </form>
    </Modal>
  );
}

function ProjectForm(props: { edit: Extract<StructureEdit, { kind: "project" }>; onClose: () => void }) {
  const project = props.edit.id ? data.projects[props.edit.id] : undefined;
  const activeAreas = () => areaList().filter((a) => a.state === "active" || a.id === project?.area_id);
  const [name, setName] = createSignal(project?.name ?? "");
  const [areaId, setAreaId] = createSignal(project?.area_id ?? props.edit.areaId ?? activeAreas()[0]?.id ?? "");
  const [status, setStatus] = createSignal(project?.status ?? "active");
  const [kind, setKind] = createSignal<ProjectKind>(project?.kind ?? "standard");
  const [repo, setRepo] = createSignal(project?.repo ?? "");
  const [view, setView] = createSignal(project?.view ?? "list");
  const [description, setDescription] = createSignal(project?.description ?? "");
  const [key, setKey] = createSignal(project?.key ?? "");

  function save(e: Event) {
    e.preventDefault();
    if (!project) {
      const id = createProject(name(), areaId(), kind(), repo().trim() || null, key().trim() || null);
      if (id) {
        props.onClose();
        navigate({ view: "bereiche", kind: "project", id });
      }
      return;
    }
    const fields: Record<string, JsonValue> = {};
    if (name().trim() !== project.name) fields.name = name().trim();
    if (areaId() !== project.area_id) fields.area_id = areaId();
    if (status() !== project.status) fields.status = status();
    if (kind() !== project.kind) fields.kind = kind();
    const repoValue = kind() === "dev" ? repo().trim() || null : null;
    if (repoValue !== project.repo) fields.repo = repoValue;
    if (view() !== project.view) fields.view = view();
    if ((description() || null) !== project.description) fields.description = description() || null;
    if ((key().trim() || null) !== project.key) fields.key = key().trim() || null;
    if (Object.keys(fields).length === 0 || updateProject(project.id, fields)) props.onClose();
  }

  return (
    <Modal label={project ? "Projekt bearbeiten" : "Neues Projekt"} onClose={props.onClose}>
      <form class="form" onSubmit={save}>
        <h2>{project ? "Projekt bearbeiten" : "Neues Projekt"}</h2>
        <label>
          Name
          <input class="field" name="name" required autofocus value={name()} onInput={(e) => setName(e.currentTarget.value)} />
        </label>
        <label>
          Bereich
          <select class="field" name="area" value={areaId()} onChange={(e) => setAreaId(e.currentTarget.value)} required>
            <For each={activeAreas()}>{(a) => <option value={a.id}>{a.name}</option>}</For>
          </select>
        </label>
        <div class="form-row">
          <label>
            Art
            <select class="field" name="kind" value={kind()} onChange={(e) => setKind(e.currentTarget.value as ProjectKind)}>
              <For each={PROJECT_KINDS}>{(k) => <option value={k}>{PROJECT_KIND_LABELS[k]}</option>}</For>
            </select>
          </label>
          <Show when={project}>
            <label>
              Status
              <select class="field" value={status()} onChange={(e) => setStatus(e.currentTarget.value as typeof PROJECT_STATUSES[number])}>
                <For each={PROJECT_STATUSES}>{(s) => <option value={s}>{PROJECT_STATUS_LABELS[s]}</option>}</For>
              </select>
            </label>
            <label>
              Ansicht
              <select class="field" value={view()} onChange={(e) => setView(e.currentTarget.value as typeof PROJECT_VIEWS[number])}>
                <For each={PROJECT_VIEWS}>{(v) => <option value={v}>{v === "list" ? "Liste" : "Board"}</option>}</For>
              </select>
            </label>
          </Show>
        </div>
        <label>
          Schlüssel <span class="muted">(für Kennungen wie {key().trim() || "ZUN"}-42; Großbuchstaben und Ziffern)</span>
          <input
            class="field mono field-short"
            name="key"
            placeholder="ZUN"
            maxLength={10}
            autocapitalize="characters"
            spellcheck={false}
            value={key()}
            onInput={(e) => setKey(e.currentTarget.value.toUpperCase())}
          />
        </label>
        <Show when={project?.key && key().trim() !== project.key}>
          <p class="muted small">Vergebene Kennungen bleiben; neue Aufgaben zählen unter dem neuen Schlüssel.</p>
        </Show>
        <Show when={kind() === "dev"}>
          <label>
            GitHub-Repository
            <input class="field mono" name="repo" placeholder="owner/name" value={repo()} onInput={(e) => setRepo(e.currentTarget.value)} />
          </label>
        </Show>
        <Show when={project}>
          <label>
            Beschreibung
            <textarea class="field" rows={4} value={description()} onInput={(e) => setDescription(e.currentTarget.value)} />
          </label>
          <p class="muted mono small">zunder_id: {project!.id}</p>
        </Show>
        <div class="modal-actions">
          <Show when={project && project.status !== "completed"}>
            <button
              type="button"
              class="btn btn-ghost"
              onClick={() => {
                if (updateProject(project!.id, { status: "completed" })) props.onClose();
              }}
            >
              Abschließen
            </button>
          </Show>
          <span class="spacer" />
          <button type="button" class="btn btn-ghost" onClick={props.onClose}>
            Abbrechen
          </button>
          <button type="submit" class="btn btn-primary" disabled={!name().trim() || !areaId()}>
            Speichern
          </button>
        </div>
      </form>
    </Modal>
  );
}
