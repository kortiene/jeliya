/// Mobile tab bar (issue #17): five tabs — Rooms / Agents / Pipes / Files /
/// Settings — each hit-testable at the 44dp touch floor, at 360x800 AND the
/// shorter 360x640, in English AND French (the #14 lesson: fr copy runs ~2x
/// wider), with ZERO recorded overflows anywhere in the mobile tree. Labels
/// are asserted via the shared catalog instances, never literals
/// (docs/i18n.md rule 6). Strict surface: textScale 1.0, DPR 1.0.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/l10n/strings_context.dart';
import 'package:jeliya_app/src/screens/mobile_shell.dart';

import 'helpers.dart';

List<String> _tabLabels(AppStrings s) => [
      s.sidebarNavRooms,
      s.sidebarNavAgents,
      s.sidebarNavPipes,
      s.sidebarNavFiles,
      s.sidebarNavSettings,
    ];

Future<void> _expectTabBarAt(WidgetTester tester, Size size,
    {required bool french}) async {
  final ready = await pumpReadyMobileApp(tester, newMockClient(), size: size);
  if (french) {
    // The live-switch idiom (panel_fr_layout_test): flip the pref, repump.
    ready.session.prefs.textLocale = 'fr';
    await pumpSteps(tester, steps: 3);
  }
  final s = french ? fr : en;

  final bar = find.byType(MobileTabBar);
  expect(bar, findsOneWidget);

  for (final label in _tabLabels(s)) {
    final tab =
        find.descendant(of: bar, matching: find.widgetWithText(InkWell, label));
    expect(tab.hitTestable(), findsOneWidget,
        reason: "tab '$label' must render and be hit-testable");
    final tabSize = tester.getSize(tab);
    expect(tabSize.width, greaterThanOrEqualTo(44),
        reason: "tab '$label' is narrower than the 44dp touch floor");
    expect(tabSize.height, greaterThanOrEqualTo(44),
        reason: "tab '$label' is shorter than the 44dp touch floor");
  }

  // The whole mobile tree — including the offstage Pipes/Files/Settings
  // surfaces the IndexedStack keeps laid out — must not overflow.
  expect(ready.overflows, isEmpty,
      reason: 'zero overflows expected at ${size.width}x${size.height} '
          '(${french ? 'fr' : 'en'}):\n${ready.overflows.join('\n')}');
}

void main() {
  for (final size in const [Size(360, 800), Size(360, 640)]) {
    testWidgets(
        'tab bar: five tabs at ${size.width.toInt()}x${size.height.toInt()}, '
        'en, 44dp targets, zero overflows', (tester) async {
      await _expectTabBarAt(tester, size, french: false);
    });

    testWidgets(
        'tab bar: five tabs at ${size.width.toInt()}x${size.height.toInt()}, '
        'fr, 44dp targets, zero overflows', (tester) async {
      await _expectTabBarAt(tester, size, french: true);
    });
  }
}
