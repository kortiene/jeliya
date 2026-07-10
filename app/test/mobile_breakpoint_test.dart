/// Breakpoint routing (issue #17): the shell forks on WINDOW WIDTH alone —
/// below 900dp the bottom-tab [MobileShell] mounts (no Sidebar, no
/// right-panel rail); at the proven 960x620 desktop minimum the three-pane
/// shell mounts with no tab bar; and a live resize re-routes on the next
/// build (the fork is build-time reactive, not init-time).
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/screens/mobile_shell.dart';
import 'package:jeliya_app/src/screens/right_panel.dart';
import 'package:jeliya_app/src/screens/sidebar.dart';

import 'helpers.dart';

void main() {
  testWidgets('360x800 mounts the mobile shell — no sidebar, no panel rail',
      (tester) async {
    final ready = await pumpReadyMobileApp(tester, newMockClient());

    expect(find.byType(MobileShell), findsOneWidget);
    expect(find.byType(MobileTabBar), findsOneWidget);
    expect(find.byType(Sidebar), findsNothing);
    // The pinned Pipes/Files surfaces exist offstage in the IndexedStack;
    // no RightPanel is VISIBLE while the Rooms tab is active (default
    // finders skip offstage).
    expect(find.byType(RightPanel), findsNothing);
    expect(ready.session.currentRoomId, isNotNull);
  });

  testWidgets('960x620 mounts the three-pane shell — no tab bar',
      (tester) async {
    useDesktopSurface(tester, size: const Size(960, 620));
    final session = newSession(newMockClient());
    await pumpApp(tester, session);
    await pumpSteps(tester);

    expect(find.byType(Sidebar), findsOneWidget);
    expect(find.byType(RightPanel), findsOneWidget);
    expect(find.byType(MobileShell), findsNothing);
    expect(find.byType(MobileTabBar), findsNothing);
  });

  testWidgets('live resize 960x620 -> 360x800 -> back re-routes the shell',
      (tester) async {
    useDesktopSurface(tester, size: const Size(960, 620));
    final session = newSession(newMockClient());
    await pumpApp(tester, session);
    await pumpSteps(tester);
    expect(find.byType(Sidebar), findsOneWidget);
    expect(find.byType(MobileShell), findsNothing);

    tester.view.physicalSize = const Size(360, 800);
    await pumpSteps(tester, steps: 3);
    expect(find.byType(MobileShell), findsOneWidget);
    expect(find.byType(MobileTabBar), findsOneWidget);
    expect(find.byType(Sidebar), findsNothing);

    tester.view.physicalSize = const Size(960, 620);
    await pumpSteps(tester, steps: 3);
    expect(find.byType(Sidebar), findsOneWidget);
    expect(find.byType(RightPanel), findsOneWidget);
    expect(find.byType(MobileShell), findsNothing);
  });
}
