import { describe, expect, it } from 'vitest';
import {
  ATTENTION_ORDER,
  attentionRank,
  attentionReason,
  hasNumericProgress,
  needsAttention,
  statusUnverified,
  type AttentionReason,
} from './fleet';
import fixtures from './conformance/fleet-attention.fixtures.json';

describe('attentionReason', () => {
  it('flags a red latest-status tone as failed', () => {
    expect(attentionReason('working', 'build_failed')).toBe('failed');
    expect(attentionReason('online-idle', 'sync_error')).toBe('failed');
    expect(attentionReason('online-idle', 'blocked')).toBe('failed');
  });

  it('flags a blue latest-status tone as review', () => {
    expect(attentionReason('online-idle', 'awaiting_review')).toBe('review');
    expect(attentionReason('working', 'reviewing')).toBe('review');
    expect(attentionReason('online-idle', 'pending')).toBe('review');
  });

  it('flags stale liveness even with a working-class latest label', () => {
    expect(attentionReason('stale', 'working')).toBe('stale');
  });

  it('flags offline-after-work: offline with a proven status history', () => {
    expect(attentionReason('offline', 'tests_passed')).toBe('offline');
  });

  it('does not flag an offline agent that never posted a status', () => {
    expect(attentionReason('offline', null)).toBeNull();
  });

  it('does not flag a healthy working or idle agent', () => {
    expect(attentionReason('working', 'working')).toBeNull();
    expect(attentionReason('online-idle', 'tests_passed')).toBeNull();
  });

  it('ranks a failure above an offline-after-work claim (severity order)', () => {
    expect(attentionReason('offline', 'deploy_failed')).toBe('failed');
  });
});

describe('attentionRank', () => {
  it('orders failed < review < stale < offline < (no attention)', () => {
    expect(attentionRank('working', 'build_failed')).toBe(0);
    expect(attentionRank('online-idle', 'awaiting_review')).toBe(1);
    expect(attentionRank('stale', 'working')).toBe(2);
    expect(attentionRank('offline', 'tests_passed')).toBe(3);
    expect(attentionRank('working', 'working')).toBe(ATTENTION_ORDER.length);
  });
});

describe('statusUnverified', () => {
  it('is true for stale and offline (the latest label is no longer a live claim)', () => {
    expect(statusUnverified('stale')).toBe(true);
    expect(statusUnverified('offline')).toBe(true);
  });
  it('is false for working and online-idle (the peer backs the claim)', () => {
    expect(statusUnverified('working')).toBe(false);
    expect(statusUnverified('online-idle')).toBe(false);
  });
});

describe('hasNumericProgress', () => {
  it('accepts a finite number and rejects everything else', () => {
    expect(hasNumericProgress(0)).toBe(true);
    expect(hasNumericProgress(68)).toBe(true);
    expect(hasNumericProgress(null)).toBe(false);
    expect(hasNumericProgress(undefined)).toBe(false);
    expect(hasNumericProgress(Number.NaN)).toBe(false);
    expect(hasNumericProgress(Number.POSITIVE_INFINITY)).toBe(false);
  });
});

// The shared classifier fixture, replayed here and (identically) in the Dart
// conventions test — the parity guard for the closed attention set.
describe('shared fleet-attention fixtures (parity with Dart)', () => {
  interface FixtureCase {
    name: string;
    liveness: string;
    latest_label: string | null;
    expect: { reason: AttentionReason | null; unverified: boolean };
  }
  const cases = fixtures.cases as FixtureCase[];

  it('covers all eight AC states plus the healthy/never-posted negatives', () => {
    expect(cases.length).toBe(12);
    for (const state of ['failed', 'awaiting-review', 'stale', 'offline-after-work']) {
      expect(cases.some((c) => c.name === state)).toBe(true);
    }
  });

  for (const c of cases) {
    it(`case "${c.name}" → reason ${c.expect.reason}, unverified ${c.expect.unverified}`, () => {
      const liveness = c.liveness as Parameters<typeof attentionReason>[0];
      expect(attentionReason(liveness, c.latest_label)).toBe(c.expect.reason);
      expect(needsAttention(liveness, c.latest_label)).toBe(c.expect.reason !== null);
      expect(statusUnverified(liveness)).toBe(c.expect.unverified);
    });
  }
});
