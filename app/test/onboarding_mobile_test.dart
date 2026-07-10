/// Onboarding below the breakpoint (issue #17): onboarding renders BEFORE the
/// shell, so a phone user hits it first — the PR #16 device test tripped on
/// the identity card's fixed 420px width. The identity step must hold at
/// 360dp with ZERO overflows in en AND fr, and the rooms step's side-by-side
/// create/join cards must stack vertically. Strict surface: textScale 1.0,
/// DPR 1.0, overflow reports recorded and asserted EMPTY (mobile layouts are
/// new — never pixel-tuned to the fat test font).
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/screens/onboarding_identity.dart';
import 'package:jeliya_app/src/screens/onboarding_rooms.dart';
import 'package:jeliya_app/src/session/daemon_session.dart';

import 'helpers.dart';

/// Boots a fresh daemon on a strict phone surface up to the identity step.
Future<({DaemonSession session, List<String> overflows})> _pumpIdentityStep(
    WidgetTester tester, Size size) async {
  final overflows = useStrictSurface(tester, size);
  final session = newSession(newMockClient(fresh: true));
  await pumpApp(tester, session);
  await pumpSteps(tester, steps: 6);
  expect(find.byType(OnboardingIdentityScreen), findsOneWidget);
  return (session: session, overflows: overflows);
}

void main() {
  for (final size in const [Size(360, 800), Size(360, 640)]) {
    testWidgets(
        'identity step holds at ${size.width}x${size.height} '
        'in en and fr with zero overflows', (tester) async {
      final ready = await _pumpIdentityStep(tester, size);

      expect(find.text(en.onboardingIdentityTitle), findsOneWidget);
      expect(find.text(en.onboardingCreateIdentity), findsOneWidget);
      // The card hugs the phone width instead of forcing its desktop 420.
      expect(tester.getSize(find.byType(OnboardingCard)).width,
          lessThanOrEqualTo(size.width));
      expect(ready.overflows, isEmpty);

      // Live locale switch — French copy is ~2x wider (the #14 lesson).
      ready.session.prefs.textLocale = 'fr';
      await pumpSteps(tester, steps: 3);
      expect(find.text(fr.onboardingIdentityTitle), findsOneWidget);
      expect(find.text(fr.onboardingCreateIdentity), findsOneWidget);
      expect(ready.overflows, isEmpty);
    });

    testWidgets(
        'rooms step stacks the create/join cards at '
        '${size.width}x${size.height} in en and fr with zero overflows',
        (tester) async {
      final ready = await _pumpIdentityStep(tester, size);
      await tester.tap(find.text(en.onboardingCreateIdentity).hitTestable());
      await pumpSteps(tester, steps: 6);
      expect(find.byType(OnboardingRoomsScreen), findsOneWidget);

      // Stacked, not side by side: create above join, sharing the left edge
      // (the desktop 880px Row of Expandeds cannot fit 360dp).
      final createCard = find.ancestor(
          of: find.text(en.modalCreateRoomTitle),
          matching: find.byType(OnboardingCard));
      final joinCard = find.ancestor(
          of: find.text(en.modalJoinRoomTitle),
          matching: find.byType(OnboardingCard));
      expect(tester.getTopLeft(createCard).dx, tester.getTopLeft(joinCard).dx);
      expect(tester.getBottomLeft(createCard).dy,
          lessThanOrEqualTo(tester.getTopLeft(joinCard).dy));
      expect(ready.overflows, isEmpty);

      ready.session.prefs.textLocale = 'fr';
      await pumpSteps(tester, steps: 3);
      // findsWidgets: the fr card title and submit label share one catalog
      // value ('Créer un salon').
      expect(find.text(fr.modalCreateRoomTitle), findsWidgets);
      expect(find.text(fr.modalJoinRoomTitle), findsOneWidget);
      expect(ready.overflows, isEmpty);
    });
  }
}
