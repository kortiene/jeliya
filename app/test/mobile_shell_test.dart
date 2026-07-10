/// Shared-behavior ports to the mobile shell (issue #17 integration): the
/// cross-cutting connection banner (shell_test idiom) and the optimistic-send
/// pending lifecycle (timeline_test) run against the SAME DaemonSession and
/// injected fakes as their desktop suites — only the surface differs: the
/// strict 360x800 phone shell, with the chat a pushed route under the Rooms
/// tab. The banner must overlay pushed routes too (the mobile shell renders
/// it above the whole tab stack), and every pending phase renders in the
/// mobile timeline without overflowing the strict surface.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/l10n/tokens.dart';
import 'package:jeliya_app/src/screens/composer.dart';
import 'package:jeliya_app/src/screens/room_header.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart'
    show ConnectionState, PendingPhases;

import 'helpers.dart';

/// The richest fixture room — the room the boot opens.
// i18n-exempt: fixture room name (coincides with modalRoomNamePlaceholder)
const String _mainRoomName = 'Build Iroh Rooms MVP';

Future<void> _openChat(WidgetTester tester) async {
  await tester.tap(find.text(_mainRoomName).hitTestable());
  await pumpSteps(tester, steps: 6);
  expect(find.byType(RoomHeader).hitTestable(), findsOneWidget);
}

Finder _composerField() => find.descendant(
    of: find.byType(Composer), matching: find.byType(TextField));

void main() {
  testWidgets(
      'connection banner narrates reconnecting and disconnected over the '
      'rooms home AND the pushed chat route', (tester) async {
    final client = ConnectionFakeClient(newMockClient());
    final ready = await pumpReadyMobileApp(tester, client);

    // The transport's describe() string fills the banner's {wsUrl} slot.
    final reconnectingBanner = en.shellBannerReconnecting(client.describe());
    final disconnectedBanner = en.shellBannerDisconnected;

    // Connected, on the rooms home: no banner; the identity footer's badge
    // (the sidebar badge's mobile counterpart) reads Connected.
    expect(find.text(reconnectingBanner), findsNothing);
    expect(find.text(disconnectedBanner), findsNothing);
    expect(find.text(en.shellConnConnected), findsOneWidget);

    client.setConnection(ConnectionState.reconnecting);
    await tester.pump(const Duration(milliseconds: 10));
    expect(find.text(reconnectingBanner), findsOneWidget);
    expect(find.text(en.shellConnReconnecting), findsOneWidget); // footer badge

    // Recover, then push the chat route: the banner must overlay pushed
    // routes too — the mobile shell hangs it above the whole tab stack.
    client.setConnection(ConnectionState.connected);
    await pumpSteps(tester, steps: 8);
    expect(find.text(reconnectingBanner), findsNothing);
    await _openChat(tester);

    client.setConnection(ConnectionState.disconnected);
    await tester.pump(const Duration(milliseconds: 10));
    expect(find.text(disconnectedBanner), findsOneWidget);
    // Composer is disabled while not connected.
    expect(tester.widget<TextField>(_composerField()).enabled, isFalse);

    // Back to connected: banner gone, bootstrap re-syncs, composer usable.
    client.setConnection(ConnectionState.connected);
    await pumpSteps(tester, steps: 8);
    expect(find.text(reconnectingBanner), findsNothing);
    expect(find.text(disconnectedBanner), findsNothing);
    expect(tester.widget<TextField>(_composerField()).enabled, isTrue);
    expect(ready.session.conn, ConnectionState.connected);

    expect(ready.overflows, isEmpty,
        reason: 'banner states must not overflow the strict phone surface:\n'
            '${ready.overflows.join('\n')}');
  });

  testWidgets(
      'pending send on the chat route: sending → reconciled by event_id '
      'when the echo wins', (tester) async {
    final ready = await pumpReadyMobileApp(tester, newMockClient());
    final session = ready.session;
    await _openChat(tester);
    final roomId = session.currentRoomId!;
    const body = 'hello from the mobile echo path';

    await tester.enterText(_composerField(), body);
    await tester.pump();
    // Drafts persist per room on each keystroke.
    expect(session.prefs.draftFor(roomId), body);

    await tester.tap(find.text(Tokens.composerSendGlyph));
    await tester.pump();
    // Optimistic: pending card renders immediately, draft cleared already.
    expect(find.text(en.timelinePendingSending), findsOneWidget);
    expect(find.text(body), findsOneWidget);
    expect(session.prefs.draftFor(roomId), isNull);

    // The mock delivers the room.event echo BEFORE the response resolves, so
    // the pending entry is dropped at the response — exactly one bubble left.
    await tester.pump(const Duration(milliseconds: 100));
    expect(session.room!.pendingMessages, isEmpty);
    expect(find.text(en.timelinePendingSending), findsNothing);
    expect(find.text(en.timelinePendingSyncing), findsNothing);
    expect(find.text(body), findsOneWidget);

    expect(ready.overflows, isEmpty,
        reason: 'the pending card must not overflow the strict phone '
            'surface:\n${ready.overflows.join('\n')}');
  });

  testWidgets(
      'pending send on the chat route: syncing is shown when the echo lags, '
      'then reconciles', (tester) async {
    final client = HeldPushClient(newMockClient());
    final ready = await pumpReadyMobileApp(tester, client);
    await _openChat(tester);
    const body = 'hello from the mobile held echo path';

    client.hold = true;
    await tester.enterText(_composerField(), body);
    await tester.pump();
    await tester.tap(find.text(Tokens.composerSendGlyph));
    await tester.pump();
    expect(find.text(en.timelinePendingSending), findsOneWidget);

    // Response resolves while the echo push is held back → phase 'syncing'
    // carrying the wire event_id.
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text(en.timelinePendingSyncing), findsOneWidget);
    final pending = ready.session.room!.pendingMessages.single;
    expect(pending.phase, PendingPhases.syncing);
    expect(pending.eventId, isNotNull);

    // Releasing the echo retires the pending entry by event_id — exactly one
    // rendered copy of the message (dedup, no double bubble).
    client.release();
    await tester.pump(const Duration(milliseconds: 50));
    await tester.pump();
    expect(ready.session.room!.pendingMessages, isEmpty);
    expect(find.text(en.timelinePendingSyncing), findsNothing);
    expect(find.text(body), findsOneWidget);
  });

  testWidgets(
      'pending send on the chat route: failure shows Retry, and the retry '
      'succeeds', (tester) async {
    final ready =
        await pumpReadyMobileApp(tester, FlakySendClient(newMockClient()));
    final session = ready.session;
    await _openChat(tester);
    const body = 'hello from the mobile retry path';

    await tester.enterText(_composerField(), body);
    await tester.pump();
    await tester.tap(find.text(Tokens.composerSendGlyph));
    await tester.pump(const Duration(milliseconds: 100));
    // Extra frames so the stick-to-bottom jump chain settles and the pending
    // card (with its Retry button) is actually on screen.
    await tester.pump();
    await tester.pump();
    await tester.pump();

    // Failed visibly, in the timeline (not a toast): "Couldn't send" + Retry.
    expect(find.text(en.timelinePendingFailed), findsOneWidget);
    expect(find.text(en.commonRetry), findsOneWidget);
    expect(find.text(body), findsOneWidget); // body kept for the retry
    expect(session.room!.pendingMessages.single.phase, PendingPhases.failed);
    // The failure was recorded for diagnostics (Settings "Last captured error").
    expect(session.lastDiagnosticError?.context, 'message.send');

    // Retry re-sends the same body reusing the clientId.
    await tester.tap(find.text(en.commonRetry));
    await tester.pump();
    expect(find.text(en.timelinePendingSending), findsOneWidget);

    await tester.pump(const Duration(milliseconds: 100));
    expect(session.room!.pendingMessages, isEmpty);
    expect(find.text(en.timelinePendingFailed), findsNothing);
    expect(find.text(body), findsOneWidget);

    expect(ready.overflows, isEmpty,
        reason: 'the failed card and Retry must not overflow the strict '
            'phone surface:\n${ready.overflows.join('\n')}');
  });
}
