// @vitest-environment jsdom
//
// jsdom gives this file a real `localStorage` for the load/save round-trip; the
// pure seed/mark/unread helpers need no DOM. The Dart mirror of the shared-
// fixture block lives in dart/jeliya_protocol/test/conventions_test.dart — both
// read ./conformance/room-attention.fixtures.json, so the two clients decide
// unread from ONE source (docs/room-attention.md; issue #63, AC7).

import { beforeEach, describe, expect, it } from 'vitest';
import {
  isRoomUnread,
  loadLastSeen,
  markRoomSeen,
  mergeLiveActivity,
  saveLastSeen,
  seedRoomSeen,
  shouldSeedFromLiveEvent,
  type LastSeen,
  type LiveActivityMap,
} from './lastSeen';
import fixtures from './conformance/room-attention.fixtures.json';

const R1 = 'blake3:1111111111111111111111111111111111111111111111111111111111111111';
const R2 = 'blake3:2222222222222222222222222222222222222222222222222222222222222222';

describe('loadLastSeen / saveLastSeen', () => {
  beforeEach(() => localStorage.clear());

  it('round-trips a mark map through localStorage', () => {
    saveLastSeen({ [R1]: 1700, [R2]: 42 });
    expect(loadLastSeen()).toEqual({ [R1]: 1700, [R2]: 42 });
  });

  it('returns an empty map when nothing is stored', () => {
    expect(loadLastSeen()).toEqual({});
  });

  it('drops non-number marks, like names.ts drops non-string aliases', () => {
    localStorage.setItem(
      'jeliya.lastSeen',
      JSON.stringify({ [R1]: 1700, aString: 'nope', aNull: null }),
    );
    expect(loadLastSeen()).toEqual({ [R1]: 1700 });
  });

  it('survives malformed JSON without throwing', () => {
    localStorage.setItem('jeliya.lastSeen', '{ not json');
    expect(loadLastSeen()).toEqual({});
  });
});

describe('seedRoomSeen', () => {
  it('writes a baseline only when the room has no mark yet', () => {
    const empty: LastSeen = {};
    const seeded = seedRoomSeen(empty, R1, 100);
    expect(seeded).toEqual({ [R1]: 100 });
    expect(seeded).not.toBe(empty);
  });

  it('never overwrites an existing mark (a seeded room keeps its acknowledged ts)', () => {
    const map: LastSeen = { [R1]: 500 };
    const after = seedRoomSeen(map, R1, 100);
    expect(after).toBe(map); // same reference — no redundant save
    expect(after[R1]).toBe(500);
  });
});

describe('markRoomSeen', () => {
  it('advances the mark to a newer ts (clears unread)', () => {
    expect(markRoomSeen({ [R1]: 100 }, R1, 300)).toEqual({ [R1]: 300 });
  });

  it('never moves the mark backwards, so out-of-order replay cannot re-raise a cleared dot', () => {
    const map: LastSeen = { [R1]: 300 };
    expect(markRoomSeen(map, R1, 100)).toBe(map);
  });

  it('affects only the named room', () => {
    expect(markRoomSeen({ [R1]: 100, [R2]: 100 }, R1, 300)).toEqual({ [R1]: 300, [R2]: 100 });
  });
});

describe('isRoomUnread', () => {
  it('is unread when the newest event is past the last-seen mark', () => {
    expect(isRoomUnread({ room_id: R1, last_event_ts: 300 }, { [R1]: 100 })).toBe(true);
  });

  it('is not unread when the mark is at or past the newest event', () => {
    expect(isRoomUnread({ room_id: R1, last_event_ts: 100 }, { [R1]: 100 })).toBe(false);
    expect(isRoomUnread({ room_id: R1, last_event_ts: 100 }, { [R1]: 300 })).toBe(false);
  });

  it('is not unread with no recency evidence (null last_event_ts) — a dot is a claim', () => {
    expect(isRoomUnread({ room_id: R1, last_event_ts: null }, { [R1]: 100 })).toBe(false);
    expect(isRoomUnread({ room_id: R1 }, { [R1]: 100 })).toBe(false);
  });

  it('is not unread with no baseline (unseeded room) — no evidence for a dot', () => {
    expect(isRoomUnread({ room_id: R1, last_event_ts: 300 }, {})).toBe(false);
  });
});

// The shared five-case fixture, replayed here and (identically) in the Dart
// conventions test — the parity guard docs/room-attention.md relies on.
describe('shared room-attention fixtures (parity with Dart)', () => {
  interface FixtureCase {
    name: string;
    room: { room_id: string; last_event_ts: number | null; last_event_kind: string | null };
    last_seen: number | null;
    connected: boolean;
    expect: { unread: boolean };
  }
  const cases = fixtures.cases as FixtureCase[];

  it('covers the five truthful states exactly once', () => {
    expect(cases.map((c) => c.name).sort()).toEqual(
      ['attention', 'no-data', 'offline', 'stale', 'unread'],
    );
  });

  for (const c of cases) {
    it(`case "${c.name}" → unread ${c.expect.unread}`, () => {
      const lastSeen: LastSeen = c.last_seen == null ? {} : { [c.room.room_id]: c.last_seen };
      expect(isRoomUnread(c.room, lastSeen)).toBe(c.expect.unread);
    });
  }
});

describe('mergeLiveActivity', () => {
  const row = (room_id: string, last_event_ts: number | null) => ({
    room_id,
    name: 'r',
    role: 'member' as const,
    status: 'active',
    member_count: 2,
    open: true,
    last_event_ts,
  });

  it('advances a room the user is NOT viewing, so its activity is not lost', () => {
    // The whole point: every open room pushes its own events, and room.list is
    // only re-fetched on user action.
    const live: LiveActivityMap = { [R2]: { ts: 500, kind: 'message' } };
    const merged = mergeLiveActivity([row(R1, 100), row(R2, 200)], live);
    expect(merged[0].last_event_ts).toBe(100);
    expect(merged[1].last_event_ts).toBe(500);
    expect(merged[1].last_event_kind).toBe('message');
  });

  it('never moves recency backwards past a newer daemon projection', () => {
    const live: LiveActivityMap = { [R1]: { ts: 100, kind: 'message' } };
    const merged = mergeLiveActivity([row(R1, 900)], live);
    expect(merged[0].last_event_ts).toBe(900);
    expect(merged[0].last_event_kind).toBeUndefined();
  });

  it('fills recency for a room the daemon reports no recency for', () => {
    const live: LiveActivityMap = { [R1]: { ts: 42, kind: 'agent_status' } };
    const merged = mergeLiveActivity([row(R1, null)], live);
    expect(merged[0].last_event_ts).toBe(42);
    expect(merged[0].last_event_kind).toBe('agent_status');
  });

  it('returns untouched rows by identity, so React can skip re-rendering them', () => {
    const rooms = [row(R1, 100)];
    expect(mergeLiveActivity(rooms, {})[0]).toBe(rooms[0]);
  });

  it('feeds an unread dot for a room with activity after its baseline', () => {
    // The end-to-end verdict: merge, then ask the same predicate the rail uses.
    const rooms = mergeLiveActivity([row(R1, 100)], { [R1]: { ts: 700, kind: 'message' } });
    expect(isRoomUnread(rooms[0], { [R1]: 100 })).toBe(true);
    // …and no dot once the mark has caught up.
    expect(isRoomUnread(rooms[0], { [R1]: 700 })).toBe(false);
  });
});

describe('shouldSeedFromLiveEvent (issue #154)', () => {
  const row = (last_event_ts: number | null) => ({ room_id: R1, last_event_ts });

  it('seeds when the listed row carries no recency — a daemon predating the projection', () => {
    // The daemon lists no room without events, so a null here is not "empty
    // room"; it is "this daemon does not compute recency".
    expect(shouldSeedFromLiveEvent(row(null))).toBe(true);
    expect(shouldSeedFromLiveEvent({ room_id: R1 })).toBe(true);
  });

  it('does NOT seed when the row has recency, so it cannot mask activity', () => {
    expect(shouldSeedFromLiveEvent(row(100))).toBe(false);
    expect(shouldSeedFromLiveEvent(row(0))).toBe(false);
  });

  it('does NOT seed for a room that is not listed yet', () => {
    expect(shouldSeedFromLiveEvent(undefined)).toBe(false);
  });

  it('absorbs the first event as a baseline, then flags every later one', () => {
    // The whole point of the rule: without a baseline this daemon could never
    // raise a dot at all; with one, only the first event is absorbed.
    let seen: LastSeen = {};
    const first = { ts: 500, kind: 'message' };
    if (shouldSeedFromLiveEvent(row(null))) seen = seedRoomSeen(seen, R1, first.ts);
    expect(isRoomUnread(mergeLiveActivity([row(null)], { [R1]: first })[0], seen)).toBe(false);

    const second = { ts: 900, kind: 'message' };
    if (shouldSeedFromLiveEvent(row(null))) seen = seedRoomSeen(seen, R1, second.ts);
    expect(isRoomUnread(mergeLiveActivity([row(null)], { [R1]: second })[0], seen)).toBe(true);
  });

  it('a current daemon keeps seeding from room.list, unaffected by live events', () => {
    const seen = seedRoomSeen({}, R1, 100);
    expect(shouldSeedFromLiveEvent(row(100))).toBe(false);
    const merged = mergeLiveActivity([row(100)], { [R1]: { ts: 700, kind: 'message' } });
    expect(isRoomUnread(merged[0], seen)).toBe(true);
  });
});

describe('baseline resolution order (issue #154 review)', () => {
  // The rule is resolved where BOTH the room.list rows and the live map are
  // visible, so an event that arrives before the first room.list still becomes
  // the baseline instead of being lost.
  const seedAll = (rooms: Array<{ room_id: string; last_event_ts: number | null }>,
                   live: LiveActivityMap, prev: LastSeen): LastSeen => {
    let next = prev;
    for (const room of rooms) {
      const baseline = shouldSeedFromLiveEvent(room) ? live[room.room_id]?.ts : room.last_event_ts;
      if (baseline != null) next = seedRoomSeen(next, room.room_id, baseline);
    }
    return next;
  };
  const row = (last_event_ts: number | null) => ({ room_id: R1, last_event_ts });

  it('an event observed BEFORE the first room.list still becomes the baseline', () => {
    // push arrives first — no rooms yet, so nothing can be seeded
    const live: LiveActivityMap = { [R1]: { ts: 500, kind: 'message' } };
    expect(seedAll([], live, {})).toEqual({});
    // …then the row lands with no recency, and the earlier event seeds it
    const seen = seedAll([row(null)], live, {});
    expect(seen).toEqual({ [R1]: 500 });
    expect(isRoomUnread(mergeLiveActivity([row(null)], live)[0], seen)).toBe(false);

    // the NEXT event flags, rather than being absorbed as a second baseline
    const later: LiveActivityMap = { [R1]: { ts: 900, kind: 'message' } };
    const after = seedAll([row(null)], later, seen);
    expect(after).toEqual({ [R1]: 500 });
    expect(isRoomUnread(mergeLiveActivity([row(null)], later)[0], after)).toBe(true);
  });

  it('a row WITH recency always seeds from room.list, never from live activity', () => {
    const live: LiveActivityMap = { [R1]: { ts: 900, kind: 'message' } };
    expect(seedAll([row(100)], live, {})).toEqual({ [R1]: 100 });
  });
});
