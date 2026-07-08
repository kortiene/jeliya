/// Timeline fold: insert-by-`ts` + `event_id` dedup, ported 1:1 from the
/// reference `room.event` reducer (ui/src/App.tsx) and the walking skeleton's
/// `_insert` (app/lib/main.dart). Both ordering hazards are normative
/// (docs/PROTOCOL.md, Pushes):
///
/// 1. **Insert by `ts`, not arrival.** A peer that reconnects after a gap has
///    its backlog validated late, so a `room.event` can carry a `ts` older
///    than events already shown. Events are spliced in at the position their
///    `ts` dictates by walking back from the end — never blindly appended.
/// 2. **Dedup by `event_id`.** The same event can arrive via both the live
///    pump and a reconcile scan (`room.open` / `room.timeline` re-sync), so an
///    `event_id` already present is skipped (idempotent).
///
/// Equal-`ts` ties break by arrival order (append after existing equal-`ts`
/// events) for a stable render.
library;

import 'dart:collection';

import '../models.dart';

/// The pure form of the fold — the exact App.tsx `setTimeline` reducer.
/// Returns [timeline] itself (the same list instance) when [event]'s
/// `event_id` is already present, else a new list with [event] spliced in at
/// the position its `ts` dictates. Never mutates [timeline].
List<TimelineEvent> spliceEventByTs(List<TimelineEvent> timeline, TimelineEvent event) {
  if (timeline.any((e) => e.eventId == event.eventId)) return timeline;
  var i = timeline.length;
  while (i > 0 && timeline[i - 1].ts > event.ts) {
    i--;
  }
  return [...timeline.take(i), event, ...timeline.skip(i)];
}

/// The stateful form for a room store: keeps the chronological list plus the
/// seen-`event_id` set (app/lib/main.dart `_seen`), so dedup stays O(1) and
/// echo↔response reconciliation ([contains]) needs no list scan.
class TimelineFold {
  TimelineFold([Iterable<TimelineEvent> initial = const []]) {
    insertAll(initial);
  }

  final List<TimelineEvent> _events = [];
  final Set<String> _seen = {};

  /// The folded timeline, chronological — a live unmodifiable view.
  late final List<TimelineEvent> events = UnmodifiableListView(_events);

  int get length => _events.length;

  /// Whether an event with [eventId] has been folded. This is the lookup the
  /// pending-message lifecycle uses to detect "the echo of my own write beat
  /// its response" (docs/PROTOCOL.md Pushes, hazard 2).
  bool contains(String eventId) => _seen.contains(eventId);

  /// Splice [event] in at the position its `ts` dictates (equal-`ts` appends
  /// after existing equal-`ts` events). Returns false — and changes nothing —
  /// when its `event_id` is already present.
  bool insert(TimelineEvent event) {
    if (!_seen.add(event.eventId)) return false;
    var i = _events.length;
    while (i > 0 && _events[i - 1].ts > event.ts) {
      i--;
    }
    _events.insert(i, event);
    return true;
  }

  /// Fold a batch (the `room.open` / `room.timeline` baseline that live
  /// pushes splice into). Returns how many events were new.
  int insertAll(Iterable<TimelineEvent> events) {
    var inserted = 0;
    for (final event in events) {
      if (insert(event)) inserted++;
    }
    return inserted;
  }

  /// Drop everything — a room switch re-baselines from the next `room.open`.
  void clear() {
    _events.clear();
    _seen.clear();
  }
}
