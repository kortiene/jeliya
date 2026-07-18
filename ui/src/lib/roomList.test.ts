import { describe, expect, it } from 'vitest';
import { projectRoomList, roomLifecycle, roomMatchesQuery, type LifecycleFilter } from './roomList';
import type { RoomSummary } from './protocol';
import type { RoomListInput } from './roomList';
import fixtures from './conformance/room-list.fixtures.json';

const UNTITLED_ROOM = 'Untitled room';
const project = (input: Omit<RoomListInput, 'untitledLabel'>) =>
  projectRoomList({ ...input, untitledLabel: UNTITLED_ROOM });

const room = (over: Partial<RoomSummary> & { room_id: string }): RoomSummary => ({
  name: null,
  role: null,
  status: 'active',
  member_count: 0,
  open: false,
  last_event_ts: null,
  ...over,
});

describe('roomMatchesQuery', () => {
  it('matches everything on a blank or whitespace-only query', () => {
    const r = room({ room_id: 'blake3:aa', name: 'Design' });
    expect(roomMatchesQuery(r, '', UNTITLED_ROOM)).toBe(true);
    expect(roomMatchesQuery(r, '   ', UNTITLED_ROOM)).toBe(true);
  });

  it('matches a case-folded, trimmed substring of the display name', () => {
    const r = room({ room_id: 'blake3:aa', name: 'Design System' });
    expect(roomMatchesQuery(r, '  SYSTEM ', UNTITLED_ROOM)).toBe(true);
    expect(roomMatchesQuery(r, 'ops', UNTITLED_ROOM)).toBe(false);
  });

  it('matches the untitled placeholder for a null-named room', () => {
    const r = room({ room_id: 'blake3:aa', name: null });
    expect(roomMatchesQuery(r, 'untitled', UNTITLED_ROOM)).toBe(true);
  });

  it('matches the raw room id (namespace stripped), not the wire namespace', () => {
    const r = room({ room_id: 'blake3:beef0000cafe', name: 'Sandbox' });
    expect(roomMatchesQuery(r, 'cafe', UNTITLED_ROOM)).toBe(true);
    expect(roomMatchesQuery(r, 'beef', UNTITLED_ROOM)).toBe(true);
    // the "blake3" namespace is not part of the id the user reads
    expect(roomMatchesQuery(r, 'blake3', UNTITLED_ROOM)).toBe(false);
  });
});

describe('roomLifecycle', () => {
  it('classifies left/removed as departed and everything else as active', () => {
    expect(roomLifecycle(room({ room_id: 'r', status: 'left' }))).toBe('departed');
    expect(roomLifecycle(room({ room_id: 'r', status: 'removed' }))).toBe('departed');
    expect(roomLifecycle(room({ room_id: 'r', status: 'active' }))).toBe('active');
    // a joined room whose status has not synced is not "departed"
    expect(roomLifecycle(room({ room_id: 'r', status: null }))).toBe('active');
  });
});

describe('projectRoomList', () => {
  it('orders a section by recency desc, null recency last, then name, then id', () => {
    const view = project({
      rooms: [
        room({ room_id: 'blake3:z', name: 'Zeta', last_event_ts: null }),
        room({ room_id: 'blake3:a', name: 'Alpha', last_event_ts: null }),
        room({ room_id: 'blake3:m', name: 'Mid', last_event_ts: 500 }),
      ],
      query: '',
      filter: 'all',
      pinned: new Set(),
      archived: new Set(),
    });
    expect(view.sections[0].rows.map((r) => r.room.name)).toEqual(['Mid', 'Alpha', 'Zeta']);
  });

  it('breaks a full tie (equal recency and name) by room id for a stable order', () => {
    const view = project({
      rooms: [
        room({ room_id: 'blake3:b', name: 'Same', last_event_ts: 10 }),
        room({ room_id: 'blake3:a', name: 'Same', last_event_ts: 10 }),
      ],
      query: '',
      filter: 'all',
      pinned: new Set(),
      archived: new Set(),
    });
    expect(view.sections[0].rows.map((r) => r.room.room_id)).toEqual(['blake3:a', 'blake3:b']);
  });

  it('emits no empty sections', () => {
    const view = project({
      rooms: [room({ room_id: 'blake3:a', name: 'Only', status: 'active' })],
      query: '',
      filter: 'all',
      pinned: new Set(),
      archived: new Set(),
    });
    expect(view.sections.map((s) => s.key)).toEqual(['active']);
  });

  it('distinguishes an empty search result from a genuinely roomless account', () => {
    const empty = project({ rooms: [], query: '', filter: 'all', pinned: new Set(), archived: new Set() });
    expect(empty.totalCount).toBe(0);
    expect(empty.visibleCount).toBe(0);

    const noMatch = project({
      rooms: [room({ room_id: 'blake3:a', name: 'Design' })],
      query: 'zzz',
      filter: 'all',
      pinned: new Set(),
      archived: new Set(),
    });
    expect(noMatch.totalCount).toBe(1);
    expect(noMatch.visibleCount).toBe(0);
    expect(noMatch.hasQuery).toBe(true);
  });
});

// The shared corpus, replayed here and (identically) in app/test/room_list_test.dart
// so React and Flutter section, order, and disambiguate one room list from ONE
// source (issue #64).
describe('shared room-list fixtures (parity with Flutter)', () => {
  interface FixtureRoom {
    room_id: string;
    name: string | null;
    status: string | null;
    last_event_ts: number | null;
  }
  interface FixtureCase {
    name: string;
    rooms: FixtureRoom[];
    query: string;
    filter: LifecycleFilter;
    pinned: string[];
    archived: string[];
    expect: {
      sections: { key: string; rooms: string[] }[];
      homonyms: string[];
      visibleCount: number;
      totalCount: number;
      hasQuery: boolean;
    };
  }
  const cases = fixtures.cases as unknown as FixtureCase[];

  const toRoom = (r: FixtureRoom): RoomSummary => room({
    room_id: r.room_id,
    name: r.name,
    status: r.status,
    last_event_ts: r.last_event_ts,
  });

  it('covers every distinct case name once', () => {
    expect(new Set(cases.map((c) => c.name)).size).toBe(cases.length);
  });

  for (const c of cases) {
    it(`case "${c.name}"`, () => {
      const view = project({
        rooms: c.rooms.map(toRoom),
        query: c.query,
        filter: c.filter,
        pinned: new Set(c.pinned),
        archived: new Set(c.archived),
      });
      const sections = view.sections.map((s) => ({ key: s.key, rooms: s.rows.map((r) => r.room.room_id) }));
      expect(sections).toEqual(c.expect.sections);
      const homonyms = view.sections
        .flatMap((s) => s.rows)
        .filter((r) => r.isHomonym)
        .map((r) => r.room.room_id)
        .sort();
      expect(homonyms).toEqual([...c.expect.homonyms].sort());
      expect(view.visibleCount).toBe(c.expect.visibleCount);
      expect(view.totalCount).toBe(c.expect.totalCount);
      expect(view.hasQuery).toBe(c.expect.hasQuery);
    });
  }
});
