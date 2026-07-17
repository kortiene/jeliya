/// Mobile Agents tab (issue #17 polish): below the breakpoint the KPI strip
/// is hidden and the filter chips carry the live counts instead (web parity:
/// styles.css hides .fleet-stats on phones), the header wraps so the search
/// flexes beside Add Agent at 360dp, every agent card states its liveness as
/// dot + label (never color alone), and the FleetStore 4s poll is GATED on
/// lifecycle: it runs only while the tab is active AND the app is foregrounded,
/// pauses (without unmounting — search/filter/scroll are retained) otherwise,
/// and resumes with exactly one reload (#69).
/// Strict surface: 360x800 AND 360x640, en AND fr, textScale 1.0, DPR 1.0,
/// zero recorded overflows; copy asserted via the shared catalog instances.
library;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/screens/fleet_dashboard.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart' show shortId;
import 'package:jeliya_protocol/testing.dart' show MockPeople;

import 'helpers.dart';

/// Counts `agents.fleet` round trips — the observable trace of the
/// FleetStore poll loop.
class _FleetCountingClient extends DelegatingClient {
  _FleetCountingClient(super.inner);

  int fleetCalls = 0;

  @override
  Future<dynamic> call(String method, [Map<String, dynamic>? params]) {
    if (method == 'agents.fleet') fleetCalls += 1;
    return super.call(method, params);
  }
}

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

Future<void> _expectAgentsSurfaceAt(WidgetTester tester, Size size,
    {required bool french}) async {
  final ready = await pumpReadyMobileApp(tester, newMockClient(), size: size);
  // Boot lands inside a room now, where the bottom bar is gone — reach the
  // Fleet pane through the rooms list. The shared nav helper keys off the
  // English Back-to-Rooms label, so navigate BEFORE switching locale, then
  // flip the pref (the panel_fr_layout_test live-switch idiom) and let the
  // fleet surface re-render in French in place.
  await mobileGoToGlobal(tester, en.sidebarNavFleet);
  if (french) {
    ready.session.prefs.textLocale = 'fr';
    await pumpSteps(tester, steps: 3);
  }
  final s = french ? fr : en;

  await pumpSteps(tester, steps: 10);
  expect(find.byType(FleetDashboard), findsOneWidget);

  // The KPI strip is hidden below the breakpoint (web: .app .fleet-stats
  // display none)...
  expect(find.text(s.fleetStatActiveAgents), findsNothing);
  expect(find.text(s.fleetStatWorkingNow), findsNothing);
  expect(find.text(s.fleetStatRoomCoverage), findsNothing);

  // ...and the filter chips carry the live counts instead. Fixture fleet:
  // 5 agents — backend working + frontend online-idle are Live, backend alone
  // is Working, and research (stale) + qa (offline) + deploy (offline, failed
  // status) fold into Offline.
  Finder chipCount(String label, String count) => find.descendant(
      of: find.widgetWithText(TextButton, label), matching: find.text(count));
  expect(chipCount(s.fleetFilterAll, '5'), findsOneWidget);
  expect(chipCount(s.fleetFilterLive, '2'), findsOneWidget);
  expect(chipCount(s.fleetFilterWorking, '1'), findsOneWidget);
  expect(chipCount(s.fleetFilterOffline, '3'), findsOneWidget);

  // The wrapped header keeps both actions usable at 360dp: the search field
  // flexes (never the fixed 200px) and Add Agent stays hit-testable.
  expect(
      find.descendant(
          of: find.byType(FleetDashboard), matching: find.byType(TextField)),
      findsOneWidget);
  expect(find.widgetWithText(TextButton, s.fleetAddAgent).hitTestable(),
      findsOneWidget);

  // Honest liveness on every card: the wire's derived state rendered as
  // dot + label, never color alone (one fixture agent per liveness state).
  final list = find
      .descendant(
          of: find.byType(FleetDashboard), matching: find.byType(Scrollable))
      .last;
  // No aliases are seeded in widget tests, so cards render each agent as
  // shortId(identityId) — compute the on-screen names from the fixture cast.
  final byLiveness = <String, String>{
    shortId(MockPeople.backendAgent.identityId): s.fleetLivenessWorking,
    shortId(MockPeople.frontendAgent.identityId): s.fleetLivenessOnline,
    shortId(MockPeople.researchAgent.identityId): s.fleetLivenessStale,
    shortId(MockPeople.qaAgent.identityId): s.fleetLivenessOffline,
  };
  for (final MapEntry(key: name, value: liveness) in byLiveness.entries) {
    // Scope to the agent grid: the Needs Attention section above it repeats the
    // actionable agents' names (and a stale reason chip reuses the same word as
    // the Stale liveness pill), so a whole-dashboard find would be ambiguous.
    final nameText = find.descendant(
        of: find.byKey(const Key('fleetAgentGrid')), matching: find.text(name));
    await _reveal(tester, list, nameText);
    // The liveness pill shares the name's Wrap; its Row holds dot + label.
    final pill = find.descendant(
        of: find.ancestor(of: nameText, matching: find.byType(Wrap)).first,
        matching: find.text(liveness));
    expect(pill, findsOneWidget,
        reason: "'$name' must state its liveness as '$liveness'");
    final dot = find.descendant(
        of: find.ancestor(of: pill, matching: find.byType(Row)).first,
        matching: find.byWidgetPredicate((w) =>
            w is Container &&
            w.decoration is BoxDecoration &&
            (w.decoration! as BoxDecoration).shape == BoxShape.circle));
    expect(dot, findsOneWidget,
        reason: "'$name' liveness must render a dot beside the label — "
            'status is never color alone');
  }

  expect(ready.overflows, isEmpty,
      reason: 'zero overflows expected on the Agents tab at '
          '${size.width.toInt()}x${size.height.toInt()} '
          '(${french ? 'fr' : 'en'}):\n${ready.overflows.join('\n')}');
}

void main() {
  for (final size in const [Size(360, 800), Size(360, 640)]) {
    testWidgets(
        'agents tab at ${size.width.toInt()}x${size.height.toInt()}, en: '
        'KPI hidden, chips carry counts, honest liveness, zero overflows',
        (tester) async {
      await _expectAgentsSurfaceAt(tester, size, french: false);
    });

    testWidgets(
        'agents tab at ${size.width.toInt()}x${size.height.toInt()}, fr: '
        'KPI hidden, chips carry counts, honest liveness, zero overflows',
        (tester) async {
      await _expectAgentsSurfaceAt(tester, size, french: true);
    });
  }

  testWidgets('the fleet poll is gated on the active tab and the app lifecycle',
      (tester) async {
    final client = _FleetCountingClient(newMockClient());
    await pumpReadyMobileApp(tester, client);
    expect(client.fleetCalls, 0,
        reason: 'no fleet polling before the Agents tab first activates');

    await mobileGoToGlobal(tester, en.sidebarNavFleet);
    expect(find.byType(FleetDashboard), findsOneWidget);
    final afterActivate = client.fleetCalls;
    expect(afterActivate, greaterThanOrEqualTo(1),
        reason: 'activating the tab loads immediately');

    await pumpSteps(tester, steps: 90); // 9s → at least two 4s ticks
    expect(client.fleetCalls, greaterThanOrEqualTo(afterActivate + 2),
        reason: 'the 4s poll runs while the tab is active and foregrounded');

    // Leaving the tab pauses the poll — the dashboard stays MOUNTED (Offstage)
    // so its state survives, but it must never poll behind another surface.
    // (skipOffstage: false — the IndexedStack child is offstage, not removed.)
    await mobileGoToGlobal(tester, en.sidebarNavRooms);
    expect(find.byType(FleetDashboard, skipOffstage: false), findsOneWidget,
        reason: 'the dashboard stays mounted (Offstage) to retain its state');
    final afterLeave = client.fleetCalls;
    await pumpSteps(tester, steps: 120); // 12s off-surface
    expect(client.fleetCalls, afterLeave,
        reason: 'the poll STOPS while the tab is inactive');

    // Returning resumes the poll.
    await mobileGoToGlobal(tester, en.sidebarNavFleet);
    await pumpSteps(tester, steps: 3);
    expect(client.fleetCalls, greaterThan(afterLeave),
        reason: 're-activation resumes with an immediate reload');

    // Backgrounding the app pauses the poll even while the tab is active.
    // (inactive, not paused: the lifecycle state machine only allows adjacent
    // transitions from resumed, and anything but resumed is treated as
    // background here.)
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    await pumpSteps(tester, steps: 3);
    final afterBackground = client.fleetCalls;
    await pumpSteps(tester, steps: 120); // 12s backgrounded
    expect(client.fleetCalls, afterBackground,
        reason: 'a backgrounded app must not poll, even on the active tab');

    // Resuming restarts the poll.
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await pumpSteps(tester, steps: 3);
    expect(client.fleetCalls, greaterThan(afterBackground),
        reason: 'foreground resumes the poll with an immediate reload');
  });

  testWidgets('search and filter survive leaving and returning to the tab',
      (tester) async {
    await pumpReadyMobileApp(tester, newMockClient());
    await mobileGoToGlobal(tester, en.sidebarNavFleet);
    await pumpSteps(tester, steps: 10);

    // Change both retained pieces of state: the search query and the filter.
    final searchField = find.descendant(
        of: find.byType(FleetDashboard), matching: find.byType(TextField));
    await tester.enterText(searchField, 'research');
    // "Live" is the second pill — on-screen at 360dp (Offline is last and would
    // need the filter row scrolled first).
    await tester.tap(find.widgetWithText(TextButton, en.fleetFilterLive));
    await pumpSteps(tester, steps: 3);
    expect(tester.widget<TextField>(searchField).controller?.text, 'research',
        reason: 'search text should be set before navigating away');

    // Leave the tab and come back — the dashboard stays mounted, so its search
    // and filter State are retained rather than reset (#69).
    await mobileGoToGlobal(tester, en.sidebarNavRooms);
    await pumpSteps(tester, steps: 3);
    await mobileGoToGlobal(tester, en.sidebarNavFleet);
    await pumpSteps(tester, steps: 3);

    // The search query survived the round trip (the strongest retention signal
    // — the TextEditingController lives in the dashboard's State), and so did
    // the Offline filter (its pill is still the pressed one).
    expect(tester.widget<TextField>(searchField).controller?.text, 'research',
        reason: 'search text must survive navigation (state is retained)');
    final livePill = tester.widget<TextButton>(
        find.widgetWithText(TextButton, en.fleetFilterLive));
    final allPill = tester.widget<TextButton>(
        find.widgetWithText(TextButton, en.fleetFilterAll));
    Color? bg(TextButton b) =>
        b.style?.backgroundColor?.resolve(<WidgetState>{});
    expect(bg(livePill), isNot(equals(bg(allPill))),
        reason: 'the Live filter is still active after returning, not reset '
            'to All');
  });
}
