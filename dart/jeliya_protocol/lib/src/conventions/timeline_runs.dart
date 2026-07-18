/// Timeline run-folding + activity-filter + counter projection — the Dart
/// mirror of ui/src/lib/timelineRuns.ts (issue #65). Kept 1:1 with the
/// reference, and both are replayed against
/// ui/src/lib/conformance/timeline-runs.fixtures.json so React and Flutter
/// fold, filter, and count one timeline identically.
///
/// Honesty rules (docs/room-attention.md): there is NO run/task token on the
/// wire, so a run is only ever a maximal streak of consecutive `agent_status`
/// events from ONE sender within a bounded window — nothing is invented. A run
/// summary exposes only real evidence (latest signed status, its timestamp, the
/// actor, real progress, real artifacts). Filters SEPARATE, they never delete
/// history, and pending messages are never filtered out.
library;

import '../models.dart' show TimelineEvent, TimelineKinds;

/// Two consecutive agent-status events fold into the same run only within this
/// gap — the same 5-minute window the timeline already uses to group messages.
const int runGapMs = 5 * 60 * 1000;

/// The view-only activity buckets a filter can isolate. A filter never removes
/// events from history; it only narrows what is shown. String values match the
/// reference so shared fixtures classify identically across clients.
abstract final class ActivityCategories {
  static const String conversation = 'conversation';
  static const String agentRuns = 'agent-runs';
  static const String membership = 'membership';
  static const String files = 'files';
  static const String pipes = 'pipes';
  static const List<String> all = [
    conversation,
    agentRuns,
    membership,
    files,
    pipes,
  ];
}

/// Which bucket a signed event kind belongs to. Unknown/forward-compat kinds
/// and plain messages are `conversation`; pending optimistic messages are
/// always `conversation` too (they carry no wire kind).
String eventCategory(String kind) {
  switch (kind) {
    case TimelineKinds.agentStatus:
      return ActivityCategories.agentRuns;
    case TimelineKinds.fileShared:
      return ActivityCategories.files;
    case TimelineKinds.pipeOpened:
    case TimelineKinds.pipeClosed:
      return ActivityCategories.pipes;
    case TimelineKinds.roomCreated:
    case TimelineKinds.memberInvited:
    case TimelineKinds.memberJoined:
    case TimelineKinds.memberLeft:
      return ActivityCategories.membership;
    default:
      return ActivityCategories.conversation;
  }
}

/// Does an event of [kind] pass the active category set? An EMPTY set means no
/// filter is applied, so everything passes. Callers must exempt pending
/// messages entirely — they are never filtered.
bool matchesActivityFilter(String kind, Set<String> active) =>
    active.isEmpty || active.contains(eventCategory(kind));

/// A folded run of agent-status updates: ≥2 consecutive `agent_status` events
/// from one sender within [runGapMs]. Every original event is preserved in
/// [events], chronological, so expanding the run reveals each signed update.
class TimelineRun {
  const TimelineRun(this.senderId, this.events);

  final String senderId;
  final List<TimelineEvent> events;
}

/// One folded row: either a standalone [event] (any kind, incl. a lone
/// agent-status) or a [run]. The list preserves every input event exactly once.
class TimelineRow {
  const TimelineRow.event(TimelineEvent this.event) : run = null;
  const TimelineRow.run(TimelineRun this.run) : event = null;

  final TimelineEvent? event;
  final TimelineRun? run;

  bool get isRun => run != null;
}

/// Fold a chronological event list into rows, collapsing maximal streaks of
/// consecutive same-sender `agent_status` events (within [runGapMs]) into runs.
/// A different sender, a non-status event, or a wider gap breaks the streak; a
/// streak of one stays a standalone event. Every input event appears once.
List<TimelineRow> groupRuns(List<TimelineEvent> events) {
  final rows = <TimelineRow>[];
  var buffer = <TimelineEvent>[];

  void flush() {
    if (buffer.length >= 2) {
      rows.add(TimelineRow.run(TimelineRun(buffer.first.sender.identityId, buffer)));
    } else {
      for (final e in buffer) {
        rows.add(TimelineRow.event(e));
      }
    }
    buffer = <TimelineEvent>[];
  }

  for (final event in events) {
    if (event.kind != TimelineKinds.agentStatus) {
      flush();
      rows.add(TimelineRow.event(event));
      continue;
    }
    final prev = buffer.isEmpty ? null : buffer.last;
    if (prev != null &&
        (prev.sender.identityId != event.sender.identityId ||
            event.ts - prev.ts > runGapMs)) {
      flush();
    }
    buffer.add(event);
  }
  flush();
  return rows;
}

/// Real, non-fabricated evidence for a run summary: how many updates, the latest
/// signed status event, the time span, and the deduped union of every artifact
/// the run produced (first-seen order).
class RunSummary {
  const RunSummary({
    required this.count,
    required this.latest,
    required this.firstTs,
    required this.lastTs,
    required this.artifacts,
  });

  final int count;
  final TimelineEvent latest;
  final int firstTs;
  final int lastTs;
  final List<String> artifacts;
}

RunSummary runSummary(TimelineRun run) {
  final events = run.events;
  final artifacts = <String>[];
  final seen = <String>{};
  for (final event in events) {
    for (final id in event.artifacts) {
      if (seen.add(id)) artifacts.add(id);
    }
  }
  return RunSummary(
    count: events.length,
    latest: events.last,
    firstTs: events.first.ts,
    lastTs: events.last.ts,
    artifacts: artifacts,
  );
}

/// The message-vs-non-message split of a batch of new items, so the floating
/// counter can say "N new messages" only when every new item is a message, and
/// "N new activity" the moment a non-message event is included. Pass the kind of
/// each new item; pending messages count as `message`.
class ActivityBreakdown {
  const ActivityBreakdown({
    required this.total,
    required this.messages,
    required this.nonMessages,
  });

  final int total;
  final int messages;
  final int nonMessages;
}

ActivityBreakdown activityBreakdown(List<String> kinds) {
  var messages = 0;
  for (final kind in kinds) {
    if (kind == TimelineKinds.message) messages += 1;
  }
  return ActivityBreakdown(
    total: kinds.length,
    messages: messages,
    nonMessages: kinds.length - messages,
  );
}

/// True when the counter should read "new messages" (every new item is a
/// message); false when it must read "new activity". An empty batch is
/// vacuously all-messages.
bool isAllMessages(ActivityBreakdown breakdown) => breakdown.nonMessages == 0;
