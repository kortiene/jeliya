/// Sidebar rooms-list collapse regression: the sidebar Column's only flexible
/// child was the rooms list, and its fixed rows (brand, profile card, 7-item
/// nav, rooms head, create/join rows, identity footer) total ~646dp at
/// textScale 1.0 — so any shorter viewport laid the rooms ListView out at
/// height ZERO: no room rows built while session.rooms was non-empty, plus a
/// bottom RenderFlex overflow. This reproduced at the desktop minimum
/// (960x620) and at phone landscape (moto g play 2023, 1422x640 logical).
/// [useDesktopSurface] masked it: 1440x900 at textScale 0.5.
///
/// The fix makes nav + rooms one shared CustomScrollView, so this test pins,
/// at REALISTIC textScale 1.0 on both short surfaces, that every fixture
/// room row can be scrolled into view and the sidebar no longer overflows.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/screens/sidebar.dart';
import 'package:jeliya_app/src/session/daemon_session.dart';

import 'helpers.dart';

Finder _inSidebar(Finder matching) =>
    find.descendant(of: find.byType(Sidebar), matching: matching);

// The shared [useStrictSurface] records overflow reports instead of
// swallowing them; these tests fail only on the ones attributed to the
// sidebar — this regression's signature (the fat test font overflows
// unrelated pixel-tuned rows at full scale).
Future<void> _expectRoomsReachableAt(WidgetTester tester, Size size) async {
  final overflows = useStrictSurface(tester, size);
  final session = newSession(newMockClient());
  await pumpApp(tester, session);
  await pumpSteps(tester);
  expect(session.phase, BootstrapPhase.ready,
      reason: 'bootstrap should reach the ready shell');
  expect(session.rooms.length, greaterThan(1),
      reason: 'the regression needs several rooms to be meaningful');

  // (a) Every room row renders or scrolls into view. `.hitTestable()` keeps
  // lazily pre-built but offscreen rows from satisfying the finder, so the
  // shared scrollable really has to bring each row on screen — before the
  // fix the zero-height list built NO rows and this timed out on the first.
  final scrollable = _inSidebar(find.byType(Scrollable)).first;
  for (final room in session.rooms) {
    final name = room.name!;
    await tester.scrollUntilVisible(
      _inSidebar(find.text(name)).hitTestable(),
      60,
      scrollable: scrollable,
    );
    expect(_inSidebar(find.text(name)).hitTestable(), findsOneWidget,
        reason: "room '$name' never became visible in the sidebar");
  }

  // (b) The collapse's other signature: the sidebar's own Column overflowing
  // off the BOTTOM. Horizontal 'on the right' reports are excluded — those
  // are the known fat-test-font artifacts the shared harness tolerates
  // everywhere, present at full scale with or without this fix.
  final sidebarOverflows = overflows
      .where((o) => o.contains('on the bottom') && o.contains('Sidebar'))
      .toList();
  expect(sidebarOverflows, isEmpty,
      reason: 'the sidebar must not overflow off the bottom at '
          '${size.width}x${size.height} textScale 1.0:\n'
          '${sidebarOverflows.join('\n')}');
}

void main() {
  testWidgets(
      'sidebar: every room reachable at the 960x620 desktop minimum, '
      'textScale 1.0', (tester) async {
    await _expectRoomsReachableAt(tester, const Size(960, 620));
  });

  testWidgets(
      'sidebar: every room reachable at 1422x640 phone landscape, '
      'textScale 1.0', (tester) async {
    await _expectRoomsReachableAt(tester, const Size(1422, 640));
  });
}
