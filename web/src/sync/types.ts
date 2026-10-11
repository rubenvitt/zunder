// Wire types mirroring the implementation contract (section 2 and 3).

/** HLC string "<wall_ms:13>:<counter:5>:<node>". */
export type Hlc = string;

export type EntityKind = "area" | "project" | "task";

export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

export interface Op {
  op_id: string;
  entity: EntityKind;
  entity_id: string;
  field: string;
  value: JsonValue;
  hlc: Hlc;
}

// ---- Field value types -------------------------------------------------

export type AreaState = "active" | "archived";
export type ProjectStatus = "active" | "paused" | "completed";
export type ProjectKind = "standard" | "dev";
export type ProjectView = "list" | "board";
export type TaskType = "aufgabe" | "bug" | "feature";
export type TaskStatus = "open" | "in_progress" | "review" | "blocked" | "done" | "dropped";
export type PhoenixRole = "feuer_schritt" | "flamme";

export type Location =
  | { kind: "inbox" }
  | { kind: "loose" }
  | { kind: "area"; id: string }
  | { kind: "project"; id: string };

export interface Link {
  url: string;
  title?: string;
}

export interface Phoenix {
  role: PhoenixRole;
  fire: string;
}

export const AREA_STATES: readonly AreaState[] = ["active", "archived"];
export const PROJECT_STATUSES: readonly ProjectStatus[] = ["active", "paused", "completed"];
export const PROJECT_KINDS: readonly ProjectKind[] = ["standard", "dev"];
export const PROJECT_VIEWS: readonly ProjectView[] = ["list", "board"];
export const TASK_TYPES: readonly TaskType[] = ["aufgabe", "bug", "feature"];
export const TASK_STATUSES: readonly TaskStatus[] = ["open", "in_progress", "review", "blocked", "done", "dropped"];
export const PHOENIX_ROLES: readonly PhoenixRole[] = ["feuer_schritt", "flamme"];

export const AREA_FIELDS = ["name", "state", "vault_path", "deleted"] as const;
export const PROJECT_FIELDS = ["name", "area_id", "status", "kind", "repo", "description", "view", "deleted"] as const;
export const TASK_FIELDS = [
  "title",
  "description",
  "type",
  "status",
  "location",
  "parent_id",
  "due",
  "remind_at",
  "vault_links",
  "links",
  "phoenix",
  "claimed_by",
  "claim_expires_at",
  "plan",
  "branch",
  "pr_url",
  "timing_ref",
  "flame_started_at",
  "flame_ended_at",
  "clickup_id",
  "rank",
  "deleted",
] as const;

export type AreaField = (typeof AREA_FIELDS)[number];
export type ProjectField = (typeof PROJECT_FIELDS)[number];
export type TaskField = (typeof TASK_FIELDS)[number];

export const FIELDS_BY_ENTITY: Record<EntityKind, readonly string[]> = {
  area: AREA_FIELDS,
  project: PROJECT_FIELDS,
  task: TASK_FIELDS,
};

// ---- Materialized entities (winning values with defaults applied) ------

interface EntityMeta {
  id: string;
  deleted: boolean;
  /** Smallest known HLC of the entity. */
  created_hlc: Hlc;
  /** Largest known HLC of the entity. */
  updated_hlc: Hlc;
}

export interface Area extends EntityMeta {
  name: string;
  state: AreaState;
  vault_path: string | null;
}

export interface Project extends EntityMeta {
  name: string;
  area_id: string;
  status: ProjectStatus;
  kind: ProjectKind;
  repo: string | null;
  description: string | null;
  view: ProjectView;
}

export interface Task extends EntityMeta {
  title: string;
  description: string | null;
  type: TaskType;
  status: TaskStatus;
  location: Location;
  parent_id: string | null;
  due: string | null;
  remind_at: string | null;
  vault_links: string[];
  links: Link[];
  phoenix: Phoenix | null;
  claimed_by: string | null;
  claim_expires_at: string | null;
  plan: string | null;
  branch: string | null;
  pr_url: string | null;
  timing_ref: string | null;
  flame_started_at: string | null;
  flame_ended_at: string | null;
  clickup_id: string | null;
  rank: string | null;
  /** HLC of the winning status op, or created_hlc when the status was never set. */
  status_hlc: Hlc;
}

// ---- HTTP API payloads ---------------------------------------------------

export interface SnapshotField {
  value: JsonValue;
  hlc: Hlc;
  /** Not part of the minimal contract; used for the op_id tie-break when the server sends it. */
  op_id?: string;
}

export interface SnapshotEntity {
  entity: EntityKind;
  entity_id: string;
  /** Smallest HLC of any op of the entity (derived created_at); optional for older servers. */
  created_hlc?: Hlc;
  fields: Record<string, SnapshotField>;
}

export interface Snapshot {
  cursor: number;
  entities: SnapshotEntity[];
}

export interface Rejection {
  op_id: string;
  code: string;
  message: string;
}

export interface Warning {
  entity_id: string;
  code: string;
  message: string;
}

export interface PushRequest {
  client_id: string;
  ops: Op[];
}

export interface PushResponse {
  accepted: string[];
  rejected: Rejection[];
  warnings: Warning[];
  cursor: number;
}

export interface PullResponse {
  ops: { seq: number; op: Op }[];
  cursor: number;
  has_more: boolean;
}

export interface HistoryEntry {
  seq: number;
  op: Op;
  /** web | api:<token> | mcp:<token> | server */
  source: string;
  /** RFC 3339 (UTC), when the server received the op. */
  received_at?: string;
}

export interface Me {
  user_id: string;
  name: string;
  email: string;
}

export interface ApiToken {
  id: string;
  name: string;
  created_at?: string;
  last_used_at?: string | null;
}

export interface CreatedToken extends ApiToken {
  token: string;
}

/** An app connected via OAuth (e.g. a claude.ai connector), see `GET /api/oauth/clients`. */
export interface OAuthClient {
  id: string;
  name: string;
  redirect_uris: string[];
  created_at?: string;
  last_used_at?: string | null;
}
