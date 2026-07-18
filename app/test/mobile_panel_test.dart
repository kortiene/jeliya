/// Mobile room tools — the compact inspector surfaces (issue #17 polish, on the
/// Room Workbench IA of docs/room-workbench.md):
///
/// - deep links keep their intent: the room app bar's ⋮ sheet Share file / Open
///   pipe and a timeline pipe tile each land on a VISIBLE room tool (Files or
///   Pipes) — an action lands on a visible surface, never a hidden pane (#54);
/// - the route, the room-nav aria-selected tab, and the visible inspector pane
///   agree — the invariant that replaced the pinned-Files-bottom-tab agreement,
///   now structurally impossible (Files and Pipes are room tools, not bottom
///   destinations);
/// - fetch honesty survives the mobile surface: hash_mismatch is TERMINAL (no
///   Retry), a self-shared file reads 'Serving to peers', never a fault;
/// - room-tool actions meet the 44dp touch floor (web `.btn` min-height parity);
/// - zero recorded overflows at 360x800 AND 360x640, in English AND French
///   (the #14 lesson), across every room tool.
///
/// Copy is asserted via the shared en/fr catalog instances (docs/i18n.md
/// rule 6); room/file/pipe names are mock fixture data and stay literal.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/routes.dart';
import 'package:jeliya_app/src/screens/right_panel.dart';
import 'package:jeliya_app/src/screens/timeline.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart'
    show ErrorCodes, RequestError;

import 'helpers.dart';

/// The auto-opened mock fixture room (bootstrap opens the first active room).
// i18n-exempt: fixture room name (coincides with modalRoomNamePlaceholder)
const String _fixtureRoom = 'Build Iroh Rooms MVP';

/// The one on-screen inspector (IndexedStack keeps the offstage twin built, but
/// finders skip it, so the hit-testable RightPanel is the visible tool).
Finder _visiblePanel() => find.byType(RightPanel).hitTestable();

/// A room-nav tab button inside the visible inspector's strip.
Finder _panelTab(String label) => find.descendant(
    of: _visiblePanel(), matching: find.widgetWithText(InkWell, label));

/// Scope a finder to the visible inspector. Files/pipes names also appear on
/// the offstage timeline pane (IndexedStack keeps it built), so an unscoped
/// text finder is ambiguous — the inspector's own copy is a descendant of the
/// hit-testable RightPanel.
Finder _inPanel(Finder matching) =>
    find.descendant(of: _visiblePanel(), matching: matching);

/// The Fetch control inside the visible inspector — scoped so an offstage
/// timeline fetch tile can never be the one a test taps.
Finder _panelFetch() =>
    _inPanel(find.widgetWithText(TextButton, en.commonFetch));

/// Reach a room tool from wherever the room-nav strip is on screen (the room's
/// Activity pane or another tool's inspector — both carry the strip). The strip
/// scrolls horizontally: five labels overflow a phone, so some tabs sit off an
/// edge at 360dp (in French the wide "Agents et exécutions" pushes Files and
/// Pipes further out still), and its scroll offset persists across tab switches
/// — so the target may be off the LEFT edge as easily as the right. mobileGoToDest
/// taps blind and cannot reach an off-screen tab, so rewind the strip to the
/// start, then advance until the target is on screen, and tap it.
Future<void> _goToTool(WidgetTester tester, String label) async {
  final strip = find.byWidgetPredicate(
      (w) => w is Scrollable && w.axisDirection == AxisDirection.right);
  // Scope to the room-nav strip: the activity-filter chips below the timeline
  // repeat "Files"/"Pipes" (issue #65), so an unscoped text finder is ambiguous
  // on the Activity pane — the nav tab lives in the horizontal strip.
  final tab = find.descendant(of: strip.first, matching: find.text(label));
  for (var i = 0; i < 6; i++) {
    await tester.drag(strip.first, const Offset(240, 0));
    await tester.pump();
  }
  for (var i = 0; i < 8 && tab.hitTestable().evaluate().isEmpty; i++) {
    await tester.drag(strip.first, const Offset(-160, 0));
    await tester.pump();
  }
  await tester.tap(tab.hitTestable().first);
  await pumpSteps(tester, steps: 4);
}

/// Boot lands inside the fixture room's Activity; reach the rooms list and
/// re-open the room the way a user would, so the deep-link tests drive the
/// room-row → Activity selection rather than depending on the boot pane.
Future<void> _openChat(WidgetTester tester) async {
  final back = find.bySemanticsLabel(en.roomBackToRooms);
  if (back.evaluate().isNotEmpty) {
    await tester.tap(back.first);
    await pumpSteps(tester, steps: 4);
  }
  await tester.tap(find.text(_fixtureRoom).hitTestable().first);
  await pumpSteps(tester, steps: 6);
}

/// Opens the room app bar's ⋮ information sheet, where the two room tools the
/// compact bar has no width for (Share file / Open pipe) now live.
Future<void> _openRoomInfo(WidgetTester tester) async {
  await tester.tap(find.bySemanticsLabel(en.roomInformation));
  await pumpSteps(tester, steps: 3);
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
  testWidgets("deep link: chat header 'Share file' opens the visible Files "
      'tool', (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());
    await _openChat(tester);

    // Share file moved into the app bar's ⋮ sheet (the compact bar is one
    // non-wrapping row). Tapping it must still land on a VISIBLE Files surface,
    // never a hidden pane (issue #54).
    await _openRoomInfo(tester);
    await tester
        .tap(find.textContaining(en.roomHeaderShareFile).hitTestable());
    await pumpSteps(tester, steps: 6);

    expect(tester.widget<RightPanel>(_visiblePanel()).tab, RoomDest.files,
        reason: 'Share file navigates to the room Files tool');
    // List-first (#67): sharing is one tap away behind a compact 'Share a file'
    // button, not a form standing open. The button is on the visible pane, and
    // tapping it reveals the share card.
    final toggle = _inPanel(find.text(en.panelFilesShareToggle));
    expect(toggle.hitTestable(), findsOneWidget,
        reason: 'the Share a file toggle leads the visible Files pane');
    await tester.tap(toggle.hitTestable());
    await pumpSteps(tester, steps: 3);
    expect(find.text(en.panelShareCardTitle).hitTestable(), findsOneWidget,
        reason: 'the toggle reveals the share form');
  });

  testWidgets("deep link: chat header 'Open pipe' opens the visible Pipes "
      'tool', (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());
    await _openChat(tester);

    await _openRoomInfo(tester);
    await tester
        .tap(find.textContaining(en.roomHeaderOpenPipe).hitTestable());
    await pumpSteps(tester, steps: 6);

    expect(tester.widget<RightPanel>(_visiblePanel()).tab, RoomDest.pipes,
        reason: 'Open pipe navigates to the room Pipes tool');
    // The fixture's open pipes render as rows (fixture targets are literal).
    expect(find.text('127.0.0.1:3000').hitTestable(), findsOneWidget);
  });

  testWidgets("deep link: a timeline pipe tile's 'Open in Pipes' lands on the "
      'Pipes tool WITH the pipe selected', (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());
    await _openChat(tester);

    final tile = find.text(en.timelineOpenInPipes);
    await _revealInTimeline(tester, tile);
    await tester.tap(tile.hitTestable().first);
    await pumpSteps(tester, steps: 6);

    final panel = tester.widget<RightPanel>(_visiblePanel());
    expect(panel.tab, RoomDest.pipes,
        reason: 'the pipe tile lands on the Pipes tool');
    expect(panel.selectedItem, isNotNull,
        reason: 'the tile deep-links to the pipe, not just the tool (#67)');
    expect(find.text('127.0.0.1:3000').hitTestable(), findsOneWidget);
  });

  testWidgets("deep link: a timeline file tile's 'Open in Files' lands on the "
      'Files tool WITH the file selected, list intact', (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());
    await _openChat(tester);

    final tile = find.text(en.timelineOpenInFiles);
    await _revealInTimeline(tester, tile);
    await tester.tap(tile.hitTestable().first);
    await pumpSteps(tester, steps: 6);

    final panel = tester.widget<RightPanel>(_visiblePanel());
    expect(panel.tab, RoomDest.files,
        reason: 'the file tile lands on the Files tool');
    expect(panel.selectedItem, isNotNull,
        reason: 'the tile deep-links to the file, not just the tool (#67)');
    // The list is not lost behind the opened inspector — the shared-files
    // section head (uppercased) is still laid out on the pane.
    expect(_inPanel(find.text(en.panelSharedInThisRoom.toUpperCase())),
        findsOneWidget,
        reason: 'the file list survives the deep link');
  });

  testWidgets(
      'file-select deep link: tapping a file row opens its inspector without '
      'losing the list', (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());
    await _goToTool(tester, en.roomDestFiles);

    // A fetchable, non-self fixture file (room-protocol.md is self-shared).
    final row = _inPanel(find.text('wireframe.png'));
    await tester.ensureVisible(row.first);
    await tester.pump();
    await tester.tap(row.hitTestable().first);
    await pumpSteps(tester, steps: 4);

    // The selection became route state (the panel reads it off the route).
    expect(tester.widget<RightPanel>(_visiblePanel()).selectedItem, isNotNull,
        reason: 'selecting a row is a deep link, not a local field');
    // The contextual inspector opened: exactly the selected file's fetch
    // control (single-selection — one control, not one per row).
    expect(_panelFetch(), findsOneWidget,
        reason: 'the selected file inspector renders its FetchControl');
    // The list is intact: the section head (uppercased) and the other rows are
    // still there.
    expect(_inPanel(find.text(en.panelSharedInThisRoom.toUpperCase())),
        findsOneWidget);
    expect(_inPanel(find.text('room-protocol.md')), findsOneWidget);
  });

  testWidgets(
      'pipe-select deep link: tapping a pipe row marks it selected on the route',
      (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());
    await _goToTool(tester, en.roomDestPipes);

    final row = _inPanel(find.text('127.0.0.1:3000'));
    await tester.ensureVisible(row.first);
    await tester.pump();
    await tester.tap(row.hitTestable().first);
    await pumpSteps(tester, steps: 4);

    expect(tester.widget<RightPanel>(_visiblePanel()).selectedItem, isNotNull,
        reason: 'selecting a pipe row is a deep link the route carries');
  });

  testWidgets(
      'route, room-nav aria-selected, and the visible pane agree — the '
      'invariant that replaced the (now impossible) pinned-Files-tab agreement',
      (tester) async {
    // Files and Pipes are no longer bottom destinations, so 'the panel tab
    // strip keeps the bottom navigation truthful' cannot be violated — there is
    // nothing left to disagree with. What CAN still drift is the route, the
    // room-nav selection, and the pane; the workbench keeps the three one thing,
    // so prove they agree: open Files from the strip and read all three back.
    await pumpReadyMobileApp(tester, newMockClient());
    await _goToTool(tester, en.roomDestFiles);

    final panel = _visiblePanel();
    // The pane (and the route it is derived from): the visible inspector is
    // Files, and its Files body is the one on screen — the list-leading
    // 'Share a file' toggle is Files-specific and always present.
    expect(tester.widget<RightPanel>(panel).tab, RoomDest.files);
    expect(_inPanel(find.text(en.panelFilesShareToggle)).hitTestable(),
        findsOneWidget,
        reason: 'the Files tool body is the one on screen');
    // The strip: the Files tab is the selected one (aria-selected), so the tab
    // strip and the pane cannot say different things — both are the route. The
    // active RoomNav tab wraps its button in Semantics(selected: true).
    expect(
      find.ancestor(
        of: _panelTab(en.roomDestFiles),
        matching: find.byWidgetPredicate(
            (w) => w is Semantics && w.properties.selected == true),
      ),
      findsOneWidget,
      reason: 'the room-nav Files tab is aria-selected',
    );
  });

  testWidgets(
      'fetch honesty on the Files tool: a self-shared file reads '
      "'Serving to peers', never a fault", (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());
    await _goToTool(tester, en.roomDestFiles);

    // The health line is the always-visible honest summary in every row.
    // Exactly the one self-shared fixture file (room-protocol.md) serves.
    final serving = find.textContaining(en.panelHealthServingToPeers);
    await tester.ensureVisible(serving.first);
    await tester.pump();
    expect(serving, findsOneWidget);
    // The genuinely unavailable non-self file stays an honest amber state.
    expect(find.textContaining(en.commonNoProviderOnline), findsWidgets);

    // Selecting the self-shared row opens its contextual inspector: the Serving
    // pill, NEVER a fetch control (SELF-OWNED FILE SEMANTICS survives the move
    // behind selection).
    final selfRow = _inPanel(find.text('room-protocol.md'));
    await tester.ensureVisible(selfRow.first);
    await tester.pump();
    await tester.tap(selfRow.hitTestable().first);
    await pumpSteps(tester, steps: 4);
    expect(find.text(en.commonServing), findsOneWidget,
        reason: 'the self-shared inspector carries the Serving pill');
    expect(find.widgetWithText(TextButton, en.commonFetch), findsNothing,
        reason: 'a self-shared file is never offered a fetch');
  });

  testWidgets(
      'fetch honesty on the Files tool: hash_mismatch is terminal — no Retry',
      (tester) async {
    await pumpReadyMobileApp(
        tester, _HashMismatchClient(newMockClient()));
    await _goToTool(tester, en.roomDestFiles);

    // The fetch control lives in a selected file's inspector now (#67): select
    // a fetchable non-self fixture file to reveal it.
    final row = _inPanel(find.text('wireframe.png'));
    await tester.ensureVisible(row.first);
    await tester.pump();
    await tester.tap(row.hitTestable().first);
    await pumpSteps(tester, steps: 4);

    final fetch = _panelFetch();
    await tester.ensureVisible(fetch);
    await tester.pump();
    await tester.tap(fetch);
    await pumpSteps(tester, steps: 6);

    expect(find.text(en.commonFailed), findsOneWidget);
    expect(find.textContaining(en.fetchErrHashMismatch), findsOneWidget,
        reason: 'the integrity failure leads with plain language');
    expect(find.text(en.commonRetry), findsNothing,
        reason: 'hash_mismatch is a hard stop — retry is withheld');
  });

  testWidgets('room-tool actions meet the 44dp touch floor', (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());

    await _goToTool(tester, en.roomDestFiles);
    // The room-nav tabs clear the floor even off the strip's visible edge
    // (getSize measures the render box regardless of scroll position).
    for (final label in [en.roomDestPeople, en.roomDestPipes]) {
      expect(tester.getSize(_panelTab(label)).height,
          greaterThanOrEqualTo(44),
          reason: "room-nav tab '$label' is under the 44dp floor");
    }
    // The share controls live behind the list-first 'Share a file' toggle now;
    // reveal them, then measure.
    await tester.tap(_inPanel(find.text(en.panelFilesShareToggle)).hitTestable());
    await pumpSteps(tester, steps: 3);
    final chooseFile = find.widgetWithText(TextButton, en.panelChooseFile);
    await tester.ensureVisible(chooseFile);
    await tester.pump();
    expect(tester.getSize(chooseFile).height, greaterThanOrEqualTo(44));
    final share = find.widgetWithText(TextButton, en.panelShare);
    await tester.ensureVisible(share);
    await tester.pump();
    expect(tester.getSize(share).height, greaterThanOrEqualTo(44));
    // The fetch control is in a selected file's inspector; select one, then
    // measure.
    final row = _inPanel(find.text('wireframe.png'));
    await tester.ensureVisible(row.first);
    await tester.pump();
    await tester.tap(row.hitTestable().first);
    await pumpSteps(tester, steps: 4);
    final fetch = _panelFetch();
    await tester.ensureVisible(fetch);
    await tester.pump();
    expect(tester.getSize(fetch).height, greaterThanOrEqualTo(44));

    await _goToTool(tester, en.roomDestPipes);
    final connect = find.widgetWithText(TextButton, en.panelConnect);
    await tester.ensureVisible(connect.first);
    await tester.pump();
    expect(tester.getSize(connect.first).height, greaterThanOrEqualTo(44));
    final close = find.widgetWithText(TextButton, en.panelClosePipe);
    expect(tester.getSize(close.first).height, greaterThanOrEqualTo(44));
  });

  // The #14 lesson at the mobile widths: every room tool's full-width, full-body
  // layout must hold at 360dp under BOTH catalogs with zero recorded overflows.
  for (final size in const [Size(360, 800), Size(360, 640)]) {
    for (final french in const [false, true]) {
      final label =
          '${size.width.toInt()}x${size.height.toInt()}, ${french ? 'fr' : 'en'}';
      testWidgets('room tools: zero overflows at $label', (tester) async {
        final ready =
            await pumpReadyMobileApp(tester, newMockClient(), size: size);
        if (french) {
          // The live-switch idiom (panel_fr_layout_test): flip the pref.
          ready.session.prefs.textLocale = 'fr';
          await pumpSteps(tester, steps: 3);
        }
        final s = french ? fr : en;

        // Boot lands in the fixture room's Activity. Walk every room tool via
        // the nav strip — on compact each tool takes the whole pane and its
        // SingleChildScrollView lays out its entire body, so every row
        // participates in the overflow check.
        await _goToTool(tester, s.roomDestPipes);
        expect(find.text(_fixtureRoom), findsOneWidget,
            reason: 'the inspector head carries the room name on compact');

        await _goToTool(tester, s.roomDestFiles);
        // Reveal the share sheet (list-first: behind a toggle now) so its full
        // layout participates in the overflow check.
        await tester
            .tap(_inPanel(find.text(s.panelFilesShareToggle)).hitTestable());
        await pumpSteps(tester, steps: 3);
        expect(find.text(s.panelShareCardTitle), findsOneWidget);
        // Select a non-self file so the contextual inspector (FetchControl +
        // detail) lays out under the reflow too.
        final fileRow = _inPanel(find.text('wireframe.png'));
        await tester.ensureVisible(fileRow.first);
        await tester.pump();
        await tester.tap(fileRow.hitTestable().first);
        await pumpSteps(tester, steps: 3);

        await _goToTool(tester, s.roomDestPeople);
        // A room tool covers the pane (there is no pushed detail route now); the
        // visible inspector IS People.
        expect(tester.widget<RightPanel>(_visiblePanel()).tab, RoomDest.people,
            reason: 'the People tool is the pane on screen');

        await _goToTool(tester, s.roomDestAgents);

        expect(ready.overflows, isEmpty,
            reason: 'zero overflows expected at $label:\n'
                '${ready.overflows.join('\n')}');
      });
    }
  }
}
