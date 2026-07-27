/// Live activity in rooms the user is NOT currently viewing (issue #151).
///
/// Every open room binds its own endpoint and pushes its own `room.event`
/// frames, so the daemon fans out for all of them. The session used to apply a
/// push only when it matched the open [RoomStore], which threw away everything
/// happening in the other rooms: the rail stayed silent until the next
/// `room.list`.
///
/// These tests pin the fix at the seam that matters — [DaemonSession.rooms]
/// (what every room-list surface renders) and [DaemonSession.isRoomUnread]
/// (the dot's verdict) — rather than at a widget, so they hold for both the
/// desktop rail and the phone home.
///
/// The honesty rules still bind: recency and kind come off the SIGNED event in
/// the push, never a local clock, and a live event must not seed its own
/// baseline (that would mark the room seen the instant it became active, and
/// the dot could never appear).
library;

import 'dart:async';

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/session/daemon_session.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart';
import 'package:jeliya_protocol/testing.dart';

import 'helpers.dart';

/// Delivers everything the mock does, plus pushes a test injects by hand. The
/// mock only simulates activity in its main room, and only on a timer; these
/// tests need an event in a DIFFERENT room, at a chosen timestamp.
class _PushInjectingClient extends DelegatingClient {
  _PushInjectingClient(super.inner) {
    inner.pushes.listen(_merged.add);
  }

  final StreamController<Push> _merged = StreamController<Push>.broadcast();

  @override
  Stream<Push> get pushes => _merged.stream;

  void injectRoomEvent(String roomId, TimelineEvent event) =>
      _merged.add(Push('room.event', {
        'room_id': roomId,
        'event': event.toJson(),
      }));
}

/// The room the session is NOT viewing: any listed room other than the one the
/// mock opens by default.
RoomSummary _otherRoom(DaemonSession session) => session.rooms
    .firstWhere((r) => r.roomId != MockClient.mainRoomId && r.status == 'active');

void main() {
  testWidgets('an event in a room you are not viewing advances that room\'s '
      'recency and raises its unread dot', (tester) async {
    final client = _PushInjectingClient(newMockClient());
    final ready = await pumpReadyMobileApp(tester, client, size: const Size(360, 800));
    final session = ready.session;

    final before = _otherRoom(session);
    expect(session.currentRoomId, isNot(before.roomId),
        reason: 'the point of the test is a room that is NOT current');
    expect(session.isRoomUnread(before), isFalse,
        reason: 'room.list seeded a baseline, so nothing is unread yet');

    // A signed event lands in that other room, newer than its baseline.
    final newer = (before.lastEventTs ?? 0) + 60_000;
    client.injectRoomEvent(
      before.roomId,
      syntheticMessage(ts: newer, body: 'while you were away', roomId: before.roomId),
    );
    await pumpSteps(tester, steps: 3);

    final after = _otherRoom(session);
    expect(after.lastEventTs, newer,
        reason: 'recency is the signed ts off the push, not a clock read');
    expect(after.lastEventKind, TimelineKinds.message);
    expect(session.isRoomUnread(after), isTrue,
        reason: 'activity past the baseline in a non-current room IS unread');
  });

  testWidgets('a live event never seeds its own baseline, so the dot survives',
      (tester) async {
    final client = _PushInjectingClient(newMockClient());
    final ready = await pumpReadyMobileApp(tester, client, size: const Size(360, 800));
    final session = ready.session;
    final room = _otherRoom(session);

    final newer = (room.lastEventTs ?? 0) + 60_000;
    client.injectRoomEvent(
      room.roomId,
      syntheticMessage(ts: newer, body: 'first', roomId: room.roomId),
    );
    await pumpSteps(tester, steps: 3);
    expect(session.isRoomUnread(_otherRoom(session)), isTrue);

    // A second refresh cycle must not quietly acknowledge it: seeding reads the
    // room.list snapshot, never the live-merged rows. Fired without awaiting —
    // the mock's call latency only elapses while the tester pumps fake time.
    unawaited(session.refreshRooms());
    await pumpSteps(tester, steps: 5);
    expect(session.isRoomUnread(_otherRoom(session)), isTrue,
        reason: 'an unacknowledged room stays unread across a room.list refresh');
  });

  testWidgets('recency never moves backwards on a stale push', (tester) async {
    final client = _PushInjectingClient(newMockClient());
    final ready = await pumpReadyMobileApp(tester, client, size: const Size(360, 800));
    final session = ready.session;
    final room = _otherRoom(session);

    final newer = (room.lastEventTs ?? 0) + 60_000;
    client.injectRoomEvent(
      room.roomId,
      syntheticMessage(ts: newer, body: 'newest', roomId: room.roomId),
    );
    await pumpSteps(tester, steps: 3);

    // A late-validated backlog event carries an OLDER signed ts. It must not
    // rewind what the row already proved.
    client.injectRoomEvent(
      room.roomId,
      syntheticMessage(ts: newer - 30_000, body: 'older', roomId: room.roomId),
    );
    await pumpSteps(tester, steps: 3);

    expect(_otherRoom(session).lastEventTs, newer);
  });
}
