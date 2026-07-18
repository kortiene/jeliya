/** Canonical navigation state (docs/room-workbench.md, decision 2).
 *
 *  The route IS the navigation state — not a mirror of it. Everything the
 *  shell needs to render (which global destination, which room, which room
 *  tool, whether the inspector is open) is derived from this module on read.
 *  Nothing else may hold navigation state that can disagree with it: the
 *  three-way `tab` / `mobileView` / `roomId` split this replaced could, and
 *  did, contradict itself.
 *
 *  The web keeps these as URL paths. `crates/jeliyad/src/serve.rs` serves
 *  `index.html` for any extensionless unknown path, and Vite dev does the
 *  same, so deep links work in production and under the e2e harness with no
 *  daemon change.
 */

/** Room tools, in tab-strip order. `activity` is the room's workspace — a
 *  real destination (the inspector is closed there), not a synonym for "a
 *  room is selected". */
export const ROOM_DESTS = ['activity', 'people', 'agents', 'files', 'pipes'] as const;
export type RoomDest = (typeof ROOM_DESTS)[number];

/** The room tools that render in the inspector; `activity` is the workspace. */
export type InspectorDest = Exclude<RoomDest, 'activity'>;

export type Route =
  | { kind: 'rooms' }
  | { kind: 'room'; roomId: string; dest: RoomDest; item?: string }
  | { kind: 'fleet' }
  | { kind: 'settings' };

export const ROOMS_ROUTE: Route = { kind: 'rooms' };

/** The room tools that select an individual item (a file id / pipe id) — the
 *  only destinations where a 4th path segment is navigation state (#67). A
 *  selected item deep-links to the workspace AND the item it opens. */
export const ITEM_DESTS: readonly RoomDest[] = ['files', 'pipes'];

function isRoomDest(value: string | undefined): value is RoomDest {
  return value !== undefined && (ROOM_DESTS as readonly string[]).includes(value);
}

function destTakesItem(dest: RoomDest): boolean {
  return ITEM_DESTS.includes(dest);
}

/** Parse a pathname into a route. Total by construction: an unknown path is
 *  not an error state to render, it is simply Rooms (decision 2 — unknown
 *  URLs resolve to a clear recoverable state, and Rooms is the recovery).
 *  `/rooms/:id` and `/rooms/:id/<unknown>` normalize to that room's Activity. */
export function parseRoute(pathname: string): Route {
  let segments: string[];
  try {
    segments = pathname.split('/').filter(Boolean).map(decodeURIComponent);
  } catch {
    // A malformed percent-escape ("/rooms/%zz") throws in decodeURIComponent.
    // A bad URL is a navigation miss, not a crash.
    return ROOMS_ROUTE;
  }
  if (segments.length === 0) return ROOMS_ROUTE;
  if (segments[0] === 'fleet') return { kind: 'fleet' };
  if (segments[0] === 'settings') return { kind: 'settings' };
  if (segments[0] === 'rooms') {
    const roomId = segments[1];
    if (!roomId) return ROOMS_ROUTE;
    const dest = isRoomDest(segments[2]) ? segments[2] : 'activity';
    // A 4th segment is the selected item, but ONLY under a dest that has items
    // (files/pipes). `/rooms/:id/people/:x` ignores the stray segment rather
    // than inventing a selection people cannot have.
    const item = destTakesItem(dest) && segments[3] ? segments[3] : undefined;
    return item !== undefined
      ? { kind: 'room', roomId, dest, item }
      : { kind: 'room', roomId, dest };
  }
  return ROOMS_ROUTE;
}

/** The canonical pathname for a route — one destination, one spelling. */
export function routePath(route: Route): string {
  switch (route.kind) {
    case 'rooms':
      return '/rooms';
    case 'fleet':
      return '/fleet';
    case 'settings':
      return '/settings';
    case 'room': {
      const base = `/rooms/${encodeURIComponent(route.roomId)}/${route.dest}`;
      return route.item !== undefined && destTakesItem(route.dest)
        ? `${base}/${encodeURIComponent(route.item)}`
        : base;
    }
  }
}

export function sameRoute(a: Route, b: Route): boolean {
  return routePath(a) === routePath(b);
}

/** The room a route selects, or null. */
export function routeRoomId(route: Route): string | null {
  return route.kind === 'room' ? route.roomId : null;
}

/** The selected file/pipe id the route deep-links to, or null (#67). Only
 *  files/pipes carry one; other destinations always return null. */
export function routeItem(route: Route): string | null {
  return route.kind === 'room' && route.item !== undefined ? route.item : null;
}

/** The tool the inspector shows, or null when it is closed. Closed is the
 *  `activity` destination — collapsing the inspector *is* navigating there. */
export function inspectorDest(route: Route): InspectorDest | null {
  return route.kind === 'room' && route.dest !== 'activity' ? route.dest : null;
}

/** Legacy `?tab=` links (App.tsx read this once at startup and never wrote it
 *  back). It is a shipped URL surface, so it migrates onto the equivalent room
 *  destination of whichever room is restored rather than 404ing. `members`
 *  became `people`; the rest kept their names. Returns null when the query
 *  carries no legacy tab. */
export function legacyTabDest(search: string): RoomDest | null {
  const tab = new URLSearchParams(search).get('tab');
  switch (tab) {
    case 'members':
      return 'people';
    case 'agents':
      return 'agents';
    case 'files':
      return 'files';
    case 'pipes':
      return 'pipes';
    default:
      return null;
  }
}

/** Strip the migrated `?tab=` while preserving every other param.
 *
 *  `?daemon=` picks the daemon and `?mock…` installs the e2e fixtures, and
 *  both are parsed from `window.location.search` — so a canonicalizing
 *  redirect that dropped the query would silently re-point the client at a
 *  different daemon and unfixture the suite (decision 2, rule 6). Only the
 *  key this module consumes is removed. */
export function searchWithoutLegacyTab(search: string): string {
  const params = new URLSearchParams(search);
  if (!params.has('tab')) return search;
  params.delete('tab');
  const rest = params.toString();
  return rest ? `?${rest}` : '';
}
