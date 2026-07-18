// Timeline run-folding + activity-filter + counter projection (issue #65).
// Repeated agent status updates overwhelm human conversation, and the floating
// counter calls every event a "message". This shared, pure projection folds a
// signed timeline into expandable agent RUNS, classifies each event into a
// view-only activity CATEGORY, and breaks a batch of new items into message vs
// non-message so the counter can tell the truth.
//
// Mirrored 1:1 in dart/jeliya_protocol/lib/src/conventions/timeline_runs.dart,
// and both are replayed against ui/src/lib/conformance/timeline-runs.fixtures.json
// so React and Flutter fold, filter, and count one timeline identically.
//
// Honesty rules (docs/room-attention.md): there is NO run/task token on the
// wire, so a run is only ever a maximal streak of consecutive `agent_status`
// events from ONE sender within a bounded window — nothing is invented. A run
// summary exposes only real evidence (the latest signed status, its timestamp,
// the actor, real progress, real artifacts); no fabricated aggregate state.
// Filters SEPARATE, they never delete history, and pending messages are never
// filtered out.

import type { TimelineEvent } from './protocol';

/** Two consecutive agent-status events fold into the same run only when they
 *  are within this gap — the same 5-minute window the timeline already uses to
 *  group consecutive messages, so a sparse status trickle stays inspectable
 *  line-by-line while a burst collapses. */
export const RUN_GAP_MS = 5 * 60 * 1000;

const AGENT_STATUS = 'agent_status';

/** The view-only activity buckets a filter can isolate (AC: conversation, agent
 *  runs, membership, files, pipes). A filter never removes events from history;
 *  it only narrows what is shown. */
export type ActivityCategory = 'conversation' | 'agent-runs' | 'membership' | 'files' | 'pipes';

export const ACTIVITY_CATEGORIES: readonly ActivityCategory[] = [
  'conversation',
  'agent-runs',
  'membership',
  'files',
  'pipes',
];

/** Which bucket a signed event kind belongs to. Unknown/forward-compat kinds
 *  and plain messages are `conversation`; pending optimistic messages are
 *  always `conversation` too (they carry no wire kind). */
export function eventCategory(kind: string): ActivityCategory {
  switch (kind) {
    case AGENT_STATUS:
      return 'agent-runs';
    case 'file_shared':
      return 'files';
    case 'pipe_opened':
    case 'pipe_closed':
      return 'pipes';
    case 'room_created':
    case 'member_invited':
    case 'member_joined':
    case 'member_left':
      return 'membership';
    default:
      return 'conversation';
  }
}

/** Does an event of [kind] pass the active category set? An EMPTY set means no
 *  filter is applied, so everything passes. Callers must exempt pending
 *  messages entirely — they are never filtered (AC: pending retry stays
 *  reachable). */
export function matchesActivityFilter(kind: string, active: ReadonlySet<ActivityCategory>): boolean {
  return active.size === 0 || active.has(eventCategory(kind));
}

/** A folded run of agent-status updates: ≥2 consecutive `agent_status` events
 *  from one sender within [RUN_GAP_MS]. Every original event is preserved in
 *  [events], chronological, so expanding the run reveals each signed update. */
export interface TimelineRun {
  senderId: string;
  events: TimelineEvent[];
}

/** One folded row: either a standalone event (any kind, incl. a lone
 *  agent-status) or a run. The list preserves every input event exactly once,
 *  in order. */
export type TimelineRow =
  | { kind: 'event'; event: TimelineEvent }
  | { kind: 'run'; run: TimelineRun };

/** Fold a chronological event list into rows, collapsing maximal streaks of
 *  consecutive same-sender `agent_status` events (within [RUN_GAP_MS]) into
 *  runs. A different sender, a non-status event, or a gap wider than the window
 *  breaks the streak. A streak of one stays a standalone event (nothing to
 *  fold). Every input event appears exactly once. */
export function groupRuns(events: readonly TimelineEvent[]): TimelineRow[] {
  const rows: TimelineRow[] = [];
  let buffer: TimelineEvent[] = [];

  const flush = () => {
    if (buffer.length >= 2) {
      rows.push({ kind: 'run', run: { senderId: buffer[0].sender.identity_id, events: buffer } });
    } else {
      for (const e of buffer) rows.push({ kind: 'event', event: e });
    }
    buffer = [];
  };

  for (const event of events) {
    if (event.kind !== AGENT_STATUS) {
      flush();
      rows.push({ kind: 'event', event });
      continue;
    }
    const prev = buffer[buffer.length - 1];
    if (prev && (prev.sender.identity_id !== event.sender.identity_id || event.ts - prev.ts > RUN_GAP_MS)) {
      flush();
    }
    buffer.push(event);
  }
  flush();
  return rows;
}

/** Real, non-fabricated evidence for a run summary: how many updates, the
 *  latest signed status event (its label / status_message / progress are what
 *  the summary shows), the time span, and the deduped union of every artifact
 *  the run produced (first-seen order). */
export interface RunSummary {
  count: number;
  latest: TimelineEvent;
  firstTs: number;
  lastTs: number;
  artifacts: string[];
}

export function runSummary(run: TimelineRun): RunSummary {
  const { events } = run;
  const artifacts: string[] = [];
  const seen = new Set<string>();
  for (const event of events) {
    for (const id of event.artifacts ?? []) {
      if (!seen.has(id)) {
        seen.add(id);
        artifacts.push(id);
      }
    }
  }
  return {
    count: events.length,
    latest: events[events.length - 1],
    firstTs: events[0].ts,
    lastTs: events[events.length - 1].ts,
    artifacts,
  };
}

/** The message-vs-non-message split of a batch of new items, so the floating
 *  counter can say "N new messages" only when every new item is a message, and
 *  "N new activity" the moment a non-message event is included. Pass the kind
 *  of each new item; pending messages count as `message`. */
export interface ActivityBreakdown {
  total: number;
  messages: number;
  nonMessages: number;
}

export function activityBreakdown(kinds: readonly string[]): ActivityBreakdown {
  let messages = 0;
  for (const kind of kinds) {
    if (kind === 'message') messages += 1;
  }
  return { total: kinds.length, messages, nonMessages: kinds.length - messages };
}

/** True when the counter should read "new messages" (every new item is a
 *  message); false when it must read "new activity" (a non-message slipped in).
 *  An empty batch is vacuously all-messages. */
export function isAllMessages(breakdown: ActivityBreakdown): boolean {
  return breakdown.nonMessages === 0;
}
