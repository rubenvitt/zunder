// Tiny hash router: "#/inbox", "#/bereiche/project/<id>", "#/phoenix/2026-W41".
// "#/aufgabe/<id or reference like ZUN-42>" and "#/clickup/<clickup_id>" are links to a single task (e.g. from
// the vault): they open the task's detail over its home view.

import { createSignal } from "solid-js";

export type Route =
  | { view: "heute" }
  | { view: "inbox" }
  | { view: "lose" }
  | { view: "bereiche"; kind?: "area" | "project"; id?: string }
  | { view: "dev" }
  | { view: "phoenix"; fire?: string }
  | { view: "tokens" }
  | { view: "einstellungen" }
  | { view: "verbindung" }
  | { view: "aufgabe"; id: string }
  | { view: "clickup"; id: string };

export type ViewName = Route["view"];

export function parseRoute(hash: string): Route {
  const parts = hash.replace(/^#\/?/, "").split("/").filter(Boolean).map(decodeURIComponent);
  switch (parts[0]) {
    case "inbox":
      return { view: "inbox" };
    case "lose":
      return { view: "lose" };
    case "bereiche":
      if ((parts[1] === "area" || parts[1] === "project") && parts[2]) return { view: "bereiche", kind: parts[1], id: parts[2] };
      return { view: "bereiche" };
    case "dev":
      return { view: "dev" };
    case "phoenix":
      return parts[1] ? { view: "phoenix", fire: parts[1] } : { view: "phoenix" };
    case "tokens":
      return { view: "tokens" };
    case "einstellungen":
      return { view: "einstellungen" };
    case "verbindung":
      return { view: "verbindung" };
    case "aufgabe":
    case "clickup":
      return parts[1] ? { view: parts[0], id: parts[1] } : { view: "heute" };
    default:
      return { view: "heute" };
  }
}

export function routeHash(route: Route): string {
  switch (route.view) {
    case "bereiche":
      return route.kind && route.id ? `#/bereiche/${route.kind}/${encodeURIComponent(route.id)}` : "#/bereiche";
    case "phoenix":
      return route.fire ? `#/phoenix/${route.fire}` : "#/phoenix";
    case "aufgabe":
    case "clickup":
      return `#/${route.view}/${encodeURIComponent(route.id)}`;
    default:
      return `#/${route.view}`;
  }
}

const initial = typeof location !== "undefined" ? parseRoute(location.hash) : ({ view: "heute" } as Route);
const [route, setRoute] = createSignal<Route>(initial);

if (typeof window !== "undefined") {
  window.addEventListener("hashchange", () => setRoute(parseRoute(location.hash)));
}

export { route };

export function navigate(next: Route, options: { replace?: boolean } = {}): void {
  const hash = routeHash(next);
  if (location.hash !== hash) {
    if (options.replace) history.replaceState(history.state, "", hash);
    else location.hash = hash;
  }
  setRoute(next);
}
