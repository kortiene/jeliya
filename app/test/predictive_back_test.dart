/// Android predictive-back contract (manifest enableOnBackInvokedCallback,
/// Flutter 3.41.5). With the OnBackInvokedCallback API the engine keeps a
/// system back callback registered only while the LAST
/// `SystemNavigator.setFrameworkHandlesBack` call said true (WidgetsApp
/// mirrors every ascending NavigationNotification.canHandlePop —
/// widgets/app.dart `_defaultOnNavigationNotification`). After a `false`,
/// the NEXT system back never reaches Flutter at all: Android animates
/// back-to-home and the shell's back policy (non-Rooms tab → Rooms →
/// pop chat → exit) silently never runs.
///
/// The mobile shell claims EVERY back with PopScope(canPop: false), so the
/// framework must never report `false` while the shell is up. The leak this
/// test pins: the nested Rooms navigator dispatches canHandlePop:false
/// whenever its stack returns to the rooms list, and the ROOT navigator
/// forwards that verbatim — its NotificationListener only rewrites to true
/// when it can pop itself and ignores the shell route's PopScope block
/// (navigator.dart `_handleHistoryChanged` vs the listener in `build`).
/// The shell absorbs the nested navigator's notifications so the
/// route-level dispatch driven by its PopScope stays the only authority.
///
/// Widget tests cannot drive a real OS predictive gesture (that animation
/// lives in the Activity); what they CAN pin is this channel contract — the
/// exact bit the OS reads to decide who owns the next back. The gesture
/// itself needs one on-device confirmation pass.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/screens/mobile_shell.dart';
import 'package:jeliya_app/src/screens/room_header.dart';
import 'package:jeliya_app/src/screens/settings_panel.dart';

import 'helpers.dart';

/// The auto-opened mock fixture room (bootstrap opens the first active room).
// i18n-exempt: fixture room name (coincides with modalRoomNamePlaceholder)
const String _fixtureRoom = 'Build Iroh Rooms MVP';

void main() {
  testWidgets(
      'the framework never hands system back to the OS while the mobile '
      'shell is up — across chat push/pop and tab switches', (tester) async {
    // WidgetsApp forwards canHandlePop to the engine only once the app
    // lifecycle is known — deliver `resumed` the way the engine would.
    await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
      'flutter/lifecycle',
      const StringCodec().encodeMessage(AppLifecycleState.resumed.toString()),
      (_) {},
    );

    final reported = <bool>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform, (call) async {
      if (call.method == 'SystemNavigator.setFrameworkHandlesBack') {
        reported.add(call.arguments as bool);
      }
      return null;
    });
    addTearDown(() => tester.binding.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, null));

    await pumpReadyMobileApp(tester, newMockClient());

    // Boot screens legitimately report false (back = leave the app); the
    // contract starts the moment the shell — whose PopScope claims every
    // back — is on screen.
    expect(reported, isNotEmpty,
        reason: 'lifecycle is resumed, so WidgetsApp must be reporting');
    expect(reported.last, isTrue,
        reason: 'the ready shell must have claimed system backs (a trailing '
            'false here means the nested rooms navigator clobbered it)');
    reported.clear();

    // Push the chat route, then pop it with a system back: the nested
    // navigator falls back to a single route — the exact moment its
    // canHandlePop:false dispatch would leak past the shell.
    await tester.tap(find.text(_fixtureRoom).hitTestable());
    await pumpSteps(tester, steps: 6);
    expect(find.byType(RoomHeader).hitTestable(), findsOneWidget);
    expect(reported, isNot(contains(false)));

    await tester.binding.handlePopRoute();
    await pumpSteps(tester, steps: 6);
    expect(find.byType(RoomHeader).hitTestable(), findsNothing);
    expect(reported, isNot(contains(false)),
        reason: 'popping chat to the rooms list must not hand the NEXT back '
            'to the OS — from another tab it would exit instead of '
            'returning to Rooms');

    // And the policy must still receive that next back: from Settings a
    // system back returns to Rooms (it would background the app if the OS
    // had taken the gesture).
    await tester.tap(find.descendant(
        of: find.byType(MobileTabBar),
        matching: find.widgetWithText(InkWell, en.sidebarNavSettings)));
    await pumpSteps(tester, steps: 6);
    expect(find.byType(SettingsPanel).hitTestable(), findsOneWidget);

    await tester.binding.handlePopRoute();
    await pumpSteps(tester, steps: 6);
    expect(find.byType(SettingsPanel).hitTestable(), findsNothing,
        reason: 'back from a non-Rooms tab must return to Rooms');
    expect(reported, isNot(contains(false)));
  });
}
