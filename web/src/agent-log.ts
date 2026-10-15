// Agent log in words: one line per entity a commit of an agent (MCP) changed.

import type { IconName } from "./components/Icon";
import { describeValue, FIELD_LABELS } from "./labels";
import { data } from "./state/store";
import type { AgentChange, JsonValue } from "./sync/types";

/** Fields that say nothing to a reader (ordering, server bookkeeping). */
const HIDDEN_FIELDS = new Set(["rank", "ref", "deleted"]);

export interface ChangeSummary {
  icon: IconName;
  /** Text before the name, e.g. "Aufgabe". */
  lead: string;
  name: string;
  href?: string;
  /** Text after the name, e.g. "angelegt". */
  verb: string;
  /** "Status → Erledigt" and the like. */
  details: string[];
}

function lastValue(change: AgentChange, field: string): JsonValue | undefined {
  const op = change.ops.findLast((o) => o.field === field);
  return op?.value;
}

function text(value: JsonValue | undefined): string | undefined {
  return typeof value === "string" && value.trim() ? value : undefined;
}

function taskName(id: string | undefined, fallback?: string): { name: string; href?: string } {
  const task = id ? data.tasks[id] : undefined;
  if (task && !task.deleted) return { name: task.title, href: `#/aufgabe/${encodeURIComponent(task.ref ?? task.id)}` };
  return { name: task?.title ?? fallback ?? "unbekannte Aufgabe" };
}

/** One line of the log in words. Names come from the current state, else from the change itself. */
export function describeChange(change: AgentChange): ChangeSummary {
  const deleted = change.ops.some((o) => o.field === "deleted" && o.value === true);
  const verb = change.created ? "angelegt" : deleted ? "gelöscht" : "geändert";
  const details = change.created || deleted ? [] : fieldChanges(change);
  const id = change.entity_id;
  switch (change.entity) {
    case "task": {
      const { name, href } = taskName(id, text(lastValue(change, "title")));
      return { icon: deleted ? "trash" : change.created ? "plus" : "pencil", lead: "Aufgabe", name, href, verb, details };
    }
    case "comment": {
      const taskId = text(lastValue(change, "task_id")) ?? data.comments[id]?.task_id;
      const { name, href } = taskName(taskId);
      const body = text(lastValue(change, "body"));
      const snippet = body && !deleted ? [body.length > 120 ? `„${body.slice(0, 119)}…“` : `„${body}“`] : [];
      return {
        icon: "comment",
        lead: "Kommentar zu",
        name,
        href,
        verb: change.created ? "geschrieben" : verb,
        details: snippet,
      };
    }
    case "project": {
      const project = data.projects[id];
      const name = project?.name ?? text(lastValue(change, "name")) ?? "unbekanntes Projekt";
      const href = project && !project.deleted ? `#/bereiche/project/${encodeURIComponent(id)}` : undefined;
      return { icon: "folder", lead: "Projekt", name, href, verb, details };
    }
    case "area": {
      const area = data.areas[id];
      const name = area?.name ?? text(lastValue(change, "name")) ?? "unbekannter Bereich";
      const href = area && !area.deleted ? `#/bereiche/area/${encodeURIComponent(id)}` : undefined;
      return { icon: "folder", lead: "Bereich", name, href, verb, details };
    }
  }
}

/** "Feld → Wert" per changed field (the last value wins), in log order. */
function fieldChanges(change: AgentChange): string[] {
  const last = new Map<string, JsonValue>();
  for (const op of change.ops) {
    if (HIDDEN_FIELDS.has(op.field)) continue;
    last.delete(op.field);
    last.set(op.field, op.value);
  }
  return [...last].map(([field, value]) => `${FIELD_LABELS[field] ?? field} → ${describeValue(field, value)}`);
}
