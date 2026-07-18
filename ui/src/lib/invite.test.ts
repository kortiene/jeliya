import { describe, expect, it } from 'vitest';
import {
  buildCombinedInvite,
  EXPIRY_PRESETS,
  inviteState,
  isIdentityId,
  splitInvite,
  type InviteState,
} from './invite';
import fixtures from './conformance/invite-lifecycle.fixtures.json';

describe('splitInvite', () => {
  it('splits a combined invite and prefers an explicit address', () => {
    expect(splitInvite('roomtkt1abc#ep@h:1', '')).toEqual({ ticket: 'roomtkt1abc', peerAddr: 'ep@h:1' });
    expect(splitInvite('roomtkt1abc#ep@h:1', 'other@h:2')).toEqual({ ticket: 'roomtkt1abc', peerAddr: 'other@h:2' });
  });
});

describe('EXPIRY_PRESETS', () => {
  it('offers 1h/24h/7d/never with the right seconds', () => {
    expect(EXPIRY_PRESETS.map((p) => p.key)).toEqual(['1h', '24h', '7d', 'never']);
    expect(EXPIRY_PRESETS.map((p) => p.seconds)).toEqual([3600, 86400, 604800, null]);
  });
});

// The shared corpus, replayed here and (identically) in
// app/test/invite_lifecycle_test.dart so React and Flutter validate, build, and
// derive the invite lifecycle from ONE source (issue #66).
describe('shared invite-lifecycle fixtures (parity with Flutter)', () => {
  for (const c of fixtures.identity as Array<{ value: string; valid: boolean }>) {
    it(`identity ${JSON.stringify(c.value)} → ${c.valid}`, () => {
      expect(isIdentityId(c.value)).toBe(c.valid);
    });
  }

  for (const c of fixtures.combined as Array<{ ticket: string; addr: string; combined: string }>) {
    it(`combined ${c.ticket}+${JSON.stringify(c.addr)}`, () => {
      expect(buildCombinedInvite(c.ticket, c.addr)).toBe(c.combined);
    });
  }

  for (const c of fixtures.lifecycle as Array<{
    name: string;
    identity_id: string;
    expires_at_ms: number | null;
    members: { identity_id: string; status: string }[];
    now_ms: number;
    state: InviteState;
  }>) {
    it(`lifecycle: ${c.name}`, () => {
      expect(
        inviteState({ identityId: c.identity_id, expiresAtMs: c.expires_at_ms }, c.members, c.now_ms),
      ).toBe(c.state);
    });
  }
});
