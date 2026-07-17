// @vitest-environment jsdom
//
// jsdom gives this file a real `localStorage` for the load/save round-trip; the
// pure toggle helpers need no DOM. Mirrors the discipline of lastSeen.test.ts.

import { beforeEach, describe, expect, it } from 'vitest';
import {
  emptyRoomFlags,
  loadRoomFlags,
  saveRoomFlags,
  togglePinned,
  toggleArchived,
  type RoomFlags,
} from './roomFlags';

const A = 'blake3:aaaa';
const B = 'blake3:bbbb';

describe('loadRoomFlags / saveRoomFlags', () => {
  beforeEach(() => localStorage.clear());

  it('round-trips pin/archive sets through localStorage', () => {
    saveRoomFlags({ pinned: new Set([A]), archived: new Set([B]) });
    const loaded = loadRoomFlags();
    expect([...loaded.pinned]).toEqual([A]);
    expect([...loaded.archived]).toEqual([B]);
  });

  it('returns empty sets when nothing is stored', () => {
    const loaded = loadRoomFlags();
    expect(loaded.pinned.size).toBe(0);
    expect(loaded.archived.size).toBe(0);
  });

  it('drops non-string / empty ids, like lastSeen drops non-numbers', () => {
    localStorage.setItem('jeliya.rooms.v1', JSON.stringify({ pinned: [A, 42, '', null], archived: 'nope' }));
    const loaded = loadRoomFlags();
    expect([...loaded.pinned]).toEqual([A]);
    expect(loaded.archived.size).toBe(0);
  });

  it('survives malformed JSON without throwing', () => {
    localStorage.setItem('jeliya.rooms.v1', '{ not json');
    expect(loadRoomFlags()).toEqual(emptyRoomFlags());
  });

  it('enforces the disjoint invariant on a tampered store (pin wins)', () => {
    localStorage.setItem('jeliya.rooms.v1', JSON.stringify({ pinned: [A], archived: [A, B] }));
    const loaded = loadRoomFlags();
    expect([...loaded.pinned]).toEqual([A]);
    expect([...loaded.archived]).toEqual([B]);
  });
});

describe('togglePinned', () => {
  it('adds a pin and clears any archive mark (the two are exclusive)', () => {
    const flags: RoomFlags = { pinned: new Set(), archived: new Set([A]) };
    const after = togglePinned(flags, A);
    expect(after.pinned.has(A)).toBe(true);
    expect(after.archived.has(A)).toBe(false);
  });

  it('removes an existing pin', () => {
    const after = togglePinned({ pinned: new Set([A]), archived: new Set() }, A);
    expect(after.pinned.has(A)).toBe(false);
  });

  it('never mutates its input', () => {
    const flags: RoomFlags = { pinned: new Set(), archived: new Set() };
    togglePinned(flags, A);
    expect(flags.pinned.size).toBe(0);
  });
});

describe('toggleArchived', () => {
  it('adds an archive and clears any pin mark (the two are exclusive)', () => {
    const after = toggleArchived({ pinned: new Set([A]), archived: new Set() }, A);
    expect(after.archived.has(A)).toBe(true);
    expect(after.pinned.has(A)).toBe(false);
  });

  it('removes an existing archive', () => {
    const after = toggleArchived({ pinned: new Set(), archived: new Set([A]) }, A);
    expect(after.archived.has(A)).toBe(false);
  });
});
