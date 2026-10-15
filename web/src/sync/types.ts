// Wire types mirroring the implementation contract (section 2 and 3).

/** HLC string "<wall_ms:13>:<counter:5>:<node>". */
export type Hlc = string;

export type EntityKind = "area" | "project" | "task" | "comment";

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
export type RecurrenceUnit = "day" | "week" | "month" | "year";
export type RecurrenceFrom = "due" | "done";
export type WeekdayCode = "mo" | "tu" | "we" | "th" | "fr" | "sa" | "su";

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

/**
 * Recurrence of a task (contract 2.4): every `every` units, optionally on given weekdays
 * (weekly) or a given day of the month (monthly, yearly), anchored at the due date or the completion.
 */
export interface Recurrence {
  every: number;
  unit: RecurrenceUnit;
  /** Only with unit "week": non-empty, distinct, in stored order. */
  weekdays?: WeekdayCode[];
  /** Only with unit "month" or "year" (yearly: in the month of the due date): 1–31 (shorter months: their last day) or "last". */
  day?: number | "last";
  from: RecurrenceFrom;
}

export const AREA_STATES: readonly AreaState[] = ["active", "archived"];
export const PROJECT_STATUSES: readonly ProjectStatus[] = ["active", "paused", "completed"];
export const PROJECT_KINDS: readonly ProjectKind[] = ["standard", "dev"];
export const PROJECT_VIEWS: readonly ProjectView[] = ["list", "board"];
export const TASK_TYPES: readonly TaskType[] = ["aufgabe", "bug", "feature"];
export const TASK_STATUSES: readonly TaskStatus[] = ["open", "in_progress", "review", "blocked", "done", "dropped"];
export const PHOENIX_ROLES: readonly PhoenixRole[] = ["feuer_schritt", "flamme"];
export const RECURRENCE_UNITS: readonly RecurrenceUnit[] = ["day", "week", "month", "year"];
export const RECURRENCE_FROMS: readonly RecurrenceFrom[] = ["due", "done"];
export const WEEKDAY_CODES: readonly WeekdayCode[] = ["mo", "tu", "we", "th", "fr", "sa", "su"];

export const AREA_FIELDS = ["name", "state", "vault_path", "deleted"] as const;
export const PROJECT_FIELDS = ["name", "area_id", "status", "kind", "repo", "description", "view", "key", "deleted"] as const;
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
  "undated_ack",
  "recurrence",
  "recurrence_prev",
  "blocked_by",
  "ref",
  "deleted",
] as const;

export const COMMENT_FIELDS = ["task_id", "body", "author", "deleted"] as const;

export type AreaField = (typeof AREA_FIELDS)[number];
export type ProjectField = (typeof PROJECT_FIELDS)[number];
export type TaskField = (typeof TASK_FIELDS)[number];
export type CommentField = (typeof COMMENT_FIELDS)[number];

export const FIELDS_BY_ENTITY: Record<EntityKind, readonly string[]> = {
  area: AREA_FIELDS,
  project: PROJECT_FIELDS,
  task: TASK_FIELDS,
  comment: COMMENT_FIELDS,
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
  /** Prefix of the task references (`ZUN` → `ZUN-42`). */
  key: string | null;
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
  /** When the task was marked as deliberately undated ("bewusst undatiert"). */
  undated_ack: string | null;
  /** Set = the task is recurring. */
  recurrence: Recurrence | null;
  /** Previous instance of the series, written by the server only (may name a deleted task). */
  recurrence_prev: string | null;
  /** IDs of the tasks this one waits for, as stored (may name deleted tasks, which no longer block). */
  blocked_by: string[];
  /** Reference such as `ZUN-42`, assigned by the server (never by a client). */
  ref: string | null;
  /** HLC of the winning status op, or created_hlc when the status was never set. */
  status_hlc: Hlc;
}

export interface Comment extends EntityMeta {
  task_id: string;
  /** Markdown. */
  body: string;
  /** Agent or tool that wrote it; null = the owner. */
  author: string | null;
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

/** One commit of an agent (MCP tool call) in `GET /api/agent-log`. */
export interface AgentAction {
  /** Largest `seq` of the commit. */
  seq: number;
  /** mcp:<token> | mcp:oauth:<client> */
  source: string;
  /** RFC 3339 (UTC). */
  received_at: string;
  changes: AgentChange[];
}

/** The operations of one commit on one entity. */
export interface AgentChange {
  entity: EntityKind;
  entity_id: string;
  /** The commit created the entity. */
  created: boolean;
  ops: Op[];
}

export interface AgentLogPage {
  /** Newest first. */
  actions: AgentAction[];
  /** `before` for older actions; null at the end. */
  next_before: number | null;
}

export interface HistoryEntry {
  seq: number;
  op: Op;
  /** web | api:<token> | mcp:<token> | server */
  source: string;
  /** RFC 3339 (UTC), when the server received the op. */
  received_at?: string;
}

/**
 * Answer of flame/start|stop (contract 4.2), reduced to what the UI reads. The Timing
 * part tells whether a Timing timer was started/stopped; a failure is not an HTTP error.
 */
export interface FlameResult {
  unchanged?: boolean;
  timing?: { configured?: boolean; used?: boolean; error?: string | null };
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

/** An intake channel: lets another server create tasks in one project, see `GET /api/intake/channels`. */
export interface IntakeChannel {
  id: string;
  name: string;
  project_id: string;
  /** Name of the target project; null when the server does not know it. */
  project_name: string | null;
  created_at?: string;
  last_used_at?: string | null;
}

/** A newly created intake channel; `token` (`zi_…`) is shown only once. */
export interface CreatedIntakeChannel extends IntakeChannel {
  token: string;
}

/** Where an effective setting comes from: the settings view or an environment variable. */
export type SettingSource = "settings" | "env";

/** Timing bridge settings, see `GET /api/settings/timing` (never contains the token itself). */
export interface TimingSettings {
  configured: boolean;
  token_source: SettingSource | null;
  /** Last characters of the token in effect, e.g. "…abcd". */
  token_hint: string | null;
  project: string | null;
  project_source: SettingSource | null;
  /** Whether stored settings override the environment. */
  overridden: boolean;
  env_token: boolean;
  env_project: string | null;
}

/** `PUT /api/settings/timing`: missing/empty token keeps it; missing project keeps it, "" removes it. */
export interface TimingSettingsUpdate {
  token?: string;
  project?: string;
}

/** A Timing project, see `GET /api/settings/timing/projects`. */
export interface TimingProject {
  /** Self link, e.g. "/projects/1"; what the settings store. */
  self: string;
  title: string;
  /** Titles from the root project down to this one. */
  title_chain: string[];
  color: string | null;
}

/** `POST /api/settings/timing/check`. */
export interface TimingCheck {
  ok: boolean;
  error: string | null;
}

/** Calendar feed, see `GET /api/settings/calendar` (never contains the feed URL). */
export interface CalendarSettings {
  enabled: boolean;
  created_at: string | null;
}

/** `POST /api/settings/calendar`: a new feed token; its URLs are shown only in this answer. */
export interface CreatedCalendarFeed extends CalendarSettings {
  /** Flames only. */
  url: string;
  /** Flames and every other task with a due time. */
  url_all: string;
}

/** A device logged in with the Zunder app, see `GET /api/devices`. */
export interface Device {
  id: string;
  name: string;
  created_at?: string;
  last_used_at?: string | null;
}

/** An app connected via OAuth (e.g. a claude.ai connector), see `GET /api/oauth/clients`. */
export interface OAuthClient {
  id: string;
  name: string;
  redirect_uris: string[];
  created_at?: string;
  last_used_at?: string | null;
}
