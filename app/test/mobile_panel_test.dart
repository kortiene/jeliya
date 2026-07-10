/// Mobile Pipes/Files surfaces + room-detail route (issue #17 polish):
///
/// - deep links keep their intent: the chat header's Share file / Open pipe
///   and the timeline pipe tile each land on the room-detail route with the
///   matching RightPanel tab;
/// - the pinned Pipes/Files tabs show the honest select-a-room empty state
///   when no room is open, and the web's `.panel-room-context` room label
///   when one is;
/// - fetch honesty survives the mobile surface: hash_mismatch is TERMINAL
///   (no Retry), a self-shared file reads 'Serving to peers', never a fault;
/// - panel actions meet the 44dp touch floor (web `.btn` min-height parity);
/// - zero recorded overflows at 360x800 AND 360x640, in English AND French
///   (the #14 lesson), across Files, Pipes, and every room-detail tab.
///
/// Copy is asserted via the shared en/fr catalog instances (docs/i18n.md
/// rule 6); room/file/pipe names are mock fixture data and stay literal.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/screens/mobile_shell.dart';
import 'package:jeliya_app/src/screens/right_panel.dart';
import 'package:jeliya_app/src/screens/timeline.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart'
    show ErrorCodes, RequestError;

import 'helpers.dart';

/// The auto-opened mock fixture room (bootstrap opens the first active room).
// i18n-exempt: fixture room name (coincides with modalRoomNamePlaceholder)
const String _fixtureRoom = 'Build Iroh Rooms MVP';

Finder _bottomTab(String label) => find.descendant(
    of: find.byType(MobileTabBar),
    matching: find.widgetWithText(InkWell, label));

/// The one on-screen panel (IndexedStack keeps the offstage twin built).
Finder _visiblePanel() => find.byType(RightPanel).hitTestable();

/// A tab button inside the visible panel's strip.
Finder _panelTab(String label) => find.descendant(
    of: _visiblePanel(), matching: find.widgetWithText(InkWell, label));

/// Taps a room row to push the chat route over the rooms list.
Future<void> _openChat(WidgetTester tester) async {
  await tester.tap(find.text(_fixtureRoom).hitTestable());
  await pumpSteps(tester, steps: 6);
}

/// Scrolls the chat timeline (a lazy ListView) upward until [finder] is
/// on screen — pipe tiles sit above the stick-to-bottom viewport.
Future<void> _revealInTimeline(WidgetTester tester, Finder finder) async {
  for (var i = 0;
      i < 40 && finder.hitTestable().evaluate().isEmpty;
      i++) {
    await tester.drag(
        find.byType(TimelineView).hitTestable(), const Offset(0, 160));
    await tester.pump();
  }
  expect(finder.hitTestable(), findsWidgets,
      reason: 'timeline never revealed $finder');
}

/// Fails every `file.fetch` with the integrity-check error — the protocol's
/// one deliberately unretryable fetch outcome.
class _HashMismatchClient extends DelegatingClient {
  _HashMismatchClient(super.inner);

  @override
  Future<dynamic> call(String method, [Map<String, dynamic>? params]) {
    if (method == 'file.fetch') {
      return Future<dynamic>.delayed(
        inner.callLatency,
        () => throw RequestError(
            ErrorCodes.hashMismatch, 'file hash did not match manifest',
            hint: 'the fetched copy was discarded'),
      );
    }
    return inner.call(method, params);
  }
}

void main() {
  testWidgets("deep link: chat header 'Share file' lands on the room-detail "
      'Files tab', (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());
    await _openChat(tester);

    await tester
        .tap(find.textContaining(en.roomHeaderShareFile).hitTestable());
    await pumpSteps(tester, steps: 6);

    expect(tester.widget<RightPanel>(_visiblePanel()).tab, PanelTab.files,
        reason: "Share file must land on the detail route's Files tab");
    expect(find.text(en.panelShareCardTitle).hitTestable(), findsOneWidget,
        reason: 'the share form must be immediately in reach');
  });

  testWidgets("deep link: chat header 'Open pipe' lands on the room-detail "
      'Pipes tab', (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());
    await _openChat(tester);

    await tester
        .tap(find.textContaining(en.roomHeaderOpenPipe).hitTestable());
    await pumpSteps(tester, steps: 6);

    expect(tester.widget<RightPanel>(_visiblePanel()).tab, PanelTab.pipes,
        reason: "Open pipe must land on the detail route's Pipes tab");
    // The fixture's open pipes render as rows (fixture targets are literal).
    expect(find.text('127.0.0.1:3000').hitTestable(), findsOneWidget);
  });

  testWidgets("deep link: a timeline pipe tile's 'Open in Pipes' lands on "
      'the room-detail Pipes tab', (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());
    await _openChat(tester);

    final tile = find.text(en.timelineOpenInPipes);
    await _revealInTimeline(tester, tile);
    await tester.tap(tile.hitTestable().first);
    await pumpSteps(tester, steps: 6);

    expect(tester.widget<RightPanel>(_visiblePanel()).tab, PanelTab.pipes,
        reason: 'the pipe tile must land on the Pipes tab');
    expect(find.text('127.0.0.1:3000').hitTestable(), findsOneWidget);
  });

  testWidgets(
      'pinned Pipes/Files tabs: honest select-a-room empty state when no '
      'room is open, room-context label when one is', (tester) async {
    final ready = await pumpReadyMobileApp(tester, newMockClient());

    // A room is open (bootstrap auto-opens the fixture room): the pinned
    // surface shows the panel under the web's `.panel-room-context` label —
    // the only room label these standalone panes get.
    await tester.tap(_bottomTab(en.sidebarNavFiles));
    await pumpSteps(tester, steps: 3);
    expect(find.text(_fixtureRoom).hitTestable(), findsOneWidget);
    expect(_visiblePanel(), findsOneWidget);

    // No room open: an honest prompt, not an empty panel.
    ready.session.leaveCurrentRoom();
    await pumpSteps(tester, steps: 3);
    expect(find.text(en.shellSelectRoom).hitTestable(), findsOneWidget);
    expect(_visiblePanel(), findsNothing);

    await tester.tap(_bottomTab(en.sidebarNavPipes));
    await pumpSteps(tester, steps: 3);
    expect(find.text(en.shellSelectRoom).hitTestable(), findsOneWidget);
    expect(_visiblePanel(), findsNothing);
  });

  testWidgets(
      'fetch honesty on the mobile Files tab: a self-shared file reads '
      "'Serving to peers', never a fault", (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());
    await tester.tap(_bottomTab(en.sidebarNavFiles));
    await pumpSteps(tester, steps: 3);

    // Exactly the one self-shared fixture file (room-protocol.md) serves.
    final serving = find.textContaining(en.panelHealthServingToPeers);
    await tester.ensureVisible(serving.first);
    await tester.pump();
    expect(serving, findsOneWidget);
    expect(find.text(en.commonServing), findsOneWidget,
        reason: 'the self-shared row carries the Serving pill, not a fetch');

    // The genuinely unavailable non-self file stays an honest amber state.
    expect(find.textContaining(en.commonNoProviderOnline), findsWidgets);

    // Fetch renders only for available non-self unfetched files (3 in the
    // fixture) — never for the self-shared or provider-less rows.
    expect(find.widgetWithText(TextButton, en.commonFetch), findsNWidgets(3));
  });

  testWidgets(
      'fetch honesty on the mobile Files tab: hash_mismatch is terminal — '
      'no Retry', (tester) async {
    await pumpReadyMobileApp(
        tester, _HashMismatchClient(newMockClient()));
    await tester.tap(_bottomTab(en.sidebarNavFiles));
    await pumpSteps(tester, steps: 3);

    final fetch = find.widgetWithText(TextButton, en.commonFetch);
    await tester.ensureVisible(fetch.first);
    await tester.pump();
    await tester.tap(fetch.first);
    await pumpSteps(tester, steps: 6);

    expect(find.text(en.commonFailed), findsOneWidget);
    expect(find.textContaining(en.fetchErrHashMismatch), findsOneWidget,
        reason: 'the integrity failure leads with plain language');
    expect(find.text(en.commonRetry), findsNothing,
        reason: 'hash_mismatch is a hard stop — retry is withheld');
    expect(fetch, findsNWidgets(2),
        reason: 'the other fetchable rows keep their controls');
  });

  testWidgets('mobile panel actions meet the 44dp touch floor',
      (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());

    await tester.tap(_bottomTab(en.sidebarNavFiles));
    await pumpSteps(tester, steps: 3);
    for (final label in [en.panelTabMembers, en.panelTabPipes]) {
      expect(tester.getSize(_panelTab(label)).height,
          greaterThanOrEqualTo(44),
          reason: "panel tab '$label' is under the 44dp floor");
    }
    final chooseFile = find.widgetWithText(TextButton, en.panelChooseFile);
    expect(tester.getSize(chooseFile).height, greaterThanOrEqualTo(44));
    final share = find.widgetWithText(TextButton, en.panelShare);
    await tester.ensureVisible(share);
    await tester.pump();
    expect(tester.getSize(share).height, greaterThanOrEqualTo(44));
    final fetch = find.widgetWithText(TextButton, en.commonFetch);
    await tester.ensureVisible(fetch.first);
    await tester.pump();
    expect(tester.getSize(fetch.first).height, greaterThanOrEqualTo(44));

    await tester.tap(_bottomTab(en.sidebarNavPipes));
    await pumpSteps(tester, steps: 3);
    final connect = find.widgetWithText(TextButton, en.panelConnect);
    await tester.ensureVisible(connect.first);
    await tester.pump();
    expect(tester.getSize(connect.first).height, greaterThanOrEqualTo(44));
    final close = find.widgetWithText(TextButton, en.panelClosePipe);
    expect(tester.getSize(close.first).height, greaterThanOrEqualTo(44));
  });

  // The #14 lesson at the mobile widths: the full-width panel content must
  // hold at 360dp under BOTH catalogs with zero recorded overflows — the
  // pinned surfaces AND every tab of the room-detail route.
  for (final size in const [Size(360, 800), Size(360, 640)]) {
    for (final french in const [false, true]) {
      final label =
          '${size.width.toInt()}x${size.height.toInt()}, ${french ? 'fr' : 'en'}';
      testWidgets(
          'panel surfaces + room detail: zero overflows at $label',
          (tester) async {
        final ready =
            await pumpReadyMobileApp(tester, newMockClient(), size: size);
        if (french) {
          // The live-switch idiom (panel_fr_layout_test): flip the pref.
          ready.session.prefs.textLocale = 'fr';
          await pumpSteps(tester, steps: 3);
        }
        final s = french ? fr : en;

        // Pinned Pipes, then Files (SingleChildScrollView lays out the
        // entire tab body, so every row participates in overflow checks).
        await tester.tap(_bottomTab(s.sidebarNavPipes).hitTestable());
        await pumpSteps(tester, steps: 3);
        expect(find.text(_fixtureRoom).hitTestable(), findsOneWidget,
            reason: 'the room-context label must top the pinned surface');

        await tester.tap(_bottomTab(s.sidebarNavFiles).hitTestable());
        await pumpSteps(tester, steps: 3);
        expect(find.text(s.panelShareCardTitle), findsOneWidget);

        // Members on the pinned strip → the shell pushes the room-detail
        // route; walk its remaining tabs (locally-owned state).
        await tester.tap(_panelTab(s.panelTabMembers).hitTestable());
        await pumpSteps(tester, steps: 6);
        expect(find.byType(BackButton).hitTestable(), findsOneWidget,
            reason: 'members must land on the room-detail route');
        for (final tab in [
          s.panelTabAgents,
          s.panelTabFiles,
          s.panelTabPipes,
        ]) {
          final target = _panelTab(tab);
          await tester.ensureVisible(target);
          await tester.pump();
          await tester.tap(target);
          await pumpSteps(tester, steps: 3);
        }

        expect(ready.overflows, isEmpty,
            reason: 'zero overflows expected at $label:\n'
                '${ready.overflows.join('\n')}');
      });
    }
  }
}
