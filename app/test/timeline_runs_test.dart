/// Timeline run-folding + activity-classification + counter parity (issue #65):
/// the Dart side of the shared projection, replaying the SAME corpus as
/// ui/src/lib/timelineRuns.test.ts from ui/src/lib/conformance/
/// timeline-runs.fixtures.json — so React and Flutter fold agent runs, classify
/// activity, and word the floating counter identically.
library;

import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart';

/// Walk up to the repo root, where the shared fixture file lives.
Directory _repoRoot() {
  var dir = Directory.current;
  for (var i = 0; i < 8; i++) {
    if (File('${dir.path}/ui/src/lib/conformance/timeline-runs.fixtures.json')
        .existsSync()) {
      return dir;
    }
    final parent = dir.parent;
    if (parent.path == dir.path) break;
    dir = parent;
  }
  throw StateError('could not locate repo root from ${Directory.current.path}');
}

TimelineEvent _toEvent(Map<String, dynamic> e) => TimelineEvent(
      eventId: e['event_id'] as String,
      roomId: 'blake3:room',
      ts: e['ts'] as int,
      sender: Sender(
        identityId: (e['sender'] as Map)['identity_id'] as String,
        deviceId: 'dev',
        role: Roles.agent,
      ),
      kind: e['kind'] as String,
      artifacts: (e['artifacts'] as List?)?.cast<String>() ?? const [],
    );

/// The projected row in the fixture's compact shape, for comparison.
Object _rowShape(TimelineRow row) {
  if (!row.isRun) return {'event': row.event!.eventId};
  final run = row.run!;
  return {
    'run': {
      'sender': run.senderId,
      'events': [for (final e in run.events) e.eventId],
      'artifacts': runSummary(run).artifacts,
    },
  };
}

void main() {
  final root = _repoRoot();
  final fixtures = jsonDecode(
    File('${root.path}/ui/src/lib/conformance/timeline-runs.fixtures.json')
        .readAsStringSync(),
  ) as Map<String, dynamic>;

  group('matchesActivityFilter', () {
    test('passes everything when no category is active', () {
      expect(matchesActivityFilter('message', {}), isTrue);
      expect(matchesActivityFilter('agent_status', {}), isTrue);
    });
    test('isolates the active categories', () {
      final active = {ActivityCategories.agentRuns};
      expect(matchesActivityFilter('agent_status', active), isTrue);
      expect(matchesActivityFilter('message', active), isFalse);
    });
  });

  group('shared timeline-runs fixtures (parity with React)', () {
    final grouping = (fixtures['grouping'] as List).cast<Map<String, dynamic>>();

    test('covers every distinct grouping case name once', () {
      final names = grouping.map((c) => c['name'] as String).toSet();
      expect(names.length, grouping.length);
    });

    for (final c in grouping) {
      test('grouping: ${c['name']}', () {
        final events = (c['events'] as List)
            .cast<Map<String, dynamic>>()
            .map(_toEvent)
            .toList();
        final rows = groupRuns(events);
        expect(rows.map(_rowShape).toList(), c['expect']);
        // Every input event is preserved exactly once, in order.
        final flattened = [
          for (final r in rows)
            if (r.isRun) ...r.run!.events else r.event!,
        ];
        expect([for (final e in flattened) e.eventId],
            [for (final e in events) e.eventId]);
      });
    }

    for (final c in (fixtures['categories'] as List).cast<Map<String, dynamic>>()) {
      test('category: ${c['kind']} → ${c['category']}', () {
        expect(eventCategory(c['kind'] as String), c['category']);
      });
    }

    for (final c in (fixtures['breakdown'] as List).cast<Map<String, dynamic>>()) {
      test('breakdown: ${c['name']}', () {
        final b = activityBreakdown((c['kinds'] as List).cast<String>());
        expect(b.total, c['total']);
        expect(b.messages, c['messages']);
        expect(b.nonMessages, c['nonMessages']);
        expect(isAllMessages(b), c['allMessages']);
      });
    }
  });
}
