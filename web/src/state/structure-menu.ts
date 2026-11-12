// Right click on an area, a project or a comment: the actions their heads and dialogs
// already offer, in one menu. The listener lives in state/context-menu.ts.

import { AREA_STATE_LABELS, PROJECT_STATUS_LABELS } from "../labels";
import { AREA_STATES, PROJECT_STATUSES, PROJECT_VIEWS, type Area, type AreaState, type Comment, type Project, type ProjectStatus } from "../sync/types";
import { appUrl } from "../components/TaskLinks";
import { deleteComment, shorten, updateArea, updateProject } from "./actions";
import type { MenuEntry } from "./menu";
import { navigate } from "./router";
import { data } from "./store";
import { copyText, setStructureEdit, toast } from "./ui";

const VIEW_LABELS = { list: "Liste", board: "Board" } as const;

export function areaHref(area: Area): string {
  return `#/bereiche/area/${encodeURIComponent(area.id)}`;
}

export function projectHref(project: Project): string {
  return `#/bereiche/project/${encodeURIComponent(project.id)}`;
}

function setAreaState(area: Area, state: AreaState): void {
  if (area.state !== state && updateArea(area.id, { state })) toast(`„${shorten(area.name)}“ → ${AREA_STATE_LABELS[state]}`);
}

function setProjectStatus(project: Project, status: ProjectStatus): void {
  if (project.status !== status && updateProject(project.id, { status })) toast(`„${shorten(project.name)}“ → ${PROJECT_STATUS_LABELS[status]}`);
}

/** The menu for one area. */
export function areaMenuItems(area: Area): MenuEntry[] {
  return [
    { kind: "item", label: "Öffnen", run: () => navigate({ view: "bereiche", kind: "area", id: area.id }) },
    { kind: "item", label: "Neues Projekt …", run: () => setStructureEdit({ kind: "project", areaId: area.id }) },
    { kind: "item", label: "Bearbeiten …", run: () => setStructureEdit({ kind: "area", id: area.id }) },
    { kind: "separator" },
    {
      kind: "submenu",
      label: "Zustand",
      items: AREA_STATES.map((s): MenuEntry => ({ kind: "item", label: AREA_STATE_LABELS[s], checked: area.state === s, run: () => setAreaState(area, s) })),
    },
    { kind: "separator" },
    { kind: "item", label: "ID kopieren", run: () => copyText(area.id, `ID kopiert: ${area.id}`) },
    { kind: "item", label: "Link kopieren", run: () => copyText(appUrl(areaHref(area)), "Link kopiert") },
  ];
}

/** The menu for one project. */
export function projectMenuItems(project: Project): MenuEntry[] {
  const items: MenuEntry[] = [
    { kind: "item", label: "Öffnen", run: () => navigate({ view: "bereiche", kind: "project", id: project.id }) },
    { kind: "item", label: "Bearbeiten …", run: () => setStructureEdit({ kind: "project", id: project.id }) },
    { kind: "separator" },
    {
      kind: "submenu",
      label: "Ansicht",
      items: PROJECT_VIEWS.map((v): MenuEntry => ({
        kind: "item",
        label: VIEW_LABELS[v],
        checked: project.view === v,
        run: () => project.view !== v && updateProject(project.id, { view: v }),
      })),
    },
    {
      kind: "submenu",
      label: "Status",
      items: PROJECT_STATUSES.map((s): MenuEntry => ({ kind: "item", label: PROJECT_STATUS_LABELS[s], checked: project.status === s, run: () => setProjectStatus(project, s) })),
    },
    { kind: "separator" },
  ];
  const area = data.areas[project.area_id];
  if (area) items.push({ kind: "item", label: `Bereich öffnen (${shorten(area.name, 24)})`, run: () => navigate({ view: "bereiche", kind: "area", id: area.id }) });
  if (project.key) items.push({ kind: "item", label: `Schlüssel kopieren (${project.key})`, run: () => copyText(project.key!, `Schlüssel kopiert: ${project.key}`) });
  items.push(
    { kind: "item", label: "ID kopieren", run: () => copyText(project.id, `ID kopiert: ${project.id}`) },
    { kind: "item", label: "Link kopieren", run: () => copyText(appUrl(projectHref(project)), "Link kopiert") },
  );
  return items;
}

/** The menu for one comment (the text itself stays editable in place). */
export function commentMenuItems(comment: Comment): MenuEntry[] {
  return [
    { kind: "item", label: "Text kopieren", run: () => copyText(comment.body, "Kommentar kopiert") },
    { kind: "separator" },
    {
      kind: "item",
      label: "Löschen …",
      danger: true,
      run: () => {
        if (confirm("Kommentar löschen?") && deleteComment(comment.id)) toast("Kommentar gelöscht.");
      },
    },
  ];
}
