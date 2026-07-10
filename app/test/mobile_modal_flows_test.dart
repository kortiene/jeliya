/// Mobile flow pop contracts (issue #17): below the breakpoint, create room
/// and leave room stay dialogs while join-with-ticket and invite present
/// full screen — and the awaited Navigator.pop results keep driving the EXACT
/// shell effects: create/join pop a roomId consumed by refreshRooms +
/// openRoom, leave pops true consumed by leaveCurrentRoom. The invite screen
/// keeps observing the LIVE RoomStore so a re-open's new endpointAddr updates
/// the combined invite while it is up. Overflows are recorded, not swallowed
/// — these tests assert contracts, not pixel fit (mobile_flow_layout_test
/// owns the zero-overflow bar for these surfaces).
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/l10n/tokens.dart';
import 'package:jeliya_app/src/screens/modals/create_room.dart';
import 'package:jeliya_app/src/screens/modals/invite.dart';
import 'package:jeliya_app/src/screens/modals/join_room.dart';
import 'package:jeliya_app/src/screens/modals/leave_room.dart';
import 'package:jeliya_app/src/screens/right_panel.dart';
import 'package:jeliya_app/src/screens/room_header.dart';
import 'package:jeliya_app/src/widgets/buttons.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart'
    show JeliyaMethods, Roles;

import 'helpers.dart';
import 'member_self_seam.dart';

/// Records every wire method in call order so a test can prove WHICH session
/// effects a popped modal result triggered: `room.list` = refreshRooms,
/// `room.open` = openRoom, `daemon.status` = refreshStatus.
class _RecordingClient extends DelegatingClient {
  _RecordingClient(super.inner);

  final List<String> calls = [];

  @override
  Future<dynamic> call(String method, [Map<String, dynamic>? params]) {
    calls.add(method);
    return inner.call(method, params);
  }
}

/// The leave flow needs both seams: self as a plain member (so Leave renders)
/// AND the call recording.
class _RecordingMemberSelfClient extends MemberSelfClient {
  _RecordingMemberSelfClient(super.inner);

  final List<String> calls = [];

  @override
  Future<dynamic> call(String method, [Map<String, dynamic>? params]) {
    calls.add(method);
    return super.call(method, params);
  }
}

/// Rewrites the `room.open` endpoint addr once [addrOverride] is set — the
/// test's stand-in for a reconnect re-open landing on a new dialable address.
class _ReboundAddrClient extends DelegatingClient {
  _ReboundAddrClient(super.inner);

  String? addrOverride;

  @override
  Future<dynamic> call(String method, [Map<String, dynamic>? params]) async {
    final result = await inner.call(method, params);
    final addr = addrOverride;
    // i18n-exempt: wire method name, not copy
    if (addr != null && method == 'room.open') {
      final map = Map<String, dynamic>.of(result as Map<String, dynamic>);
      map['endpoint'] = <String, dynamic>{
        ...(map['endpoint'] as Map).cast<String, dynamic>(),
        'addr': addr,
      };
      return map;
    }
    return result;
  }
}

/// The calls recorded from [mark] on, split at the first occurrence of
/// [method] (which must exist): everything AFTER that call began.
List<String> _callsAfter(List<String> calls, int mark, String method) {
  final tail = calls.sublist(mark);
  final ix = tail.indexOf(method);
  expect(ix, isNonNegative, reason: '$method should have been called');
  return tail.sublist(ix + 1);
}

void main() {
  testWidgets(
      'create room stays a dialog; the popped roomId drives '
      'refreshRooms + openRoom and lands in the new chat', (tester) async {
    final client = _RecordingClient(newMockClient());
    final ready = await pumpReadyMobileApp(tester, client);
    final session = ready.session;
    final roomsBefore = session.rooms.length;

    await tester.tap(find.text(en.modalCreateRoom).hitTestable());
    await pumpSteps(tester, steps: 3);
    expect(find.byType(CreateRoomModal), findsOneWidget);
    expect(find.byType(Dialog), findsOneWidget); // a dialog, even on phones

    await tester.enterText(
        find.widgetWithText(TextField, en.modalRoomNamePlaceholder),
        'Polish Lane');
    await tester.pump();
    final mark = client.calls.length;
    await tester
        .tap(find.widgetWithText(JeliyaButton, en.modalCreateRoom).hitTestable());
    await pumpSteps(tester);

    expect(find.byType(CreateRoomModal), findsNothing);
    expect(session.rooms, hasLength(roomsBefore + 1)); // refreshRooms ran
    final created = session.rooms.firstWhere((r) => r.name == 'Polish Lane');
    expect(session.currentRoomId, created.roomId); // openRoom ran
    // i18n-exempt: wire method names, not copy
    final after = _callsAfter(client.calls, mark, 'room.create');
    expect(after, contains('room.list'));
    expect(after, contains('room.open'));
    // Web parity: create lands in the new room's chat (pushed route).
    expect(find.byType(RoomHeader).hitTestable(), findsOneWidget);
  });

  testWidgets(
      'join with a ticket presents full screen; the popped roomId drives '
      'refreshRooms + openRoom', (tester) async {
    final client = _RecordingClient(newMockClient());
    final ready = await pumpReadyMobileApp(tester, client);
    final session = ready.session;
    final review =
        session.rooms.firstWhere((r) => r.name == 'Product Review');
    // Mint a redeemable ticket on the same mock daemon.
    final minted = client.inviteCreate(
        roomId: review.roomId, identityId: 'b' * 64, role: Roles.member);
    await pumpSteps(tester, steps: 2);
    final ticket = await minted;

    await tester.tap(find.text(en.modalJoinRoomTitle).hitTestable());
    await pumpSteps(tester, steps: 3);
    expect(find.byType(JoinRoomModal), findsOneWidget);
    expect(find.byType(Dialog), findsNothing); // full screen, not a dialog
    expect(
        find.descendant(
            of: find.byType(JoinRoomModal), matching: find.byType(Scaffold)),
        findsOneWidget);

    await tester.enterText(
        find.widgetWithText(TextField, en.modalTicketPlaceholder), ticket);
    await tester.pump();
    final mark = client.calls.length;
    await tester
        .tap(find.widgetWithText(JeliyaButton, en.modalJoinRoom).hitTestable());
    await pumpSteps(tester);

    expect(find.byType(JoinRoomModal), findsNothing);
    expect(session.currentRoomId, review.roomId); // openRoom ran
    // i18n-exempt: wire method names, not copy
    final after = _callsAfter(client.calls, mark, 'room.join');
    expect(after, contains('room.list'));
    expect(after, contains('room.open'));
    expect(find.byType(RoomHeader).hitTestable(), findsOneWidget);
  });

  testWidgets(
      'leave room stays a dialog; the popped true drives leaveCurrentRoom '
      'and lands back on the rooms list', (tester) async {
    final client = _RecordingMemberSelfClient(newMockClient());
    final ready = await pumpReadyMobileApp(tester, client);
    final session = ready.session;
    // Present self as a plain member of Product Review so Leave is offered.
    client.memberRoomId =
        session.rooms.firstWhere((r) => r.name == 'Product Review').roomId;

    await tester.tap(find.text('Product Review').hitTestable());
    await pumpSteps(tester, steps: 6);
    await tester.tap(find.text(en.panelTabMembers).hitTestable().first);
    await pumpSteps(tester, steps: 6);
    expect(find.byType(RightPanel).hitTestable(), findsOneWidget);

    await tester.tap(
        find.widgetWithText(JeliyaButton, en.panelLeave).hitTestable());
    await pumpSteps(tester, steps: 3);
    expect(find.byType(LeaveRoomModal), findsOneWidget);
    expect(find.byType(Dialog), findsOneWidget); // destructive confirm dialog

    final mark = client.calls.length;
    await tester
        .tap(find.widgetWithText(JeliyaButton, en.modalLeaveRoom).hitTestable());
    await pumpSteps(tester);

    expect(find.byType(LeaveRoomModal), findsNothing);
    expect(session.currentRoomId, isNull); // leaveCurrentRoom ran
    expect(session.prefs.lastRoomId, isNull);
    // i18n-exempt: wire method names, not copy
    final after = _callsAfter(client.calls, mark, 'room.leave');
    expect(after, contains('room.list'));
    expect(after, contains('daemon.status'));
    // Back on the rooms list, with the departed room receded.
    expect(find.byType(RoomHeader).hitTestable(), findsNothing);
    expect(find.text(en.sidebarStateLeft).hitTestable(), findsOneWidget);
  });

  testWidgets(
      'invite presents full screen: role/expiry stack and the combined '
      'invite follows the LIVE endpointAddr', (tester) async {
    final client = _ReboundAddrClient(newMockClient());
    final ready = await pumpReadyMobileApp(tester, client);
    final session = ready.session;
    final review =
        session.rooms.firstWhere((r) => r.name == 'Product Review');
    await tester.tap(find.text('Product Review').hitTestable());
    await pumpSteps(tester, steps: 6);

    // The chat header scrolls internally on short viewports — bring its
    // primary Invite action into view first.
    final invite =
        find.text('${Tokens.roomHeaderInviteGlyph} ${en.roomHeaderInvite}');
    await tester.scrollUntilVisible(invite, 60,
        scrollable: find
            .ancestor(
                of: find.byType(RoomHeader), matching: find.byType(Scrollable))
            .first);
    await tester.tap(invite.hitTestable());
    await pumpSteps(tester, steps: 3);
    expect(find.byType(InviteModal), findsOneWidget);
    expect(find.byType(Dialog), findsNothing); // full screen, not a dialog

    // Stacked, not the desktop two-column Row: role and expiry share the
    // left edge, expiry below role.
    final role = find.byType(DropdownButtonFormField<String>);
    final expiry = find.byWidgetPredicate(
        (w) => w is TextField && w.keyboardType == TextInputType.number);
    expect(tester.getTopLeft(role).dx, tester.getTopLeft(expiry).dx);
    expect(tester.getBottomLeft(role).dy,
        lessThanOrEqualTo(tester.getTopLeft(expiry).dy));

    await tester.enterText(
        find.widgetWithText(TextField, en.inviteInviteePlaceholder), 'c' * 64);
    await tester.pump();
    await tester.tap(
        find.widgetWithText(JeliyaButton, en.inviteGenerateTicket).hitTestable());
    await pumpSteps(tester, steps: 3);

    // Combined result: the 4-row read-only box holds 'ticket#address'.
    String combined() => tester
        .widget<TextField>(find
            .byWidgetPredicate(
                (w) => w is TextField && w.readOnly && w.maxLines == 4)
            .first)
        .controller!
        .text;
    final addrBefore = session.room!.endpointAddr!;
    expect(combined(), startsWith('roomtkt1'));
    expect(combined(), endsWith('#$addrBefore'));

    // A reconnect re-open lands on a new dialable address while the screen
    // is up — the combined invite must follow it, not freeze at open time.
    client.addrOverride = 'rebound@203.0.113.7:4242';
    final reopened = session.openRoom(review.roomId);
    await pumpSteps(tester, steps: 6);
    await reopened;
    expect(find.byType(InviteModal), findsOneWidget);
    expect(combined(), endsWith('#rebound@203.0.113.7:4242'));
  });
}
