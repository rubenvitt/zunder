// Pluggable transport. HttpTransport talks to the server (browser); the Tauri
// app provides the same interfaces backed by its commands (platform/tauri.ts).

import type {
  ApiToken,
  CreatedToken,
  HistoryEntry,
  Me,
  OAuthClient,
  PullResponse,
  PushRequest,
  PushResponse,
  Snapshot,
} from "./types";

/** The server answered with an error status. */
export class HttpError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "HttpError";
  }
}

/** No answer at all (offline, DNS, connection reset). */
export class NetworkError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "NetworkError";
  }
}

export function isUnauthorized(err: unknown): boolean {
  return err instanceof HttpError && err.status === 401;
}

/** Errors worth retrying later: network failures, timeouts and server-side errors. */
export function isTransient(err: unknown): boolean {
  if (err instanceof NetworkError) return true;
  return err instanceof HttpError && (err.status >= 500 || err.status === 408 || err.status === 429);
}

export interface SubscriptionHandlers {
  onCursor(cursor: number): void;
  onConnection(connected: boolean): void;
  /**
   * The source state changed in a way a pull cannot express (Tauri app: its
   * own background sync pulled or rolled back operations). The engine then
   * reloads the whole snapshot.
   */
  onReset?(): void;
}

/** What the sync engine needs from the outside world. */
export interface SyncTransport {
  snapshot(): Promise<Snapshot>;
  push(request: PushRequest): Promise<PushResponse>;
  pull(since: number, limit?: number): Promise<PullResponse>;
  /** Live change notifications; returns an unsubscribe function. */
  subscribe(handlers: SubscriptionHandlers): () => void;
}

/** Further server calls used by the UI. */
export interface ApiClient {
  me(): Promise<Me>;
  history(taskId: string): Promise<HistoryEntry[]>;
  flame(taskId: string, action: "start" | "stop"): Promise<void>;
  listTokens(): Promise<ApiToken[]>;
  createToken(name: string): Promise<CreatedToken>;
  revokeToken(id: string): Promise<void>;
  listOAuthClients(): Promise<OAuthClient[]>;
  revokeOAuthClient(id: string): Promise<void>;
  logout(): Promise<void>;
}

/** CSRF header required by the server for cookie-authenticated, state-changing requests. */
export const CLIENT_HEADER = "X-Zunder-Client";

type FetchFn = typeof fetch;

export class HttpTransport implements SyncTransport, ApiClient {
  constructor(
    private readonly baseUrl = "",
    private readonly fetchFn: FetchFn = (...args) => fetch(...args),
  ) {}

  private async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    let response: Response;
    try {
      response = await this.fetchFn(this.baseUrl + path, {
        method,
        credentials: "same-origin",
        // Contract 3.1 (CSRF): every API request of the web UI carries X-Zunder-Client: web.
        headers:
          body === undefined
            ? { Accept: "application/json", [CLIENT_HEADER]: "web" }
            : { Accept: "application/json", "Content-Type": "application/json", [CLIENT_HEADER]: "web" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch (err) {
      throw new NetworkError(err instanceof Error ? err.message : String(err));
    }
    if (!response.ok) {
      let code = `http_${response.status}`;
      let message = `Serverfehler (${response.status}).`;
      try {
        const data = (await response.json()) as { error?: { code?: string; message?: string } };
        if (data.error?.code) code = data.error.code;
        if (data.error?.message) message = data.error.message;
      } catch {
        // Non-JSON error body: keep the generic message.
      }
      throw new HttpError(response.status, code, message);
    }
    if (response.status === 204) return undefined as T;
    const text = await response.text();
    return (text ? JSON.parse(text) : undefined) as T;
  }

  snapshot(): Promise<Snapshot> {
    return this.request("GET", "/api/snapshot");
  }

  push(request: PushRequest): Promise<PushResponse> {
    return this.request("POST", "/api/sync/push", request);
  }

  pull(since: number, limit = 1000): Promise<PullResponse> {
    return this.request("GET", `/api/sync/pull?since=${since}&limit=${limit}`);
  }

  subscribe(handlers: SubscriptionHandlers): () => void {
    if (typeof EventSource === "undefined") {
      handlers.onConnection(false);
      return () => {};
    }
    const source = new EventSource(`${this.baseUrl}/api/sync/events`, { withCredentials: true });
    source.addEventListener("open", () => handlers.onConnection(true));
    source.addEventListener("error", () => handlers.onConnection(false));
    source.addEventListener("cursor", (event) => {
      const cursor = Number((event as MessageEvent<string>).data);
      if (Number.isFinite(cursor)) handlers.onCursor(cursor);
    });
    return () => source.close();
  }

  me(): Promise<Me> {
    return this.request("GET", "/api/me");
  }

  history(taskId: string): Promise<HistoryEntry[]> {
    return this.request<HistoryEntry[] | { entries: HistoryEntry[] }>("GET", `/api/tasks/${encodeURIComponent(taskId)}/history`).then(
      (data) => (Array.isArray(data) ? data : (data?.entries ?? [])),
    );
  }

  async flame(taskId: string, action: "start" | "stop"): Promise<void> {
    await this.request("POST", `/api/tasks/${encodeURIComponent(taskId)}/flame/${action}`);
  }

  listTokens(): Promise<ApiToken[]> {
    return this.request<ApiToken[] | { tokens: ApiToken[] }>("GET", "/api/tokens").then((data) =>
      Array.isArray(data) ? data : (data?.tokens ?? []),
    );
  }

  createToken(name: string): Promise<CreatedToken> {
    return this.request("POST", "/api/tokens", { name });
  }

  async revokeToken(id: string): Promise<void> {
    await this.request("DELETE", `/api/tokens/${encodeURIComponent(id)}`);
  }

  listOAuthClients(): Promise<OAuthClient[]> {
    return this.request<OAuthClient[] | { clients: OAuthClient[] }>("GET", "/api/oauth/clients").then((data) =>
      Array.isArray(data) ? data : (data?.clients ?? []),
    );
  }

  async revokeOAuthClient(id: string): Promise<void> {
    await this.request("DELETE", `/api/oauth/clients/${encodeURIComponent(id)}`);
  }

  async logout(): Promise<void> {
    await this.request("POST", "/auth/logout");
  }
}
