/// Mobile system-back policy (issue #17 review): a back press must always
/// change what is VISIBLE. From a non-Rooms tab it returns to the Rooms tab
/// with its stack UNTOUCHED (never silently popping a hidden chat route);
/// on Rooms it pops the pushed routes; with nothing left it asks the
/// platform to exit. Room-detail routes never stack: re-entering Members
/// from a pinned surface replaces the previous detail route, so one back
/// always leaves it.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/screens/mobile_rooms.dart';
import 'package:jeliya_app/src/screens/mobile_shell.dart';
import 'package:jeliya_app/src/screens/room_header.dart';
import 'package:jeliya_app/src/screens/settings_panel.dart';

import 'helpers.dart';

/// The auto-opened mock fixture room (bootstrap opens the first active room).
// i18n-exempt: fixture room name (coincides with modalRoomNamePlaceholder)
const String _fixtureRoom = 'Build Iroh Rooms MVP';

Future<void> _tapTab(WidgetTester tester, String label) async {
  await tester.tap(find.descendant(
      of: find.byType(MobileTabBar),
      matching: find.widgetWithText(InkWell, label)));
  await pumpSteps(tester, steps: 6);
}

Future<void> _openChat(WidgetTester tester) async {
  await tester.tap(find.text(_fixtureRoom).hitTestable());
  await pumpSteps(tester, steps: 6);
  expect(find.byType(RoomHeader).hitTestable(), findsOneWidget);
}

/// The rooms LIST (not the chat/detail routes) is the visible Rooms surface.
Finder _visibleRoomsList() => find
    .descendant(
        of: find.byType(MobileRoomsScreen), matching: find.text(_fixtureRoom))
    .hitTestable();

/// System back, as Android's back gesture dispatches it.
Future<void> _systemBack(WidgetTester tester) async {
  await tester.binding.handlePopRoute();
  await pumpSteps(tester, steps: 6);
}

void main() {
  testWidgets(
      'system back from Settings reveals the Rooms stack untouched — the '
      'chat route survives, then pops, then the app exits', (tester) async {
    // Capture the exit request instead of letting it hit the platform.
    final platformCalls = <String>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform, (call) async {
      platformCalls.add(call.method);
      return null;
    });
    addTearDown(() => tester.binding.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, null));

    await pumpReadyMobileApp(tester, newMockClient());
    await _openChat(tester);

    // Switch to Settings; the chat route stays mounted under Rooms.
    await _tapTab(tester, en.sidebarNavSettings);
    expect(find.byType(SettingsPanel).hitTestable(), findsOneWidget);

    // Back #1: return to the Rooms tab — the chat route must still be
    // there (popping it while hidden would be a visual no-op that also
    // destroys the surface the user expects to come back to).
    await _systemBack(tester);
    expect(find.byType(SettingsPanel).hitTestable(), findsNothing,
        reason: 'back on a non-Rooms tab must leave that tab');
    expect(find.byType(RoomHeader).hitTestable(), findsOneWidget,
        reason: 'the hidden chat route must survive the tab round trip');

    // Back #2: pop the (now visible) chat route to the rooms list.
    await _systemBack(tester);
    expect(find.byType(RoomHeader).hitTestable(), findsNothing);
    expect(_visibleRoomsList(), findsOneWidget);

    // Back #3: nothing left to close — the shell asks the platform to exit.
    expect(platformCalls, isNot(contains('SystemNavigator.pop')));
    await _systemBack(tester);
    expect(platformCalls, contains('SystemNavigator.pop'),
        reason: 'an unstacked Rooms tab must hand back to the platform');
  });

  testWidgets(
      'system back from Pipes with no chat route open lands on the rooms '
      'list', (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());

    await _tapTab(tester, en.sidebarNavPipes);
    expect(_visibleRoomsList(), findsNothing);

    await _systemBack(tester);
    expect(_visibleRoomsList(), findsOneWidget,
        reason: 'back from a non-Rooms tab must return to Rooms');
  });

  testWidgets(
      're-entering Members from a pinned surface replaces the detail route '
      '— one back always leaves it', (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());

    // Pinned Files → Members pushes the room-detail route on the Rooms tab.
    await _tapTab(tester, en.sidebarNavFiles);
    await tester.tap(
        find.widgetWithText(InkWell, en.panelTabMembers).hitTestable().first);
    await pumpSteps(tester, steps: 6);
    expect(find.byType(BackButton).hitTestable(), findsOneWidget);

    // Back to pinned Files, then Members AGAIN: the previous detail route
    // must be replaced, never stacked under the new one.
    await _tapTab(tester, en.sidebarNavFiles);
    await tester.tap(
        find.widgetWithText(InkWell, en.panelTabMembers).hitTestable().first);
    await pumpSteps(tester, steps: 6);
    expect(find.byType(BackButton).hitTestable(), findsOneWidget);

    await tester.tap(find.byType(BackButton).hitTestable());
    await pumpSteps(tester, steps: 6);
    expect(find.byType(BackButton).hitTestable(), findsNothing,
        reason: 'a single back press must leave the detail route');
    expect(_visibleRoomsList(), findsOneWidget);
  });
}
