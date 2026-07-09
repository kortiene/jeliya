/// French-width layout regressions in the 320px right panel: the self-owner
/// roster row once collapsed to ONE GLYPH PER LINE because the inline
/// « Le propriétaire reste » note (~2x wider than 'Owner stays') starved the
/// Expanded name column, and the Members tab ellipsized to 'Me…' in its
/// rigid quarter-width slot. The layout must hold under the wider French
/// copy at the real panel width — with EVERY count badge populated (the
/// review measured the no-flex strip overflowing by 4.5px exactly there).
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/screens/right_panel.dart';

import 'helpers.dart';

Future<void> _pumpFrenchShell(WidgetTester tester) async {
  final session = await pumpReadyApp(tester, newMockClient());
  session.prefs.textLocale = 'fr';
  await pumpSteps(tester, steps: 3);
}

Finder _inPanel(Finder matching) =>
    find.descendant(of: find.byType(RightPanel), matching: matching);

void main() {
  testWidgets('fr roster: the self-owner row lays out horizontally',
      (tester) async {
    await _pumpFrenchShell(tester);

    // 'Vous' renders on ONE line IN THE ROSTER (the timeline has its own
    // never-broken 'Vous' labels — scope to the panel): the regression
    // stacked it one letter per line, taller than wide.
    final youSize =
        tester.getSize(_inPanel(find.text(fr.commonYou)).first);
    expect(youSize.width, greaterThan(youSize.height),
        reason: 'name column collapsed — glyphs are stacking vertically');

    // The owner note is present, width-capped under the pills (96 logical
    // px at the harness's 0.5 text scale → 48).
    final note = _inPanel(find.text(fr.panelOwnerStays));
    expect(note, findsOneWidget);
    final cap = 96 * tester.platformDispatcher.textScaleFactor;
    expect(tester.getSize(note).width, lessThanOrEqualTo(cap));
  });

  testWidgets('fr tabs: every label renders at full width and the strip fits',
      (tester) async {
    await _pumpFrenchShell(tester);

    // Ellipsis happens at paint time, so compare laid-out width with the
    // label's intrinsic width under the same style and the HARNESS's text
    // scale (never hard-code it — a scale change must not disarm this).
    final scaler =
        TextScaler.linear(tester.platformDispatcher.textScaleFactor);
    for (final label in [fr.panelTabMembers, fr.panelTabFiles]) {
      final painter = TextPainter(
        text: TextSpan(
          text: label,
          style: const TextStyle(fontSize: 13, fontWeight: FontWeight.w600),
        ),
        textDirection: TextDirection.ltr,
        textScaler: scaler,
      )..layout();
      final rendered = tester.getSize(_inPanel(find.text(label)).first);
      expect(rendered.width, greaterThanOrEqualTo(painter.width - 0.5),
          reason: "tab label '$label' got less than its intrinsic width — "
              'it ellipsizes');
    }

    // And the strip FITS: the overflow scroll is a pathological-state
    // safety valve (four 99+ badges), never engaged by the demo fixture —
    // the harness swallows RenderFlex overflow reports, so assert the
    // scroll extent instead.
    final tabStrip = tester
        .widgetList<Scrollable>(_inPanel(find.byType(Scrollable)))
        .where((s) => axisDirectionToAxis(s.axisDirection) == Axis.horizontal);
    expect(tabStrip, hasLength(1),
        reason: 'expected exactly the tab strip to scroll horizontally');
    final position = tester
        .stateList<ScrollableState>(_inPanel(find.byType(Scrollable)))
        .map((s) => s.position)
        .firstWhere((p) => p.axis == Axis.horizontal);
    expect(position.maxScrollExtent, 0,
        reason: 'the fr tab strip must fit without scrolling — it overflows');
  });
}
