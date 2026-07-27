// Device-local unread: a per-room last-seen mark, stored only on this device
// (docs/room-attention.md, decision 3). It never leaves this machine and never
// implies another participant read or received anything — the protocol has no
// delivery or read receipt, and unread here is the honest absence of one.
// Mirrors the localStorage load/save discipline of names.ts.

const STORAGE_KEY = 'jeliya.lastSeen';

/** room_id → the newest signed-event ts (Unix ms) this device has acknowledged. */
export type LastSeen = Record<string, number>;

/** The minimum an unread verdict needs: a room id and its recency projection
 *  (docs/room-attention.md, decision 2). Satisfied by `RoomSummary`. */
export interface RecencyRoom {
  room_id: string;
  /** Null when the daemon supplies no recency (older daemon, or not synced). */
  last_event_ts?: number | null;
}

export function loadLastSeen(): LastSeen {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== 'object' || parsed === null) return {};
    const out: LastSeen = {};
    for (const [k, v] of Object.entries(parsed as Record<string, unknown>)) {
      // Drop non-finite / non-number marks, as names.ts drops non-string aliases.
      if (typeof v === 'number' && Number.isFinite(v)) out[k] = v;
    }
    return out;
  } catch {
    return {};
  }
}

export function saveLastSeen(lastSeen: LastSeen): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(lastSeen));
  } catch {
    // storage full/blocked — unread state just won't persist across restart.
  }
}

/** Establish the baseline the first time a room appears on this device, so a
 *  backlog that synced before you ever opened the room does not retroactively
 *  read as unread (docs/room-attention.md, decision 3). Writes only when no
 *  mark exists; returns a new map when it changed, else the same reference so
 *  callers can skip a redundant save. */
export function seedRoomSeen(lastSeen: LastSeen, roomId: string, ts: number): LastSeen {
  if (roomId in lastSeen) return lastSeen;
  return { ...lastSeen, [roomId]: ts };
}

/** Clear unread for one room by advancing its mark to the newest known ts
 *  (never backwards, so an out-of-order replay cannot re-raise a cleared dot).
 *  Affects only [roomId] (docs/room-attention.md, decision 3). */
export function markRoomSeen(lastSeen: LastSeen, roomId: string, ts: number): LastSeen {
  const current = lastSeen[roomId];
  if (current !== undefined && current >= ts) return lastSeen;
  return { ...lastSeen, [roomId]: ts };
}

/** Unread iff the room has a signed event newer than this device's last-seen
 *  mark (docs/room-attention.md, decision 3). Never a delivery/read receipt.
 *  No recency (null `last_event_ts`) and no baseline (unseeded) both read as
 *  NOT unread: an unread dot is a claim, and neither case holds the evidence
 *  for one — the app seeds a baseline for every listed room, and only genuine
 *  activity after that baseline flags. */
export function isRoomUnread(room: RecencyRoom, lastSeen: LastSeen): boolean {
  const ts = room.last_event_ts;
  if (ts == null) return false;
  const seen = lastSeen[room.room_id];
  if (seen === undefined) return false;
  return ts > seen;
}

/** The newest signed event observed live for a room, taken straight off a
 *  `room.event` push. Both values are the event's own — never a local clock. */
export interface LiveActivity {
  ts: number;
  kind: string;
}

/** room_id → the newest event seen live on this connection. */
export type LiveActivityMap = Record<string, LiveActivity>;

/** Fold live push activity into `room.list` rows.
 *
 *  Every open room pushes its own events, so activity arrives for rooms the
 *  user is not currently viewing. `room.list` is only re-fetched on user
 *  action, so without this the rail would stay silent about those rooms until
 *  the next refresh. Recency only ever moves FORWARD: a stale or equal live
 *  value never overwrites a newer daemon projection, and a room with no live
 *  activity is returned untouched (identity-preserved, so React can skip it).
 *
 *  This is a render-time projection. It must NOT feed `seedRoomSeen` — seeding
 *  a baseline from a live event would mark the room seen the moment it became
 *  active, and the unread dot could never appear. */
export function mergeLiveActivity<T extends RecencyRoom>(
  rooms: readonly T[],
  live: LiveActivityMap,
): T[] {
  return rooms.map((room) => {
    const seen = live[room.room_id];
    if (!seen) return room;
    const known = room.last_event_ts;
    if (known != null && known >= seen.ts) return room;
    return { ...room, last_event_ts: seen.ts, last_event_kind: seen.kind };
  });
}
