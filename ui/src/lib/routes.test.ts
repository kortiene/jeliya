import { describe, expect, it } from 'vitest';
import {
  inspectorDest,
  legacyTabDest,
  parseRoute,
  routePath,
  routeRoomId,
  searchWithoutLegacyTab,
} from './routes';
import type { Route } from './routes';

const ROOM = 'blake3:1111111111111111111111111111111111111111111111111111111111111111';

describe('parseRoute', () => {
  it('resolves the bare root and unknown paths to Rooms', () => {
    // Total by construction: an unknown URL is a navigation miss, and Rooms is
    // the recovery — never a blank panel or a thrown error.
    expect(parseRoute('/')).toEqual({ kind: 'rooms' });
    expect(parseRoute('')).toEqual({ kind: 'rooms' });
    expect(parseRoute('/rooms')).toEqual({ kind: 'rooms' });
    expect(parseRoute('/nope')).toEqual({ kind: 'rooms' });
    expect(parseRoute('/rooms/')).toEqual({ kind: 'rooms' });
  });

  it('parses the global destinations', () => {
    expect(parseRoute('/fleet')).toEqual({ kind: 'fleet' });
    expect(parseRoute('/settings')).toEqual({ kind: 'settings' });
  });

  it('parses every room destination', () => {
    for (const dest of ['activity', 'people', 'agents', 'files', 'pipes'] as const) {
      expect(parseRoute(`/rooms/${encodeURIComponent(ROOM)}/${dest}`)).toEqual({
        kind: 'room',
        roomId: ROOM,
        dest,
      });
    }
  });

  it('normalizes a bare room and an unknown destination to Activity', () => {
    expect(parseRoute(`/rooms/${encodeURIComponent(ROOM)}`)).toEqual({
      kind: 'room',
      roomId: ROOM,
      dest: 'activity',
    });
    expect(parseRoute(`/rooms/${encodeURIComponent(ROOM)}/bogus`)).toEqual({
      kind: 'room',
      roomId: ROOM,
      dest: 'activity',
    });
  });

  it('survives a malformed percent-escape', () => {
    // decodeURIComponent throws on '%zz'; a bad URL is a miss, not a crash.
    expect(parseRoute('/rooms/%zz/activity')).toEqual({ kind: 'rooms' });
  });

  it('round-trips every route through routePath', () => {
    const routes: Route[] = [
      { kind: 'rooms' },
      { kind: 'fleet' },
      { kind: 'settings' },
      { kind: 'room', roomId: ROOM, dest: 'activity' },
      { kind: 'room', roomId: ROOM, dest: 'pipes' },
    ];
    for (const route of routes) {
      expect(parseRoute(routePath(route))).toEqual(route);
    }
  });

  it('round-trips a room id that needs escaping', () => {
    const awkward = 'blake3:a/b c';
    const path = routePath({ kind: 'room', roomId: awkward, dest: 'files' });
    expect(path).not.toContain('a/b');
    expect(parseRoute(path)).toEqual({ kind: 'room', roomId: awkward, dest: 'files' });
  });
});

describe('routeRoomId', () => {
  it('is the selected room, or null for a global destination', () => {
    expect(routeRoomId({ kind: 'room', roomId: ROOM, dest: 'activity' })).toBe(ROOM);
    expect(routeRoomId({ kind: 'rooms' })).toBeNull();
    expect(routeRoomId({ kind: 'fleet' })).toBeNull();
    expect(routeRoomId({ kind: 'settings' })).toBeNull();
  });
});

describe('inspectorDest', () => {
  it('is closed on Activity and open on every tool', () => {
    expect(inspectorDest({ kind: 'room', roomId: ROOM, dest: 'activity' })).toBeNull();
    expect(inspectorDest({ kind: 'room', roomId: ROOM, dest: 'people' })).toBe('people');
    expect(inspectorDest({ kind: 'room', roomId: ROOM, dest: 'files' })).toBe('files');
  });

  it('is closed for every global destination', () => {
    expect(inspectorDest({ kind: 'rooms' })).toBeNull();
    expect(inspectorDest({ kind: 'fleet' })).toBeNull();
    expect(inspectorDest({ kind: 'settings' })).toBeNull();
  });
});

describe('legacyTabDest', () => {
  it('migrates ?tab= onto room destinations', () => {
    expect(legacyTabDest('?tab=members')).toBe('people');
    expect(legacyTabDest('?tab=agents')).toBe('agents');
    expect(legacyTabDest('?tab=files')).toBe('files');
    expect(legacyTabDest('?tab=pipes')).toBe('pipes');
  });

  it('ignores a missing or unknown tab', () => {
    expect(legacyTabDest('')).toBeNull();
    expect(legacyTabDest('?daemon=7420')).toBeNull();
    expect(legacyTabDest('?tab=nonsense')).toBeNull();
  });
});

describe('searchWithoutLegacyTab', () => {
  it('drops only the migrated key', () => {
    // ?daemon= picks the daemon and ?mock… installs the e2e fixtures: losing
    // them would re-point the client and silently unfixture the suite.
    expect(searchWithoutLegacyTab('?tab=files&daemon=7420')).toBe('?daemon=7420');
    expect(searchWithoutLegacyTab('?mock=1&tab=pipes&mock_fail=room.open:1')).toBe(
      '?mock=1&mock_fail=room.open%3A1',
    );
  });

  it('leaves a query with no legacy tab byte-identical', () => {
    // Not merely equivalent: re-encoding '?mock_fail=room.open:1' would escape
    // the colon and change the string for no reason.
    expect(searchWithoutLegacyTab('?mock_fail=room.open:1')).toBe('?mock_fail=room.open:1');
    expect(searchWithoutLegacyTab('')).toBe('');
  });

  it('empties a query that was only a legacy tab', () => {
    expect(searchWithoutLegacyTab('?tab=members')).toBe('');
  });
});
