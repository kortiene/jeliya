/// Mobile IA smoke (issue #17): a room tap pushes the chat route under the
/// Rooms tab (RoomHeader + Timeline + Composer), the header's Members
/// affordance pushes the room-detail route hosting the RightPanel, back pops
/// each in turn, and the bottom tabs swap surfaces (Settings, and the pinned
/// Files panel). Overflows are recorded, not swallowed — these flows assert
/// navigation, not pixel fit (the tab-bar test owns the zero-overflow bar).
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/screens/composer.dart';
import 'package:jeliya_app/src/screens/mobile_shell.dart';
import 'package:jeliya_app/src/screens/right_panel.dart';
import 'package:jeliya_app/src/screens/room_header.dart';
import 'package:jeliya_app/src/screens/settings_panel.dart';

import 'helpers.dart';

void main() {
  testWidgets(
      'rooms tab: room tap pushes chat; Members pushes room detail; '
      'back pops each', (tester) async {
    final ready = await pumpReadyMobileApp(tester, newMockClient());
    final session = ready.session;

    // The rooms list is the base route: no chat surface yet.
    expect(find.byType(RoomHeader).hitTestable(), findsNothing);

    await tester.tap(
        find.text('Product Review').hitTestable()); // fixture room name
    await pumpSteps(tester, steps: 6);

    final reviewId =
        session.rooms.firstWhere((r) => r.name == 'Product Review').roomId;
    expect(session.currentRoomId, reviewId);
    expect(find.byType(RoomHeader).hitTestable(), findsOneWidget);
    expect(find.byType(Composer).hitTestable(), findsOneWidget);

    // Members affordance → room-detail route hosting the RightPanel.
    await tester.tap(find.text(en.panelTabMembers).hitTestable().first);
    await pumpSteps(tester, steps: 6);
    expect(find.byType(RightPanel).hitTestable(), findsOneWidget);

    // Back → chat, back again → the rooms list.
    await tester.tap(find.byType(BackButton).hitTestable().first);
    await pumpSteps(tester, steps: 6);
    expect(find.byType(RoomHeader).hitTestable(), findsOneWidget);
    await tester.tap(find.byType(BackButton).hitTestable().first);
    await pumpSteps(tester, steps: 6);
    expect(find.byType(RoomHeader).hitTestable(), findsNothing);
    expect(find.text('Product Review').hitTestable(), findsOneWidget);
  });

  testWidgets('bottom tabs swap surfaces: Settings and the pinned Files panel',
      (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());
    final bar = find.byType(MobileTabBar);

    await tester.tap(find.descendant(
        of: bar,
        matching: find.widgetWithText(InkWell, en.sidebarNavSettings)));
    await pumpSteps(tester, steps: 3);
    expect(find.byType(SettingsPanel).hitTestable(), findsOneWidget);

    // Files: the room-scoped RightPanel pinned full width (a room is open).
    await tester.tap(find.descendant(
        of: bar, matching: find.widgetWithText(InkWell, en.sidebarNavFiles)));
    await pumpSteps(tester, steps: 3);
    expect(find.byType(RightPanel).hitTestable(), findsOneWidget);

    await tester.tap(find.descendant(
        of: bar, matching: find.widgetWithText(InkWell, en.sidebarNavRooms)));
    await pumpSteps(tester, steps: 3);
    expect(find.byType(RightPanel).hitTestable(), findsNothing);
  });
}
