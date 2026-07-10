/// Mobile Agents tab (issue #17 polish): below the breakpoint the KPI strip
/// is hidden and the filter chips carry the live counts instead (web parity:
/// styles.css hides .fleet-stats on phones), the header wraps so the search
/// flexes beside Add Agent at 360dp, every agent card states its liveness as
/// dot + label (never color alone), and the FleetStore 4s poll runs ONLY
/// while the tab is active — it must stop the moment the tab deactivates.
/// Strict surface: 360x800 AND 360x640, en AND fr, textScale 1.0, DPR 1.0,
/// zero recorded overflows; copy asserted via the shared catalog instances.
library;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/screens/fleet_dashboard.dart';
import 'package:jeliya_app/src/screens/mobile_shell.dart';
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

Future<void> _tapTab(WidgetTester tester, String label) async {
  await tester.tap(find.descendant(
      of: find.byType(MobileTabBar),
      matching: find.widgetWithText(InkWell, label)));
  await pumpSteps(tester, steps: 6);
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
  if (french) {
    // The live-switch idiom (panel_fr_layout_test): flip the pref, repump.
    ready.session.prefs.textLocale = 'fr';
    await pumpSteps(tester, steps: 3);
  }
  final s = french ? fr : en;

  await _tapTab(tester, s.sidebarNavAgents);
  await pumpSteps(tester, steps: 10);
  expect(find.byType(FleetDashboard), findsOneWidget);

  // The KPI strip is hidden below the breakpoint (web: .app .fleet-stats
  // display none)...
  expect(find.text(s.fleetStatActiveAgents), findsNothing);
  expect(find.text(s.fleetStatRunningTasks), findsNothing);
  expect(find.text(s.fleetStatRoomCoverage), findsNothing);

  // ...and the filter chips carry the live counts instead. Fixture fleet:
  // 4 agents — backend working + frontend online-idle are Active, backend
  // alone is Working, research (stale) + qa (offline) fold into Offline.
  Finder chipCount(String label, String count) => find.descendant(
      of: find.widgetWithText(TextButton, label), matching: find.text(count));
  expect(chipCount(s.fleetFilterAll, '4'), findsOneWidget);
  expect(chipCount(s.fleetFilterActive, '2'), findsOneWidget);
  expect(chipCount(s.fleetFilterWorking, '1'), findsOneWidget);
  expect(chipCount(s.fleetFilterOffline, '2'), findsOneWidget);

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
    final nameText = find.descendant(
        of: find.byType(FleetDashboard), matching: find.text(name));
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

  testWidgets('the 4s fleet poll runs only while the Agents tab is active',
      (tester) async {
    final client = _FleetCountingClient(newMockClient());
    await pumpReadyMobileApp(tester, client);
    expect(client.fleetCalls, 0,
        reason: 'no fleet polling before the Agents tab first activates');

    await _tapTab(tester, en.sidebarNavAgents);
    expect(find.byType(FleetDashboard), findsOneWidget);
    final afterMount = client.fleetCalls;
    expect(afterMount, greaterThanOrEqualTo(1),
        reason: 'mounting the dashboard loads immediately');

    await pumpSteps(tester, steps: 90); // 9s → at least two 4s ticks
    expect(client.fleetCalls, greaterThanOrEqualTo(afterMount + 2),
        reason: 'the 4s poll must run while the tab is active');

    await _tapTab(tester, en.sidebarNavRooms);
    expect(find.byType(FleetDashboard), findsNothing,
        reason: 'FleetDashboard must unmount when its tab deactivates');
    final afterLeave = client.fleetCalls;
    await pumpSteps(tester, steps: 120); // 12s of background time
    expect(client.fleetCalls, afterLeave,
        reason: 'the poll must STOP when the tab deactivates — it may '
            'never run in the background');

    // Re-activating mounts a fresh store: one immediate reload.
    await _tapTab(tester, en.sidebarNavAgents);
    expect(client.fleetCalls, greaterThan(afterLeave),
        reason: 're-activation reloads immediately');
  });
}
