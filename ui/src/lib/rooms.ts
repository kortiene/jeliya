// Room identity vs. label (docs/room-workbench.md, decision 6). `room_id` is
// identity; `name` is a non-unique, daemon-local label that is null until the
// genesis event syncs. Homonym detection and the untitled placeholder both live
// here so every surface shows — and disambiguates by short id — the SAME
// display name the user reads.

/** What a room with no synced name renders as. A null name means the genesis
 *  event has not arrived yet, so two such rooms render the same string and are
 *  homonyms of each other — the most likely case for a brand-new user. Must
 *  match the placeholder App.tsx renders for a null name. */
export const UNTITLED_ROOM = 'Untitled room';

/** The minimum a room needs to be named and compared — satisfied by both
 *  `RoomSummary` and a fleet room reference (`FleetAgentRoom`). */
export interface NamedRoom {
  room_id: string;
  name: string | null;
}

/** The name the UI shows for a room: its label, or the untitled placeholder
 *  when the name has not synced. Every surface must display this so homonym
 *  detection stays based on the exact string the user reads. */
export function roomDisplayName(room: NamedRoom): string {
  return room.name ?? UNTITLED_ROOM;
}

/** The key two rooms collide on: the displayed name, trimmed and case-folded.
 *  Case and surrounding whitespace are not a distinction a human scanning a
 *  list can act on, so " Design " and "design" fold together. */
function homonymKey(room: NamedRoom): string {
  return roomDisplayName(room).trim().toLowerCase();
}

/** The set of `room_id`s that share a display name (trimmed, case-folded) with
 *  at least one OTHER room in the list. A room whose display name is unique is
 *  NOT in the set: the short-id disambiguator is noise when the name already
 *  identifies the room. The untitled placeholder participates like any other
 *  name, so two unsynced rooms are homonyms of each other. Pure — every surface
 *  disambiguates off this one rule. */
export function homonymousRoomIds(rooms: NamedRoom[]): Set<string> {
  const idsByKey = new Map<string, string[]>();
  for (const room of rooms) {
    const key = homonymKey(room);
    const ids = idsByKey.get(key);
    if (ids) ids.push(room.room_id);
    else idsByKey.set(key, [room.room_id]);
  }
  const homonyms = new Set<string>();
  for (const ids of idsByKey.values()) {
    if (ids.length > 1) for (const id of ids) homonyms.add(id);
  }
  return homonyms;
}
