/// Shared widget-test plumbing: a desktop-sized surface, a [DaemonSession]
/// over the package [MockClient] (no supervisor, no disk, no live-activity
/// timers), deterministic pump helpers for the mock's fixed latencies, and
/// small delegating [Client] wrappers that let one test bend exactly one
/// behavior (connection states, push ordering, send failures) without
/// re-implementing any protocol logic.
library;

import 'dart:async';

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter/rendering.dart' show DiagnosticsDebugCreator;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/main.dart';
import 'package:jeliya_app/src/l10n/strings_context.dart';
import 'package:jeliya_app/src/session/daemon_session.dart';
import 'package:jeliya_app/src/session/prefs_store.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart';
import 'package:jeliya_protocol/testing.dart';

/// A comfortable desktop surface (the shipping window minimum is 960x620) so
/// the 3-column shell lays out without overflow errors and long timelines
/// build enough rows to assert against.
void useDesktopSurface(WidgetTester tester,
    {Size size = const Size(1440, 900)}) {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1.0;
  // The bundled test font renders every glyph ~1em wide (roughly 2.5x a real
  // UI font), which overflows the pixel-tuned desktop layout. Halving the
  // text scale restores realistic text widths without touching any layout.
  tester.platformDispatcher.textScaleFactorTestValue = 0.5;
  addTearDown(tester.platformDispatcher.clearAllTestValues);
  addTearDown(tester.view.reset);
  _tolerateOverflows();
}

/// Like [useDesktopSurface] but at an explicit REALISTIC textScale 1.0 (the
/// 0.5 default masked the sidebar collapse and the PR-#14 French-width
/// regressions) and WITHOUT the blanket overflow tolerance. The oversized
/// test font (~1em-wide glyphs) does overflow some pixel-tuned desktop rows
/// at full scale, so overflow reports are RECORDED here instead of swallowed
/// and each test decides what to fail on — desktop tests filter to the
/// widget under test; NEW mobile layouts assert the list is EMPTY outright.
/// Attribution is rendered EAGERLY at report time: the reporting element is
/// still mounted then, while lazily rendering the details at assertion time
/// walks ancestors that boot-phase rebuilds have already deactivated.
///
/// Teardown order: the registered [TestPlatformDispatcher.clearAllTestValues]
/// wipes EVERY platform test value, so set locale test values
/// (localesTestValue/localeTestValue) AFTER installing the surface (the
/// locale_test.dart precedent).
List<String> useStrictSurface(WidgetTester tester, Size size) {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1.0;
  tester.platformDispatcher.textScaleFactorTestValue = 1.0;
  addTearDown(tester.platformDispatcher.clearAllTestValues);
  addTearDown(tester.view.reset);

  final overflows = <String>[];
  final prior = FlutterError.onError;
  FlutterError.onError = (details) {
    final summary = details.exceptionAsString().split('\n').first;
    if (summary.contains('overflowed by')) {
      final creator = details.informationCollector
          ?.call()
          .whereType<DiagnosticsDebugCreator>()
          .firstOrNull
          ?.value;
      final chain = creator is DebugCreator
          ? creator.element.debugGetCreatorChain(4)
          : 'unknown creator';
      overflows.add('$summary in $chain');
      return;
    }
    prior?.call(details);
  };
  addTearDown(() => FlutterError.onError = prior);
  return overflows;
}

/// Swallows RenderFlex *overflow* reports only — even at half scale the test
/// font (~1em-wide glyphs) overflows a few pixel-tuned rows (e.g. the member
/// row's "this device" chip). Every other exception still fails the test;
/// real layout crashes (infinite constraints, missing sizes) stay fatal.
void _tolerateOverflows() {
  final prior = FlutterError.onError;
  FlutterError.onError = (details) {
    if (details.exceptionAsString().contains('overflowed by')) {
      return;
    }
    prior?.call(details);
  };
  addTearDown(() => FlutterError.onError = prior);
}

/// A timer-free mock: no live-activity simulation (widget tests own the
/// clock); fixed latencies stay (connect 200ms, calls 60ms, fetch 900ms).
MockClient newMockClient({bool fresh = false}) =>
    MockClient(fresh: fresh, simulateLiveActivity: false);

/// A session over an injected [client]: the sidecar supervisor is skipped
/// entirely and prefs stay in memory (no disk I/O). Disposed with the test.
/// The en catalog for copy assertions — tests assert `find.text(en.<key>)`
/// so translation work never breaks them (docs/i18n.md rule 6).
final AppStrings en = lookupAppStrings(const Locale('en'));

/// The fr catalog, for locale-resolution and French-contract assertions.
final AppStrings fr = lookupAppStrings(const Locale('fr'));

DaemonSession newSession(Client client, {PrefsStore? prefs}) {
  final session =
      DaemonSession(client: client, prefs: prefs ?? PrefsStore.inMemory());
  addTearDown(session.dispose);
  return session;
}

/// Mounts the real app entry ([JeliyaApp]) over [session]. Keyed uniquely so
/// pumping a second app in the same test gets a fresh element (and therefore
/// actually starts the new session) instead of reusing the old state.
Future<void> pumpApp(WidgetTester tester, DaemonSession session) =>
    tester.pumpWidget(JeliyaApp(key: UniqueKey(), session: session));

/// Advances fake time in fixed steps — enough for the mock connect handshake
/// (200ms) plus the whole bootstrap ladder of 60ms calls. `pumpAndSettle` is
/// deliberately avoided everywhere: spinners and the reconnect pulse dot
/// animate forever, so it would never settle.
Future<void> pumpSteps(WidgetTester tester,
    {int steps = 20, Duration step = const Duration(milliseconds: 100)}) async {
  for (var i = 0; i < steps; i++) {
    await tester.pump(step);
  }
}

/// Boots the app over [client] all the way to the ready shell.
Future<DaemonSession> pumpReadyApp(WidgetTester tester, Client client,
    {PrefsStore? prefs}) async {
  useDesktopSurface(tester);
  final session = newSession(client, prefs: prefs);
  await pumpApp(tester, session);
  await pumpSteps(tester);
  expect(session.phase, BootstrapPhase.ready,
      reason: 'bootstrap should reach the ready shell');
  return session;
}

/// Boots the app over [client] to the ready shell on a STRICT phone surface
/// (default 360x800 portrait): textScale 1.0, DPR 1.0, overflow reports
/// recorded, not swallowed. Returns the session AND the recorded overflow
/// list — mobile layouts are new (never pixel-tuned to the fat test font),
/// so mobile tests assert the list stays EMPTY outright.
Future<({DaemonSession session, List<String> overflows})> pumpReadyMobileApp(
    WidgetTester tester, Client client,
    {Size size = const Size(360, 800), PrefsStore? prefs}) async {
  final overflows = useStrictSurface(tester, size);
  final session = newSession(client, prefs: prefs);
  await pumpApp(tester, session);
  await pumpSteps(tester);
  expect(session.phase, BootstrapPhase.ready,
      reason: 'bootstrap should reach the ready shell');
  return (session: session, overflows: overflows);
}

/// Delegates every [Client] member to [inner]; subclasses bend one seam.
class DelegatingClient implements Client {
  DelegatingClient(this.inner);

  final MockClient inner;

  @override
  Future<void> start() => inner.start();

  @override
  Future<void> stop() => inner.stop();

  @override
  ConnectionState get state => inner.state;

  @override
  Stream<ConnectionState> get states => inner.states;

  @override
  Stream<Push> get pushes => inner.pushes;

  @override
  Future<dynamic> call(String method, [Map<String, dynamic>? params]) =>
      inner.call(method, params);

  @override
  String describe() => inner.describe();
}

/// Lets a test drive [ConnectionState] transitions by hand — the mock only
/// walks connecting→connected→disconnected on its own, but the banner/badge
/// contract also has a 'reconnecting' state.
class ConnectionFakeClient extends DelegatingClient {
  ConnectionFakeClient(super.inner) {
    inner.states.listen(_emit);
  }

  final StreamController<ConnectionState> _states =
      StreamController<ConnectionState>.broadcast();
  ConnectionState _state = ConnectionState.disconnected;

  void setConnection(ConnectionState next) => _emit(next);

  void _emit(ConnectionState next) {
    _state = next;
    _states.add(next);
  }

  @override
  ConnectionState get state => _state;

  @override
  Stream<ConnectionState> get states => _states.stream;
}

/// Buffers pushes while [hold] is true so a test can observe the
/// response-before-echo pending phase ('Sent locally, syncing...') that the
/// mock's echo-beats-response ordering otherwise skips straight past.
class HeldPushClient extends DelegatingClient {
  HeldPushClient(super.inner) {
    inner.pushes.listen((push) {
      if (hold) {
        _held.add(push);
      } else {
        _pushes.add(push);
      }
    });
  }

  final StreamController<Push> _pushes = StreamController<Push>.broadcast();
  final List<Push> _held = [];
  bool hold = false;

  /// Stop holding and deliver everything buffered, in order.
  void release() {
    hold = false;
    for (final push in _held) {
      _pushes.add(push);
    }
    _held.clear();
  }

  @override
  Stream<Push> get pushes => _pushes.stream;
}

/// Fails the next [failures] `message.send` calls with a typed
/// [RequestError], then behaves normally — drives the pending message's
/// failed → Retry path.
class FlakySendClient extends DelegatingClient {
  FlakySendClient(super.inner, {this.failures = 1});

  int failures;

  @override
  Future<dynamic> call(String method, [Map<String, dynamic>? params]) {
    if (method == 'message.send' && failures > 0) {
      failures -= 1;
      return Future<dynamic>.delayed(
        inner.callLatency,
        () => throw RequestError('internal', 'mock send failure',
            hint: 'try again'),
      );
    }
    return inner.call(method, params);
  }
}

/// Masks the identity off the FIRST `daemon.status` so the identity
/// onboarding step renders against a daemon that already has one — the race
/// where `identity.create` fails with `identity_exists` and the client must
/// treat that as success.
class IdentityMaskClient extends DelegatingClient {
  IdentityMaskClient(super.inner);

  bool _masked = false;

  @override
  Future<dynamic> call(String method, [Map<String, dynamic>? params]) async {
    final result = await inner.call(method, params);
    if (method == 'daemon.status' && !_masked) {
      _masked = true;
      return <String, dynamic>{
        ...result as Map<String, dynamic>,
        'identity': null,
        'endpoint': null,
      };
    }
    return result;
  }
}
