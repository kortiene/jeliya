import { describe, expect, it } from 'vitest';
import {
  activityBreakdown,
  eventCategory,
  groupRuns,
  isAllMessages,
  matchesActivityFilter,
  runSummary,
  type ActivityCategory,
  type TimelineRow,
} from './timelineRuns';
import type { TimelineEvent, TimelineKind } from './protocol';
import fixtures from './conformance/timeline-runs.fixtures.json';

interface FixtureEvent {
  event_id: string;
  ts: number;
  sender: { identity_id: string };
  kind: string;
  artifacts?: string[];
}

const toEvent = (e: FixtureEvent): TimelineEvent => ({
  event_id: e.event_id,
  room_id: 'blake3:room',
  ts: e.ts,
  sender: { identity_id: e.sender.identity_id, device_id: 'dev', role: 'agent' },
  kind: e.kind as TimelineKind,
  ...(e.artifacts ? { artifacts: e.artifacts } : {}),
});

/** Collapse a projected row to the fixture's compact shape for comparison. */
const rowShape = (row: TimelineRow): unknown =>
  row.kind === 'event'
    ? { event: row.event.event_id }
    : {
        run: {
          sender: row.run.senderId,
          events: row.run.events.map((e) => e.event_id),
          artifacts: runSummary(row.run).artifacts,
        },
      };

describe('matchesActivityFilter', () => {
  it('passes everything when no category is active', () => {
    expect(matchesActivityFilter('message', new Set())).toBe(true);
    expect(matchesActivityFilter('agent_status', new Set())).toBe(true);
  });
  it('isolates the active categories', () => {
    const active = new Set<ActivityCategory>(['agent-runs']);
    expect(matchesActivityFilter('agent_status', active)).toBe(true);
    expect(matchesActivityFilter('message', active)).toBe(false);
  });
});

describe('runSummary', () => {
  it('takes the latest event and the real span', () => {
    const events = [
      toEvent({ event_id: 'a', ts: 10, sender: { identity_id: 'x' }, kind: 'agent_status' }),
      toEvent({ event_id: 'b', ts: 40, sender: { identity_id: 'x' }, kind: 'agent_status' }),
    ];
    const s = runSummary({ senderId: 'x', events });
    expect(s.count).toBe(2);
    expect(s.latest.event_id).toBe('b');
    expect(s.firstTs).toBe(10);
    expect(s.lastTs).toBe(40);
  });
});

// The shared corpus, replayed here and (identically) in
// app/test/timeline_runs_test.dart so React and Flutter fold runs, classify
// activity, and count new items from ONE source (issue #65).
describe('shared timeline-runs fixtures (parity with Flutter)', () => {
  const grouping = fixtures.grouping as Array<{
    name: string;
    events: FixtureEvent[];
    expect: unknown[];
  }>;

  it('covers every distinct grouping case name once', () => {
    expect(new Set(grouping.map((c) => c.name)).size).toBe(grouping.length);
  });

  for (const c of grouping) {
    it(`grouping: ${c.name}`, () => {
      const rows = groupRuns(c.events.map(toEvent));
      expect(rows.map(rowShape)).toEqual(c.expect);
      // Every input event is preserved exactly once, in order.
      const flattened = rows.flatMap((r) => (r.kind === 'event' ? [r.event] : r.run.events));
      expect(flattened.map((e) => e.event_id)).toEqual(c.events.map((e) => e.event_id));
    });
  }

  for (const c of fixtures.categories as Array<{ kind: string; category: string }>) {
    it(`category: ${c.kind} → ${c.category}`, () => {
      expect(eventCategory(c.kind)).toBe(c.category);
    });
  }

  for (const c of fixtures.breakdown as Array<{
    name: string;
    kinds: string[];
    total: number;
    messages: number;
    nonMessages: number;
    allMessages: boolean;
  }>) {
    it(`breakdown: ${c.name}`, () => {
      const b = activityBreakdown(c.kinds);
      expect(b).toEqual({ total: c.total, messages: c.messages, nonMessages: c.nonMessages });
      expect(isAllMessages(b)).toBe(c.allMessages);
    });
  }
});
