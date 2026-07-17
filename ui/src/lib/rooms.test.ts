import { describe, expect, it } from 'vitest';
import { homonymousRoomIds, roomDisplayName, UNTITLED_ROOM } from './rooms';

const room = (room_id: string, name: string | null) => ({ room_id, name });

describe('roomDisplayName', () => {
  it('shows the label when present', () => {
    expect(roomDisplayName(room('r1', 'Design System'))).toBe('Design System');
  });

  it('falls back to the untitled placeholder for a null (unsynced) name', () => {
    expect(roomDisplayName(room('r1', null))).toBe(UNTITLED_ROOM);
  });
});

describe('homonymousRoomIds', () => {
  it('returns no ids when every display name is unique', () => {
    const set = homonymousRoomIds([
      room('a', 'Design System'),
      room('b', 'Product Review'),
      room('c', 'Research Lab'),
    ]);
    expect(set.size).toBe(0);
  });

  it('flags every member of a homonym group, and only those', () => {
    const set = homonymousRoomIds([
      room('a', 'Bug Triage'),
      room('b', 'Bug Triage'),
      room('c', 'Design System'),
    ]);
    expect([...set].sort()).toEqual(['a', 'b']);
    expect(set.has('c')).toBe(false);
  });

  it('treats two unsynced (null-named) rooms as homonyms of each other', () => {
    // The most likely case for a new user: neither genesis event has synced, so
    // both render the untitled placeholder and must both disambiguate.
    const set = homonymousRoomIds([room('a', null), room('b', null)]);
    expect([...set].sort()).toEqual(['a', 'b']);
  });

  it('a single untitled room is not a homonym of anything', () => {
    const set = homonymousRoomIds([room('a', null), room('b', 'Named Room')]);
    expect(set.size).toBe(0);
  });

  it('folds case and surrounding whitespace before comparing', () => {
    // The same display name the UI shows, trimmed and case-folded — a user
    // cannot act on the difference between "Design" and " design ".
    const set = homonymousRoomIds([
      room('a', 'Design'),
      room('b', '  design  '),
      room('c', 'DESIGN'),
    ]);
    expect([...set].sort()).toEqual(['a', 'b', 'c']);
  });

  it('an unsynced room is a homonym of a room literally named "Untitled room"', () => {
    // The placeholder is compared like any other display name, so a null name
    // and an explicit "untitled room" label collide.
    const set = homonymousRoomIds([room('a', null), room('b', 'untitled room')]);
    expect([...set].sort()).toEqual(['a', 'b']);
  });
});
