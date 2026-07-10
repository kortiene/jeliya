/// Mobile Settings tab (issue #17 polish): at 360dp the daemon-detail label
/// column gives every TRANSLATED label at least its widest word — the fr
/// labels (« Superviseur », « Dossier de données ») outgrow the reference
/// 96px column (the #14 lesson) — and both language pickers stay usable
/// end-to-end (scroll to, open, pick, live-switch). Strict surface: 360x800
/// AND 360x640, en AND fr, textScale 1.0, DPR 1.0, zero recorded overflows;
/// copy asserted via the shared catalog instances.
library;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/l10n/strings_context.dart';
import 'package:jeliya_app/src/l10n/tokens.dart';
import 'package:jeliya_app/src/screens/mobile_shell.dart';
import 'package:jeliya_app/src/screens/settings_panel.dart';
import 'package:jeliya_app/src/theme.dart';

import 'helpers.dart';

List<String> _detailLabels(AppStrings s) => [
      s.settingsVersionLabel,
      s.settingsProtocolLabel,
      s.settingsPidLabel,
      s.settingsPortLabel,
      s.settingsDataDirLabel,
      s.settingsTransportLabel,
      s.settingsSupervisorLabel,
    ];

/// Deterministically reveals [target] inside [scrollable]: jumpTo in fixed
/// steps until the lazy list builds it, then ensureVisible to bring it fully
/// on-screen. scrollUntilVisible is unusable here — tester.drag's one-frame
/// gesture reads as a fling, and the ballistic overshoot skips right past
/// targets on these long fat-font lists.
Future<void> _reveal(
    WidgetTester tester, Finder scrollable, Finder target) async {
  final position = tester.state<ScrollableState>(scrollable).position;
  while (target.evaluate().isEmpty &&
      position.pixels < position.maxScrollExtent) {
    position.jumpTo(
        (position.pixels + 200).clamp(0.0, position.maxScrollExtent));
    await tester.pump();
  }
  await Scrollable.ensureVisible(target.evaluate().single);
  await tester.pump();
}

Future<void> _expectSettingsAt(WidgetTester tester, Size size,
    {required bool french}) async {
  final ready = await pumpReadyMobileApp(tester, newMockClient(), size: size);
  if (french) {
    // The live-switch idiom (panel_fr_layout_test): flip the pref, repump.
    ready.session.prefs.textLocale = 'fr';
    await pumpSteps(tester, steps: 3);
  }
  final s = french ? fr : en;

  await tester.tap(find.descendant(
      of: find.byType(MobileTabBar),
      matching: find.widgetWithText(InkWell, s.sidebarNavSettings)));
  await pumpSteps(tester, steps: 6);
  expect(find.byType(SettingsPanel).hitTestable(), findsOneWidget);

  final scrollable = find
      .descendant(
          of: find.byType(SettingsPanel), matching: find.byType(Scrollable))
      .first;

  // Every daemon-detail label gets at least its widest word: the shared
  // label column grows with the translation instead of clipping it.
  final scaler = TextScaler.linear(tester.platformDispatcher.textScaleFactor);
  for (final label in _detailLabels(s)) {
    final rendered = label.toUpperCase();
    final finder = find.descendant(
        of: find.byType(SettingsPanel), matching: find.text(rendered));
    await _reveal(tester, scrollable, finder);
    final painter = TextPainter(
      text: TextSpan(text: rendered, style: JeliyaText.microLabel),
      textDirection: TextDirection.ltr,
      textScaler: scaler,
    )..layout();
    expect(tester.getSize(finder).width,
        greaterThanOrEqualTo(painter.minIntrinsicWidth - 0.5),
        reason: "detail label '$rendered' gets less than its widest word — "
            'it clips');
    painter.dispose();
  }

  final target = french ? 'en' : 'fr';
  final after = french ? en : fr;

  // Text-locale picker: usable end-to-end at this width — reveal the card
  // (the picker sits right under its label), open, pick the other language;
  // the window re-renders live (no restart). The picker displays the
  // CURRENT selection: System default in the en run, Français in the fr run
  // (its pref was flipped at test start).
  await _reveal(
      tester,
      scrollable,
      find.descendant(
          of: find.byType(SettingsPanel),
          matching: find.text(s.settingsLanguageLabel.toUpperCase())));
  final textPickerValue =
      french ? Tokens.langName('fr')! : s.settingsLocaleSystemDefault;
  await tester.tap(find.text(textPickerValue).hitTestable().first);
  await pumpSteps(tester, steps: 3);
  await tester.tap(find.text(Tokens.langName(target)!).hitTestable().last);
  await pumpSteps(tester, steps: 3);
  expect(ready.session.prefs.textLocale, target);
  expect(find.text(after.sidebarNavSettings), findsWidgets,
      reason: 'the window re-renders live in the picked language');

  // Formatting picker: moves its own pref (decision 4), same touch flow.
  await _reveal(
      tester,
      scrollable,
      find.descendant(
          of: find.byType(SettingsPanel),
          matching: find.text(after.settingsFormattingLabel.toUpperCase())));
  await tester
      .tap(find.text(after.settingsLocaleSystemDefault).hitTestable().first);
  await pumpSteps(tester, steps: 3);
  await tester.tap(find.text(Tokens.langName(target)!).hitTestable().last);
  await pumpSteps(tester, steps: 3);
  expect(ready.session.prefs.formattingLocale, target);
  expect(ready.session.prefs.textLocale, target,
      reason: 'the two prefs move independently (decision 4)');

  expect(ready.overflows, isEmpty,
      reason: 'zero overflows expected on the Settings tab at '
          '${size.width.toInt()}x${size.height.toInt()} '
          '(${french ? 'fr' : 'en'}):\n${ready.overflows.join('\n')}');
}

void main() {
  for (final size in const [Size(360, 800), Size(360, 640)]) {
    testWidgets(
        'settings tab at ${size.width.toInt()}x${size.height.toInt()}, en: '
        'detail labels hold, pickers usable, zero overflows', (tester) async {
      await _expectSettingsAt(tester, size, french: false);
    });

    testWidgets(
        'settings tab at ${size.width.toInt()}x${size.height.toInt()}, fr: '
        'detail labels hold, pickers usable, zero overflows', (tester) async {
      await _expectSettingsAt(tester, size, french: true);
    });
  }
}
