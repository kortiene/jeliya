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
  // A MEDIUM window (900-1279): there the room's tool opens as the 360dp
  // inspector DRAWER that carries its own tab strip and roster — the exact
  // narrow panel this file measures. The wide shell hands the strip to the
  // workspace and leaves the inspector closed at Activity, and compact runs
  // the panel at textScale 1.0; neither is the surface these regressions live
  // on. Boot lands in the owner-held main room, so its self-owner roster row
  // and the ownerStays note are present once People is open.
  useDesktopSurface(tester, size: const Size(1024, 768));
  final session = newSession(newMockClient());
  await pumpApp(tester, session);
  await pumpSteps(tester);
  session.prefs.textLocale = 'fr';
  await pumpSteps(tester, steps: 3);
  // Open the People tool: collapsing/opening the inspector IS navigating, so
  // tapping the room-nav strip's Personnes tab is what mounts the RightPanel.
  await tester.tap(find.text(fr.roomDestPeople).hitTestable().first);
  await pumpSteps(tester, steps: 4);
}

Finder _inPanel(Finder matching) =>
    find.descendant(of: find.byType(RightPanel), matching: matching);

/// Switch the open French drawer to [label]'s tool. The 5-tab strip scrolls at
/// the 360dp drawer width under wide French copy, so reveal the tab before
/// tapping (ensureVisible scrolls the horizontal strip).
Future<void> _openDrawerTool(WidgetTester tester, String label) async {
  final tab = _inPanel(find.text(label));
  await tester.ensureVisible(tab.first);
  await tester.pump();
  await tester.tap(tab.hitTestable().first);
  await pumpSteps(tester, steps: 4);
}

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

  testWidgets(
      'fr tabs: every label renders at full width and the strip stays one '
      'scrolling row', (tester) async {
    await _pumpFrenchShell(tester);

    // Ellipsis happens at paint time, so compare laid-out width with the
    // label's intrinsic width under the same style and the HARNESS's text
    // scale (never hard-code it — a scale change must not disarm this).
    final scaler =
        TextScaler.linear(tester.platformDispatcher.textScaleFactor);
    for (final label in [fr.roomDestPeople, fr.roomDestFiles]) {
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

    // The original bug lived in a RIGID four-slot Row that ellipsized ('Me…')
    // and overflowed by 4.5px under wide French. The Room Workbench strip
    // (RoomNav) is a horizontal SingleChildScrollView, so a RenderFlex
    // overflow there is structurally impossible and "fits without scrolling"
    // is no longer the invariant — Activity became a fifth tab and the strip
    // is DESIGNED to scroll rather than wrap (a second row would eat the
    // timeline height the compact budget protects). The invariant that
    // replaced it: the strip degrades to exactly ONE horizontal scroll row,
    // and every label keeps its full width there (asserted above) instead of
    // ellipsizing — so widening French copy scrolls the strip, never clips it.
    final tabStrip = tester
        .widgetList<Scrollable>(_inPanel(find.byType(Scrollable)))
        .where((s) => axisDirectionToAxis(s.axisDirection) == Axis.horizontal);
    expect(tabStrip, hasLength(1),
        reason: 'the tab strip must be exactly one horizontal scroll row — a '
            'second row would eat the timeline height');
    final position = tester
        .stateList<ScrollableState>(_inPanel(find.byType(Scrollable)))
        .map((s) => s.position)
        .firstWhere((p) => p.axis == Axis.horizontal);
    // The five wide French tabs overrun the 360dp panel, and the strip absorbs
    // that by scrolling (labels stay full-width above) rather than overflowing
    // or ellipsizing — the graceful degradation the rigid strip lacked.
    expect(position.maxScrollExtent, greaterThan(0),
        reason: 'wide French copy must scroll the strip, not clip it');
  });

  testWidgets(
      'fr reflow: the list-first Files tool holds at the 360dp drawer (#67)',
      (tester) async {
    await _pumpFrenchShell(tester);
    await _openDrawerTool(tester, fr.roomDestFiles);

    // List-first: the 'Partager un fichier' toggle leads and the share card is
    // NOT standing open above the list.
    expect(_inPanel(find.text(fr.panelFilesShareToggle)).hitTestable(),
        findsOneWidget);
    expect(_inPanel(find.text(fr.panelShareCardTitle)), findsNothing,
        reason: 'the share card is behind the toggle, not open above the list');

    // The #14 clip guard on the reflowed panel: the shared-files section head's
    // French title keeps its full intrinsic width (it ellipsizes under overflow
    // pressure, so a shortfall is the clip this file exists to catch).
    final headTitle = fr.panelSharedInThisRoom.toUpperCase();
    final scaler =
        TextScaler.linear(tester.platformDispatcher.textScaleFactor);
    final painter = TextPainter(
      text: TextSpan(
          text: headTitle,
          style: const TextStyle(fontSize: 12, letterSpacing: 0.72)),
      textDirection: TextDirection.ltr,
      textScaler: scaler,
    )..layout();
    final rendered = tester.getSize(_inPanel(find.text(headTitle)).first);
    expect(rendered.width, greaterThanOrEqualTo(painter.width - 0.5),
        reason: 'the Files section head clips under wide French copy');

    // Revealing the sheet brings the share card into reach.
    await tester
        .tap(_inPanel(find.text(fr.panelFilesShareToggle)).hitTestable());
    await pumpSteps(tester, steps: 3);
    expect(_inPanel(find.text(fr.panelShareCardTitle)), findsOneWidget);
  });

  testWidgets(
      'fr reflow: the action-first Pipes tool leads with the expose form (#67)',
      (tester) async {
    await _pumpFrenchShell(tester);
    await _openDrawerTool(tester, fr.roomDestPipes);

    // Action-first: the expose form leads; the live pipes stay visible below.
    expect(_inPanel(find.text(fr.panelExposeTitle)), findsOneWidget);
    expect(_inPanel(find.text('127.0.0.1:3000')), findsWidgets,
        reason: 'existing pipes remain visible below the hoisted form');
  });
}
