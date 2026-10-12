// The location quick capture preselects: the list the user is looking at.
// Lose → loose, a selected area or project → that area or project. Views that
// are not a location (Heute, Inbox, Dev, Phönix …) keep the inbox.

import type { Route } from "../state/router";
import type { Location } from "../sync/types";

export interface CaptureContextLookup {
  /** Whether a live area with this ID exists. */
  hasArea(id: string): boolean;
  /** Whether a live project with this ID exists. */
  hasProject(id: string): boolean;
}

/** null = inbox. */
export function captureContext(route: Route, lookup: CaptureContextLookup): Location | null {
  switch (route.view) {
    case "lose":
      return { kind: "loose" };
    case "bereiche":
      if (!route.id) return null;
      if (route.kind === "area" && lookup.hasArea(route.id)) return { kind: "area", id: route.id };
      if (route.kind === "project" && lookup.hasProject(route.id)) return { kind: "project", id: route.id };
      return null;
    default:
      return null;
  }
}
