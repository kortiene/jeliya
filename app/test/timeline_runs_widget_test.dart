/// Timeline P12 rendering (issue #65): the Flutter side of folded agent runs,
/// view-only activity filters, and the honest floating counter — layered ON TOP
/// of the reconnect-anchor machinery (#68) without disturbing it. Mirrors the
/// committed React P11 (ui/src/components/Timeline.tsx).
///
/// Covered: a same-sender agent-status streak folds to ONE card showing only
/// the latest status behind a "Show N updates" disclosure that expands to reveal
/// every original update and collapses to hide them again (history is folded,
/// never lost); an Agent-runs filter chip isolates agent activity and hides
/// conversation, and clearing it restores history; a pending/failed message is
/// exempt from the filter so Retry stays reachable; the counter switches to the
/// "new activity" wording the moment a non-message is among the new items; and
/// reduced motion makes the pill's jump-to-bottom instant.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/screens/composer.dart';
import 'package:jeliya_app/src/screens/timeline.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart'
    show Sender, TimelineEvent, TimelineKinds;
import 'package:jeliya_protocol/testing.dart';

import 'helpers.dart';

// The three real fixture statuses in the ONE folded backend run (9:05–9:09).
const _statusFirst = 'Scaffolding room invite flow and peer discovery.';
const _statusSecond = 'Peer discovery handshakes verified across 3 relays.';
const _statusLatest = 'Invite tickets minting and redeeming end-to-end.';

Finder _timelineScrollable() => find.descendant(
    of: find.byType(TimelineView), matching: find.byType(Scrollable));

ScrollPosition _position(WidgetTester tester) =>
    tester.state<ScrollableState>(_timelineScrollable()).position;

Finder _composerField() => find.descendant(
    of: find.byType(Composer), matching: find.byType(TextField));

/// Jump the timeline to [offset] (0 = oldest at the top) and let stick/anchor
/// state settle.
Future<void> _scrollTo(WidgetTester tester, double offset) async {
  final pos = _position(tester);
  pos.jumpTo(offset.clamp(0.0, pos.maxScrollExtent));
  await tester.pump();
  await tester.pump();
}

/// Jump to the very bottom of the (current) timeline so on-screen assertions are
/// independent of where filtering left the offset.
Future<void> _toBottom(WidgetTester tester) async {
  final pos = _position(tester);
  pos.jumpTo(pos.maxScrollExtent);
  await tester.pump();
  await tester.pump();
}

List<TimelineEvent> _messages(int count,
    {required int startTs, required String prefix, MockPerson? from}) {
  return [
    for (var i = 0; i < count; i++)
      syntheticMessage(
          ts: startTs + i * 60000, body: '$prefix $i', from: from),
  ];
}

/// A minimal `agent_status` event (a NON-message), for exercising the counter's
/// activity wording and category filtering.
TimelineEvent _status(
    {required int ts, required String body, MockPerson? from}) {
  final person = from ?? MockPeople.backendAgent;
  return TimelineEvent(
    eventId: 'test-status-$ts',
    roomId: MockClient.mainRoomId,
    ts: ts,
    sender: Sender(
        identityId: person.identityId,
        deviceId: person.deviceId,
        role: person.role),
    kind: TimelineKinds.agentStatus,
    label: 'working',
    statusMessage: body,
    progress: 50,
  );
}

void main() {
  final now = DateTime.now().millisecondsSinceEpoch;
  final historyStart = now - 7 * 86400000;
  final freshStart = now + 3600000;

  testWidgets(
      'the backend agent run folds to one card (latest status + Show 3 updates); '
      'expanding reveals every update and collapsing hides them',
      (tester) async {
    await pumpReadyApp(tester, newMockClient());
    // Bring the early backend run (9:05–9:09, near the top) into view.
    await _scrollTo(tester, 0);

    // Collapsed: only the LATEST signed status shows, behind an honest
    // "Show 3 updates" disclosure. The two earlier updates are folded away.
    expect(find.text(_statusLatest), findsOneWidget);
    expect(find.text(en.timelineRunShow(3)), findsOneWidget);
    expect(find.text(_statusFirst), findsNothing);
    expect(find.text(_statusSecond), findsNothing);

    // Expand: every original update is revealed in order; the disclosure flips
    // to Hide. The latest now appears twice — the summary card AND its last
    // history child.
    await tester.tap(find.text(en.timelineRunShow(3)));
    await tester.pump();
    await tester.pump();
    expect(find.text(_statusFirst), findsOneWidget);
    expect(find.text(_statusSecond), findsOneWidget);
    expect(find.text(_statusLatest), findsNWidgets(2));
    expect(find.text(en.timelineRunHide), findsOneWidget);
    expect(find.text(en.timelineRunShow(3)), findsNothing);

    // Collapse again: the folded updates hide, the summary latest stays.
    await tester.tap(find.text(en.timelineRunHide));
    await tester.pump();
    await tester.pump();
    expect(find.text(_statusFirst), findsNothing);
    expect(find.text(_statusSecond), findsNothing);
    expect(find.text(_statusLatest), findsOneWidget);
    expect(find.text(en.timelineRunShow(3)), findsOneWidget);
  });

  testWidgets(
      'the Agent runs filter isolates agent activity and hides conversation; '
      'clearing it restores history',
      (tester) async {
    await pumpReadyApp(tester, newMockClient());

    // Baseline at the bottom: a file-share tile is on screen.
    await _toBottom(tester);
    expect(find.text('test-report.json'), findsOneWidget);

    // Filter to Agent runs: agent statuses remain, the file-share is hidden.
    await tester.tap(find.text(en.timelineFilterAgentRuns));
    await tester.pump();
    await tester.pump();
    await _toBottom(tester);
    expect(find.text('Completed test suite v1. Summary attached.'),
        findsOneWidget);
    expect(find.text('Sync convergence suite running (14/24 green).'),
        findsOneWidget);
    // The file-share tile is gone (its artifact chip "⎘ test-report.json" is a
    // different string, so an exact match still finds nothing).
    expect(find.text('test-report.json'), findsNothing);

    // Clear the filter: history returns — filtering was a view, not a delete.
    await tester.tap(find.text(en.timelineFilterAgentRuns));
    await tester.pump();
    await tester.pump();
    await _toBottom(tester);
    expect(find.text('test-report.json'), findsOneWidget);
  });

  testWidgets(
      'a failed pending message survives the Agent runs filter — Retry stays reachable',
      (tester) async {
    await pumpReadyApp(tester, FlakySendClient(newMockClient()));
    const body = 'pending survives the filter';

    await tester.enterText(_composerField(), body);
    await tester.pump();
    await tester.tap(find.text('➤'));
    await tester.pump(const Duration(milliseconds: 100));
    await tester.pump();
    await tester.pump();
    expect(find.text(en.timelinePendingFailed), findsOneWidget);
    expect(find.text(body), findsOneWidget);

    // Filter to Agent runs: the message category is hidden, but pending is
    // exempt entirely — the retry affordance must never disappear behind a view.
    await tester.tap(find.text(en.timelineFilterAgentRuns));
    await tester.pump();
    await tester.pump();
    await _toBottom(tester);
    expect(find.text(body), findsOneWidget);
    expect(find.text(en.timelinePendingFailed), findsOneWidget);
    expect(find.text(en.commonRetry), findsOneWidget);
  });

  testWidgets(
      'the counter reads "new activity" (not "new messages") when a non-message '
      'is among the new items',
      (tester) async {
    final client = ReplayClient(newMockClient());
    // Seed a tall history so the room scrolls and the reader can sit up in it.
    client.backlog
        .addAll(_messages(16, startTs: historyStart, prefix: 'history'));
    await pumpReadyApp(tester, client);

    // Read history, then a NON-message (agent status) arrives at the tail.
    await _scrollTo(tester, 0);
    client.pushEvent(_status(ts: freshStart, body: 'a brand new status'));
    await tester.pump();
    await tester.pump();
    await tester.pump();

    // Same count (1) as before, but the wording tells the truth about the kind.
    expect(find.text(en.timelineNewActivity(1)), findsOneWidget);
    expect(find.text(en.timelineNewMessages(1)), findsNothing);
  });

  testWidgets('reduced motion: tapping the pill jumps to the bottom instantly',
      (tester) async {
    final client = ReplayClient(newMockClient());
    client.backlog
        .addAll(_messages(16, startTs: historyStart, prefix: 'history'));
    await pumpReadyApp(tester, client);

    // Turn reduced motion on for this surface (cleared by clearAllTestValues).
    tester.platformDispatcher.accessibilityFeaturesTestValue =
        const FakeAccessibilityFeatures(disableAnimations: true);
    await tester.pump();

    // Read history, then a live tail message raises the pill.
    await _scrollTo(tester, 0);
    client.pushEvent(
        syntheticMessage(ts: freshStart, body: 'ping', from: MockPeople.sam));
    await tester.pump();
    await tester.pump();
    await tester.pump();
    expect(find.text(en.timelineNewMessages(1)), findsOneWidget);
    expect(_position(tester).pixels, closeTo(0, 1.0));

    // Tap the pill: under reduced motion the jump is instant — after a SINGLE
    // frame the viewport is already at the bottom (an animateTo would still be
    // mid-flight and land far from the extent).
    await tester.tap(find.text(en.timelineNewMessages(1)));
    await tester.pump();
    final p = _position(tester);
    expect(p.pixels, closeTo(p.maxScrollExtent, 1.0));
  });
}
