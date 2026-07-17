// The searchable, stateful room-list projection (issue #64). A single pure
// function turns the raw `room.list` result plus this device's search, lifecycle
// filter, and pin/archive choices into an ordered, sectioned view. Both clients
// import it (the Dart mirror is app/lib/src/session/room_list.dart) so React and
// Flutter section, order, and disambiguate a room list identically — the shared
// room-list.fixtures.json is the parity guard.
//
// It renders no recency, unread, or attention itself: those are per-row claims
// the surface layers on from the #63 evidence primitives (lastSeen.ts), and only
// where the evidence exists (docs/room-attention.md, decision 6). This file
// decides *which rooms show, in what order, under which section* — nothing that
// asserts a fact about a room's contents.

import type { RoomSummary } from './protocol';
import { homonymousRoomIds, roomDisplayName } from './rooms';

/** Which lifecycle the list shows. `departed` is the signed Left/Removed set
 *  (docs/room-workbench.md, decision 4). A filter separates lifecycles; it never
 *  drops a room from existence — every room is reachable under some filter. */
export type LifecycleFilter = 'all' | 'active' | 'departed';

/** The sections a projected list can contain, in canonical render order. Only
 *  non-empty sections are emitted. `pinned` floats above lifecycle; `archived`
 *  is the device-local put-away bucket, orthogonal to the lifecycle filter. */
export type RoomSectionKey = 'pinned' | 'active' | 'departed' | 'archived';

export interface RoomListRow {
  room: RoomSummary;
  /** The name the row shows — the label or the untitled placeholder. */
  displayName: string;
  /** True iff this room shares a display name with another room in the SAME
   *  visible result (docs/room-workbench.md, decision 6). Recomputed over the
   *  filtered/searched subset, not the full list: the short-id disambiguator is
   *  noise when the twin that made it a homonym is not even on screen. */
  isHomonym: boolean;
}

export interface RoomListSection {
  key: RoomSectionKey;
  rows: RoomListRow[];
}

export interface RoomListView {
  /** Non-empty sections in canonical order (pinned, active, departed, archived). */
  sections: RoomListSection[];
  /** Rooms shown across all sections, after search + filter. Zero with a
   *  non-empty `totalCount` is "no matches", not "no rooms". */
  visibleCount: number;
  /** Rooms supplied before any search/filter — so the surface tells an empty
   *  search result apart from a genuinely roomless account. */
  totalCount: number;
  /** Whether a non-blank search query was applied. */
  hasQuery: boolean;
}

export interface RoomListInput {
  rooms: RoomSummary[];
  query: string;
  filter: LifecycleFilter;
  /** Device-local pin set (docs/room-attention.md, decision 1: device-local
   *  state, never on the wire). Disjoint from `archived` — see roomFlags.ts. */
  pinned: ReadonlySet<string>;
  /** Device-local archive set. Disjoint from `pinned`. */
  archived: ReadonlySet<string>;
}

/** The room-id with any `namespace:` prefix stripped — the exact characters the
 *  short-id disambiguator shows (format.ts `shortId`). Search folds against this
 *  so "searching by short room ID" matches what the eye reads, not the wire
 *  namespace. */
function rawId(id: string): string {
  return id.includes(':') ? id.slice(id.indexOf(':') + 1) : id;
}

/** A room matches a query when the query (trimmed, case-folded) is a substring
 *  of its display name OR of its raw room-id. Blank query matches everything.
 *  Folded exactly like the homonym key so search and disambiguation agree on
 *  what a "name" is. */
export function roomMatchesQuery(room: RoomSummary, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (q === '') return true;
  if (roomDisplayName(room).toLowerCase().includes(q)) return true;
  return rawId(room.room_id).toLowerCase().includes(q);
}

/** A room's lifecycle from its signed membership status. A null/unknown status
 *  is treated as active (a joined room whose status has not synced is not
 *  "departed"), matching what the sidebar renders. */
export function roomLifecycle(room: RoomSummary): 'active' | 'departed' {
  return room.status === 'left' || room.status === 'removed' ? 'departed' : 'active';
}

/** Order within a section: newest signed activity first (the recency projection
 *  of docs/room-attention.md, decision 2), rooms with no recency last, then by
 *  display name, then by room-id for a total, stable order. Null recency sorts
 *  last so a real daemon (which omits `last_event_ts` today) falls back to a
 *  predictable alphabetical order rather than the daemon's arbitrary one. */
function compareRooms(a: RoomSummary, b: RoomSummary): number {
  const ta = a.last_event_ts ?? null;
  const tb = b.last_event_ts ?? null;
  if (ta !== tb) {
    if (ta === null) return 1;
    if (tb === null) return -1;
    return tb - ta;
  }
  const na = roomDisplayName(a).toLowerCase();
  const nb = roomDisplayName(b).toLowerCase();
  if (na !== nb) return na < nb ? -1 : 1;
  return a.room_id < b.room_id ? -1 : a.room_id > b.room_id ? 1 : 0;
}

/** Project the raw room list into the searched, filtered, ordered, sectioned
 *  view both clients render. Pure — no I/O, no clock, no localization — so it
 *  unit-tests against the shared fixtures with no environment. */
export function projectRoomList(input: RoomListInput): RoomListView {
  const { rooms, query, filter, pinned, archived } = input;
  const totalCount = rooms.length;
  const hasQuery = query.trim() !== '';

  const pinnedRooms: RoomSummary[] = [];
  const activeRooms: RoomSummary[] = [];
  const departedRooms: RoomSummary[] = [];
  const archivedRooms: RoomSummary[] = [];

  for (const room of rooms) {
    if (!roomMatchesQuery(room, query)) continue;
    // Archive is a search-only bucket, orthogonal to the lifecycle filter: a
    // room you deliberately put away stays found by name, and switching the
    // filter never surfaces it back into the main sections.
    if (archived.has(room.room_id)) {
      archivedRooms.push(room);
      continue;
    }
    const lifecycle = roomLifecycle(room);
    // The filter applies uniformly to everything non-archived — including
    // pinned, so "Active" never shows a departed room and "Left & removed"
    // never shows an active one, whatever its pin state.
    if (filter !== 'all' && lifecycle !== filter) continue;
    if (pinned.has(room.room_id)) pinnedRooms.push(room);
    else if (lifecycle === 'active') activeRooms.push(room);
    else departedRooms.push(room);
  }

  pinnedRooms.sort(compareRooms);
  activeRooms.sort(compareRooms);
  departedRooms.sort(compareRooms);
  archivedRooms.sort(compareRooms);

  // Homonyms over the full visible subset (every section, including a pinned
  // room and its unpinned twin), so the disambiguator appears wherever two
  // on-screen rooms collide and nowhere it would be noise.
  const visible = [...pinnedRooms, ...activeRooms, ...departedRooms, ...archivedRooms];
  const homonyms = homonymousRoomIds(visible);
  const toRow = (room: RoomSummary): RoomListRow => ({
    room,
    displayName: roomDisplayName(room),
    isHomonym: homonyms.has(room.room_id),
  });

  const sections: RoomListSection[] = [];
  if (pinnedRooms.length) sections.push({ key: 'pinned', rows: pinnedRooms.map(toRow) });
  if (activeRooms.length) sections.push({ key: 'active', rows: activeRooms.map(toRow) });
  if (departedRooms.length) sections.push({ key: 'departed', rows: departedRooms.map(toRow) });
  if (archivedRooms.length) sections.push({ key: 'archived', rows: archivedRooms.map(toRow) });

  return { sections, visibleCount: visible.length, totalCount, hasQuery };
}
