// Device-local pin / archive marks for the room list (issue #64). Stored only on
// this device, never on the wire (docs/room-attention.md, decision 1:
// device-local state). Pin and archive are display preferences — where a room
// sits in *your* list — and never a claim about the room or another participant,
// so they are honest to diverge across your devices, exactly like the last-seen
// mark (lastSeen.ts) and aliases (names.ts) they sit beside.
//
// Pin and archive are mutually exclusive: pinning "keep this in front" and
// archiving "put this away" are opposites, so the toggles enforce a disjoint
// {pinned} and {archived}. That invariant is what lets the projection
// (roomList.ts) treat archived rooms as never pinned without a tiebreak.

const STORAGE_KEY = 'jeliya.rooms.v1';

export interface RoomFlags {
  pinned: Set<string>;
  archived: Set<string>;
}

export function emptyRoomFlags(): RoomFlags {
  return { pinned: new Set(), archived: new Set() };
}

/** Coerce a stored value into a set of non-empty string ids, dropping anything
 *  malformed (as lastSeen.ts drops non-number marks). */
function toIdSet(value: unknown): Set<string> {
  const out = new Set<string>();
  if (!Array.isArray(value)) return out;
  for (const item of value) if (typeof item === 'string' && item) out.add(item);
  return out;
}

export function loadRoomFlags(): RoomFlags {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return emptyRoomFlags();
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== 'object' || parsed === null) return emptyRoomFlags();
    const obj = parsed as Record<string, unknown>;
    const pinned = toIdSet(obj.pinned);
    const archived = toIdSet(obj.archived);
    // Enforce the disjoint invariant even on a hand-tampered store, so the
    // projection never sees a room that is both pinned and archived.
    for (const id of pinned) archived.delete(id);
    return { pinned, archived };
  } catch {
    return emptyRoomFlags();
  }
}

export function saveRoomFlags(flags: RoomFlags): void {
  try {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({ pinned: [...flags.pinned], archived: [...flags.archived] }),
    );
  } catch {
    // storage full/blocked — pin/archive just won't persist across restart.
  }
}

/** Toggle a room's pin. Pinning clears any archive mark (the two are exclusive);
 *  unpinning leaves the room unarchived. Returns a new RoomFlags — never mutates
 *  its input — so callers can compare references and setState safely. */
export function togglePinned(flags: RoomFlags, roomId: string): RoomFlags {
  const pinned = new Set(flags.pinned);
  const archived = new Set(flags.archived);
  if (pinned.has(roomId)) {
    pinned.delete(roomId);
  } else {
    pinned.add(roomId);
    archived.delete(roomId);
  }
  return { pinned, archived };
}

/** Toggle a room's archive. Archiving clears any pin mark (the two are
 *  exclusive); unarchiving leaves the room unpinned. Returns a new RoomFlags. */
export function toggleArchived(flags: RoomFlags, roomId: string): RoomFlags {
  const pinned = new Set(flags.pinned);
  const archived = new Set(flags.archived);
  if (archived.has(roomId)) {
    archived.delete(roomId);
  } else {
    archived.add(roomId);
    pinned.delete(roomId);
  }
  return { pinned, archived };
}
