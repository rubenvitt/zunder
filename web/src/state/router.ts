// Tiny hash router: "#/inbox", "#/bereiche/project/<id>", "#/phoenix/2026-W41".

import { createSignal } from "solid-js";

export type Route =
  | { view: "heute" }
  | { view: "inbox" }
  | { view: "lose" }
  | { view: "bereiche"; kind?: "area" | "project"; id?: string }
  | { view: "dev" }
  | { view: "phoenix"; fire?: string }
  | { view: "tokens" }
  | { view: "verbindung" };

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
    case "verbindung":
      return { view: "verbindung" };
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

export function navigate(next: Route): void {
  const hash = routeHash(next);
  if (location.hash !== hash) location.hash = hash;
  setRoute(next);
}
