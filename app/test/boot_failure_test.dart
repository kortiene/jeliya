/// BootStage → copy narration AND the resilient failure surface (issue #71,
/// P29/P30): the session stores structured failure facts and BootScreen
/// composes localized copy at render time, inside a SafeArea + scroll view, in
/// the order friendly summary → Retry → collapsed technical details, with
/// initial focus on the error heading. These tests pump the real failed branch.
library;

import 'dart:async';

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/screens/boot_screen.dart';
import 'package:jeliya_app/src/screens/shell.dart';
import 'package:jeliya_app/src/session/daemon_session.dart';
import 'package:jeliya_app/src/widgets/buttons.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart';
import 'package:jeliya_protocol/testing.dart';

import 'helpers.dart';

/// A client whose start() throws — drives DaemonSession.start's classified
/// catch without any supervisor.
class _FailingClient extends MockClient {
  _FailingClient(this._error);

  final Object _error;

  @override
  Future<void> start() => throw _error;
}

/// Throws on the FIRST start() (a transient bring-up failure), then delegates
/// to the real mock — the Retry-reaches-ready recovery path.
class _FailOnceClient extends DelegatingClient {
  _FailOnceClient(super.inner);

  bool _failed = false;

  @override
  Future<void> start() {
    if (!_failed) {
      _failed = true;
      return Future<void>.error(SidecarError('transient spawn hiccup'));
    }
    return inner.start();
  }
}

/// The heading's nearest Focus ancestor holds the focus node the screen grabs
/// on failure; hasFocus proves initial focus landed on the summary heading.
bool _headingHasFocus(WidgetTester tester, String headingText) {
  final focus = find
      .ancestor(of: find.text(headingText), matching: find.byType(Focus))
      .first;
  return tester.widget<Focus>(focus).focusNode?.hasFocus ?? false;
}

void main() {
  Future<void> pumpFailed(WidgetTester tester, Object error) async {
    final session = newSession(_FailingClient(error));
    await pumpApp(tester, session);
    await pumpSteps(tester, steps: 5);
  }

  group('classified stage narration', () {
    testWidgets('SidecarError start failure narrates bootDaemonStartFailed',
        (tester) async {
      await pumpFailed(tester, SidecarError('spawn exploded'));
      expect(find.text(en.bootCouldNotStart), findsOneWidget);
      expect(find.text(en.bootDaemonStartFailed), findsOneWidget);
      // Raw exception text is now tucked behind the collapsed disclosure — it
      // must NOT be visible until the user opens Technical details.
      expect(find.textContaining('spawn exploded'), findsNothing);
      await tester.tap(find.textContaining(en.commonTechnicalDetails));
      await tester.pump();
      expect(find.textContaining('spawn exploded'), findsOneWidget);
      expect(find.text(en.commonRetry), findsOneWidget);
    });

    testWidgets('TimeoutException narrates bootDaemonConnectTimeout',
        (tester) async {
      await pumpFailed(tester, TimeoutException('no connect'));
      expect(find.text(en.bootCouldNotStart), findsOneWidget);
      expect(find.text(en.bootDaemonConnectTimeout), findsOneWidget);
    });

    testWidgets('unclassified failures narrate bootFailedGeneric',
        (tester) async {
      await pumpFailed(tester, StateError('what even'));
      expect(find.text(en.bootCouldNotStart), findsOneWidget);
      expect(find.text(en.bootFailedGeneric), findsOneWidget);
    });

    testWidgets('ProtocolMismatchError narrates versions and keeps Retry',
        (tester) async {
      await pumpFailed(tester, ProtocolMismatchError(actual: 9, expected: 1));
      expect(find.text(en.bootCouldNotStart), findsOneWidget);
      expect(find.text(en.bootProtocolMismatch(9, 1)), findsOneWidget);
      expect(find.text(en.commonRetry), findsOneWidget);
    });
  });

  group('resilient surface at 360x640, 200% text', () {
    // The failure surface must stay usable in the tightest supported layout:
    // a short phone height with the largest text scale, in both locales.
    Future<List<String>> pumpFailedStrict(WidgetTester tester,
        {required bool french}) async {
      final overflows = useStrictSurface(tester, const Size(360, 640));
      tester.platformDispatcher.textScaleFactorTestValue = 2.0;
      final session = newSession(_FailingClient(SidecarError('spawn exploded')));
      await pumpApp(tester, session);
      await pumpSteps(tester, steps: 5);
      if (french) {
        session.prefs.textLocale = 'fr';
        await pumpSteps(tester, steps: 3);
      }
      return overflows;
    }

    for (final french in const [false, true]) {
      final label = french ? 'fr' : 'en';
      testWidgets(
          'summary + Retry reachable, ordered before hidden details, focus on '
          'heading ($label)', (tester) async {
        final overflows = await pumpFailedStrict(tester, french: french);
        final s = french ? fr : en;

        // Friendly summary and Retry are both present and on-screen (the
        // scroll view guarantees reachability — nothing clipped off-frame).
        final heading = find.text(s.bootCouldNotStart);
        final retry = find.widgetWithText(JeliyaButton, s.commonRetry);
        expect(heading, findsOneWidget);
        expect(retry.hitTestable(), findsOneWidget);
        expect(find.text(s.bootDaemonStartFailed), findsOneWidget);

        // Retry precedes the collapsed Technical details disclosure.
        final details = find.textContaining(s.commonTechnicalDetails);
        expect(details, findsOneWidget);
        expect(tester.getTopLeft(retry).dy,
            lessThan(tester.getTopLeft(details).dy),
            reason: 'Retry must come before the technical-details toggle');

        // Technical text stays hidden until the disclosure is expanded.
        expect(find.textContaining('spawn exploded'), findsNothing);

        // Initial focus is on the summary heading.
        expect(_headingHasFocus(tester, s.bootCouldNotStart), isTrue,
            reason: 'initial focus must land on the error heading');

        expect(overflows, isEmpty,
            reason: 'zero overflows expected on the boot-failure surface at '
                '360x640, textScale 2.0 ($label):\n${overflows.join('\n')}');
      });
    }

    testWidgets('expanding the disclosure reveals the raw technical text',
        (tester) async {
      await pumpFailedStrict(tester, french: false);
      expect(find.textContaining('spawn exploded'), findsNothing);
      await tester.tap(find.textContaining(en.commonTechnicalDetails));
      await tester.pump();
      expect(find.textContaining('spawn exploded'), findsOneWidget);
    });
  });

  testWidgets(
      'a start-failure records NO diagnostic — bootTechnical never leaks into '
      'the diagnostics event', (tester) async {
    final session =
        newSession(_FailingClient(SidecarError('/home/alex/secret spawn')));
    await pumpApp(tester, session);
    await pumpSteps(tester, steps: 5);
    expect(session.boot, Boot.failed);
    // The raw technical string renders (behind the disclosure) but is NEVER
    // recorded as a DiagnosticEvent, so no path can leak through diagnostics.
    expect(session.lastDiagnosticError, isNull);
    expect(session.buildDiagnosticsReport(), isNot(contains('/home/alex')));
  });

  testWidgets('Retry after a transient failure reaches the ready shell',
      (tester) async {
    useDesktopSurface(tester);
    final session = newSession(_FailOnceClient(newMockClient()));
    await pumpApp(tester, session);
    await pumpSteps(tester, steps: 5);
    expect(session.boot, Boot.failed);
    expect(find.byType(BootScreen), findsOneWidget);

    await tester.tap(find.text(en.commonRetry));
    await pumpSteps(tester);
    expect(session.phase, BootstrapPhase.ready);
    expect(find.byType(ShellScreen), findsOneWidget);
    expect(find.byType(BootScreen), findsNothing);
  });
}
