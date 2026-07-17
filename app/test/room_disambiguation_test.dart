/// Homonymous-room disambiguation (issue #49; docs/room-workbench.md,
/// decision 6). `room_id` is identity and `name` is a non-unique local label,
/// so any surface where acting on the wrong room matters shows the short id.
///
/// Covered here:
///  - the pure [homonymousRoomIds] fold: named collisions, the untitled
///    placeholder (two unsynced rooms are homonyms), case/trim folding, and
///    the fleet's repeated-id dedup;
///  - the rooms list shows the short id for BOTH same-named rooms — in the
///    desktop rail AND the compact list, in the row's accessible name — at a
///    strict phone surface in English AND French with zero overflow;
///  - the Leave Room confirmation ALWAYS repeats the short id (homonym or not);
///  - the Create Room dialog WARNS on a local homonym without disabling Create,
///    and clears the warning when the name no longer collides.
///
/// Copy is asserted through the shared `en`/`fr` catalog instances
/// (test/helpers.dart, docs/i18n.md rule 6). Room names are fixture data.
library;

import 'dart:async';

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/screens/mobile_rooms.dart';
import 'package:jeliya_app/src/screens/modals/create_room.dart';
import 'package:jeliya_app/src/screens/modals/leave_room.dart';
import 'package:jeliya_app/src/l10n/strings_context.dart';
import 'package:jeliya_app/src/screens/sidebar.dart';
import 'package:jeliya_app/src/session/daemon_session.dart';
import 'package:jeliya_app/src/session/room_homonyms.dart';
import 'package:jeliya_app/src/widgets/buttons.dart';
import 'package:jeliya_app/src/widgets/modal_scaffold.dart';
import 'package:jeliya_app/src/widgets/room_short_id.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart' show JeliyaMethods, shortId;

import 'helpers.dart';

// i18n-exempt: fixture room name reused across the file, not catalog copy.
const _review = 'Product Review';

/// Create a second room whose name collides with an existing one, then refresh
/// the session so both appear in the room list (room.create allows duplicate
/// names — decision 6). The mock delays every call (~60ms), so the create and
/// the follow-up room.list are kicked off UNAWAITED and the fake clock is
/// advanced by pumping — awaiting them directly would deadlock (no pump runs
/// during the await).
Future<void> _seedHomonym(
    WidgetTester tester, DaemonSession session, String name) async {
  unawaited(
      session.client!.roomCreate(name).then((_) => session.refreshRooms()));
  await pumpSteps(tester, steps: 8);
}

/// The TextButton inside the Create dialog's primary [JeliyaButton], for the
/// enabled/disabled assertion.
TextButton _createSubmit(WidgetTester tester) =>
    tester.widget<TextButton>(find.descendant(
      of: find.descendant(
          of: find.byType(CreateRoomModal),
          matching: find.widgetWithText(JeliyaButton, en.modalCreateRoom)),
      matching: find.byType(TextButton),
    ));

/// A homonymous row's short id must be in the row's ACCESSIBLE name, not only a
/// visual mono span. The row is a button that merges its descendants, so the
/// disambiguator label is a substring of the merged node label.
Finder _accessibleShortId(AppStrings s, String roomId) =>
    find.bySemanticsLabel(RegExp(RegExp.escape(s.roomShortIdLabel(shortId(roomId)))));

void main() {
  group('homonymousRoomIds', () {
    ({String roomId, String? name}) room(String id, String? name) =>
        (roomId: id, name: name);
    final untitled = en.shellUntitledRoom;

    test('same displayed name → homonyms; a unique name is not', () {
      final set = homonymousRoomIds(
        [
          room('a', _review),
          room('b', _review),
          room('c', 'Design System'), // i18n-exempt: fixture room name
        ],
        untitledLabel: untitled,
      );
      expect(set, {'a', 'b'});
    });

    test('two null-named rooms are homonyms via the untitled placeholder', () {
      final set = homonymousRoomIds(
        [
          room('a', null),
          room('b', null),
          room('c', 'Named'), // i18n-exempt: fixture room name
        ],
        untitledLabel: untitled,
      );
      expect(set, {'a', 'b'},
          reason: 'two unsynced rooms both render the placeholder');
    });

    test('a room literally named the placeholder is a homonym of a null room',
        () {
      final set = homonymousRoomIds(
        [
          room('a', null),
          room('b', en.shellUntitledRoom),
        ],
        untitledLabel: untitled,
      );
      expect(set, {'a', 'b'});
    });

    test('folds case and trims surrounding whitespace', () {
      final set = homonymousRoomIds(
        [
          room('a', 'Ops'), // i18n-exempt: fixture room name
          room('b', '  ops  '), // i18n-exempt: fixture room name
          room('c', 'OPS'), // i18n-exempt: fixture room name
        ],
        untitledLabel: untitled,
      );
      expect(set, {'a', 'b', 'c'});
    });

    test('a single room, and an empty list, yield no homonyms', () {
      expect(
          homonymousRoomIds([room('a', 'Solo')], untitledLabel: untitled), // i18n-exempt: fixture room name
          isEmpty);
      expect(homonymousRoomIds(const [], untitledLabel: untitled), isEmpty);
    });

    test('a repeated room id under one name collapses (fleet dedup)', () {
      final set = homonymousRoomIds(
        [
          room('a', 'Shared'), // i18n-exempt: fixture room name
          room('a', 'Shared'), // i18n-exempt: fixture room name
        ],
        untitledLabel: untitled,
      );
      expect(set, isEmpty,
          reason: 'the same room referenced twice is not its own homonym');
    });
  });

  testWidgets(
      'desktop rail: both same-named rooms show the short id, in the row '
      'accessible name', (tester) async {
    final session = await pumpReadyApp(tester, newMockClient());
    await _seedHomonym(tester, session, _review);

    final inRail = find.descendant(
        of: find.byType(Sidebar), matching: find.byType(RoomShortId));
    expect(inRail, findsNWidgets(2),
        reason: 'both Product Review rooms carry the disambiguator; '
            'the unique-named rooms do not');

    final reviews = session.rooms.where((r) => r.name == _review).toList();
    expect(reviews, hasLength(2));
    for (final r in reviews) {
      expect(_accessibleShortId(en, r.roomId), findsOneWidget,
          reason: 'the short id belongs in the row accessible name');
    }
  });

  for (final french in const [false, true]) {
    final lang = french ? 'fr' : 'en';
    testWidgets(
        'compact list ($lang): both same-named rooms show the short id, '
        'accessible, zero overflow', (tester) async {
      final ready = await pumpReadyMobileApp(tester, newMockClient());
      final session = ready.session;
      await _seedHomonym(tester, session, _review);
      // Reach the rooms list the way a user does (EN back label) BEFORE the
      // French flip, mirroring the rooms-screen regression test.
      await mobileShowRoomsList(tester);
      if (french) {
        session.prefs.textLocale = 'fr';
        await pumpSteps(tester, steps: 3);
      }
      final s = french ? fr : en;

      final screen = find.byType(MobileRoomsScreen);
      expect(screen, findsOneWidget);
      expect(
          find.descendant(of: screen, matching: find.byType(RoomShortId)),
          findsNWidgets(2),
          reason: 'both Product Review rows show the short id');

      final reviews = session.rooms.where((r) => r.name == _review).toList();
      expect(reviews, hasLength(2));
      for (final r in reviews) {
        expect(_accessibleShortId(s, r.roomId), findsOneWidget,
            reason: 'the short id belongs in the row accessible name ($lang)');
      }

      expect(ready.overflows, isEmpty,
          reason: 'zero overflows expected ($lang):\n'
              '${ready.overflows.join('\n')}');
    });
  }

  testWidgets('leave modal always repeats the room short id (mono, accessible)',
      (tester) async {
    final session = await pumpReadyApp(tester, newMockClient());
    // Any room — the short id is shown ALWAYS, homonym or not, because leaving
    // the wrong room publishes an irreversible signed departure.
    final target = session.rooms.first;
    final ctx = tester.element(find.byType(Sidebar));
    unawaited(showJeliyaModal<bool>(ctx,
        builder: (_) =>
            LeaveRoomModal(roomId: target.roomId, roomName: target.name)));
    await pumpSteps(tester, steps: 3);

    final modal = find.byType(LeaveRoomModal);
    expect(modal, findsOneWidget);
    expect(find.descendant(of: modal, matching: find.byType(RoomShortId)),
        findsOneWidget);
    expect(
        find.descendant(of: modal, matching: find.text(shortId(target.roomId))),
        findsOneWidget,
        reason: 'the truncated room id renders as a mono span');
    expect(_accessibleShortId(en, target.roomId), findsOneWidget,
        reason: 'the short id is announced to assistive tech');
  });

  testWidgets('create dialog warns on a local homonym without disabling Create',
      (tester) async {
    await pumpReadyApp(tester, newMockClient());
    final ctx = tester.element(find.byType(Sidebar));
    unawaited(showJeliyaModal<String>(ctx,
        builder: (_) => const CreateRoomModal()));
    await pumpSteps(tester, steps: 3);
    expect(find.byType(CreateRoomModal), findsOneWidget);

    final field = find.widgetWithText(TextField, en.modalRoomNamePlaceholder);

    // A name that collides with an existing local room: warn, stay enabled.
    await tester.enterText(field, _review);
    await tester.pump();
    expect(find.text(en.modalCreateRoomHomonymWarning), findsOneWidget);
    expect(_createSubmit(tester).onPressed, isNotNull,
        reason: 'a homonym never blocks creation');

    // Case-folded / whitespace-padded collisions still warn.
    await tester.enterText(field, '  product review  '); // i18n-exempt: fixture room name, folded
    await tester.pump();
    expect(find.text(en.modalCreateRoomHomonymWarning), findsOneWidget);

    // A unique name clears the warning; Create stays enabled throughout.
    await tester.enterText(field, 'A room with no twin'); // i18n-exempt: fixture room name
    await tester.pump();
    expect(find.text(en.modalCreateRoomHomonymWarning), findsNothing);
    expect(_createSubmit(tester).onPressed, isNotNull);

    // French: the warning renders from the fr catalog.
    await tester.enterText(field, _review);
    await tester.pump();
    final session = SessionScope.of(ctx);
    session.prefs.textLocale = 'fr';
    await pumpSteps(tester, steps: 3);
    expect(find.text(fr.modalCreateRoomHomonymWarning), findsOneWidget);
  });
}
