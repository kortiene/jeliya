// The Phase 4 crux, third oracle: the in-process FfiClient is held to the
// EXACT same golden corpus as the daemon and mock oracles. Loads the host
// build of the engine library (target/debug/libjeliya_ffi.{dylib,so}),
// starts the engine over a temp data dir, and replays
// ui/src/lib/conformance/corpus.json through the UNCHANGED replay harness —
// proving the FFI transport speaks the same envelope as the WebSocket seam.
//
//   cargo build -p jeliya-ffi   (from the repo root)
//   dart test                   (from dart/jeliya_protocol/)
//
// Skips with a clear reason if the engine library is not built.
//
// EVERY test that touches the engine lives in this one file: the engine
// singleton is per-PROCESS while `dart test` runs suites as isolates sharing
// one VM process — a second file starting an engine over its own data dir
// would race this one into DATA_DIR_MISMATCH. Within a file, groups run
// sequentially, so each start/stop cycle below owns the singleton outright.

import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import 'package:jeliya_protocol/ffi.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart';
import 'package:test/test.dart';

/// Walk up from the current dir to the repo root (the dir containing
/// `ui/src/lib/conformance/corpus.json`) — a CHECKED-IN marker, never a built
/// artifact, so an unbuilt checkout still resolves far enough to skip loudly.
Directory _repoRoot() {
  var dir = Directory.current;
  for (var i = 0; i < 8; i++) {
    if (File('${dir.path}/ui/src/lib/conformance/corpus.json').existsSync()) return dir;
    final parent = dir.parent;
    if (parent.path == dir.path) break;
    dir = parent;
  }
  throw StateError('could not locate repo root from ${Directory.current.path}');
}

/// The host `cdylib` name `cargo build -p jeliya-ffi` produces per platform.
String _dylibName() {
  if (Platform.isMacOS) return 'libjeliya_ffi.dylib';
  if (Platform.isWindows) return 'jeliya_ffi.dll';
  return 'libjeliya_ffi.so';
}

Matcher _throwsCode(String code) =>
    throwsA(isA<RequestError>().having((e) => e.code, 'code', code));

void main() {
  final repo = _repoRoot();
  final dylib = '${repo.path}/target/debug/${_dylibName()}';
  final corpusFile = File('${repo.path}/ui/src/lib/conformance/corpus.json');

  if (!File(dylib).existsSync()) {
    test('ffi conformance (engine library missing)', () {
      // A silent skip on a conformance suite reads as green while covering
      // nothing; mark it explicitly.
      markTestSkipped('engine library not built at $dylib — run `cargo build -p jeliya-ffi`');
    }, skip: 'libjeliya_ffi not built (run `cargo build -p jeliya-ffi`)');
    return;
  }

  FfiClient newClient(String dataDir) =>
      // loopback satisfies the corpus's mode:'loopback' pin, exactly like the
      // daemon oracle's supervisor flag.
      FfiClient(dataDir: dataDir, loopback: true, open: () => DynamicLibrary.open(dylib));

  final corpus = jsonDecode(corpusFile.readAsStringSync()) as Map<String, dynamic>;
  final scenarios = (corpus['scenarios'] as List)
      .cast<Map<String, dynamic>>()
      // mockOnly = daemon.shutdown, which really tears down the shared engine
      // (the shim's honesty contract) — same reason the daemon run skips it.
      .where((s) => !((s['tags'] as List?)?.contains('mockOnly') ?? false))
      .toList();

  group('conformance: FfiClient vs in-process engine', () {
    late FfiClient client;
    late Directory dataDir;
    late List<StepResult> preIdentityResults;

    setUpAll(() async {
      dataDir = Directory.systemTemp.createTempSync('jeliya-ffi-conf-');
      client = newClient(dataDir.path);
      await client.start().timeout(const Duration(seconds: 10));
      final preIdentity = scenarios.singleWhere(
          (s) => (s['tags'] as List?)?.contains('preIdentity') ?? false);
      preIdentityResults = await replayScenario(client, preIdentity, pushWaitMs: 3000);
      // Establish the shared precondition only after the identity-free corpus
      // vector has crossed the singleton in-process engine.
      await client.call('identity.create');
    });

    tearDownAll(() async {
      await client.stop();
      try {
        dataDir.deleteSync(recursive: true);
      } catch (_) {}
    });

    for (final scenario in scenarios) {
      test(scenario['name'] as String, () async {
        final preIdentity = (scenario['tags'] as List?)?.contains('preIdentity') ?? false;
        final results = preIdentity
            ? preIdentityResults
            : await replayScenario(client, scenario, pushWaitMs: 3000);
        final failures = results.where((r) => !r.ok).toList();
        expect(failures.map((f) => 'step ${f.step} (${f.method}): ${f.detail}').toList(), isEmpty);
      });
    }
  });

  // The corpus-invisible Client contract, mirroring the pins WsClient and
  // MockClient carry in their own tests (test/review_regressions_test.dart,
  // test/mock_client_test.dart) — these are prose-normative in PROTOCOL.md,
  // not corpus vectors, so the FFI transport needs its own copies.
  group('FfiClient contract', () {
    late FfiClient client;
    late Directory dataDir;

    setUp(() {
      dataDir = Directory.systemTemp.createTempSync('jeliya-ffi-contract-');
      client = newClient(dataDir.path);
    });

    tearDown(() async {
      await client.stop(); // idempotent; frees the engine singleton for the next test
      try {
        dataDir.deleteSync(recursive: true);
      } catch (_) {}
    });

    test('start walks connecting → connected; stop lands disconnected; never reconnecting',
        () async {
      final seen = <ConnectionState>[];
      final sub = client.states.listen(seen.add);
      expect(client.state, ConnectionState.disconnected);
      await client.start();
      expect(client.state, ConnectionState.connected);
      await client.stop();
      expect(client.state, ConnectionState.disconnected);
      await Future<void>.delayed(Duration.zero);
      // No transport exists that can drop independently of the process, so
      // reconnecting would be a fabricated state (it renders in Settings).
      expect(seen, [
        ConnectionState.connecting,
        ConnectionState.connected,
        ConnectionState.disconnected,
      ]);
      await sub.cancel();
    });

    test('start is idempotent and restartable after stop (rebind path)', () async {
      await Future.wait([client.start(), client.start()]);
      expect(client.state, ConnectionState.connected);
      await client.stop();
      await client.start();
      expect(client.state, ConnectionState.connected);
      expect(await client.call('daemon.status'), isA<Map<dynamic, dynamic>>());
    });

    test('stop() right after start() leaves both settled — an awaiter never hangs', () async {
      final starting = client.start();
      final stopping = client.stop();
      await starting.timeout(const Duration(seconds: 10));
      await stopping.timeout(const Duration(seconds: 15));
      expect(client.state, ConnectionState.disconnected);
    });

    test('start() interleaved with a still-awaiting stop() leaves a usable client', () async {
      await client.start();
      // stop() runs synchronously up to its done-port await (the host slot
      // is emptied by then), so the restart below overlaps the teardown
      // wait — the reviewed wedge: stop's tail must not close the restarted
      // client's fresh frames port or stamp disconnected over its state.
      final stopping = client.stop();
      await client.start().timeout(const Duration(seconds: 10));
      expect(client.state, ConnectionState.connected);
      await stopping.timeout(const Duration(seconds: 15));
      expect(client.state, ConnectionState.connected);
      final status = await client.call('daemon.status').timeout(const Duration(seconds: 10));
      expect((status as Map)['protocol'], 1);
    });

    test('engine death via daemon.shutdown is observed: calls fail, state drops', () async {
      await client.start();
      expect(await client.call('daemon.shutdown'), {'shutting_down': true});
      // The reply lands first; real teardown follows a beat later. Poll until
      // the host reports the engine gone — the client must surface that as
      // connection_lost AND stop rendering `connected` against a dead engine.
      final deadline = DateTime.now().add(const Duration(seconds: 10));
      while (true) {
        try {
          await client.call('daemon.status').timeout(const Duration(seconds: 10));
        } on RequestError catch (e) {
          expect(e.code, 'connection_lost');
          break;
        }
        if (!DateTime.now().isBefore(deadline)) {
          fail('engine never tore down after daemon.shutdown');
        }
        await Future<void>.delayed(const Duration(milliseconds: 50));
      }
      expect(client.state, ConnectionState.disconnected);
      // Observed death folds into the stopped state: restartable like stop().
      await client.start().timeout(const Duration(seconds: 10));
      expect(client.state, ConnectionState.connected);
      final status = await client.call('daemon.status').timeout(const Duration(seconds: 10));
      expect((status as Map)['protocol'], 1);
    });

    test('a call issued after stop() fails fast with connection_lost', () async {
      await client.start();
      await client.stop();
      await expectLater(client.call('daemon.status'), _throwsCode('connection_lost'));
    });

    test('a never-started client refuses calls with connection_lost', () async {
      await expectLater(client.call('daemon.status'), _throwsCode('connection_lost'));
    });

    test('an in-flight call fails with connection_lost at stop()', () async {
      await client.start();
      final inFlight = client.call('daemon.status');
      // Attach the expectation BEFORE stop() so the error is never unhandled.
      final expectation = expectLater(inFlight, _throwsCode('connection_lost'));
      await client.stop();
      await expectation;
    });

    test('describe() states the truth: in-process engine + data dir', () {
      expect(client.describe(), 'in-process engine (${dataDir.path})');
    });

    test('states and pushes are broadcast streams (DaemonSession listens more than once)',
        () async {
      final s1 = client.states.listen((_) {});
      final s2 = client.states.listen((_) {}); // second listen throws on a single-sub stream
      final p1 = client.pushes.listen((_) {});
      final p2 = client.pushes.listen((_) {});
      for (final sub in [s1, s2, p1, p2]) {
        await sub.cancel();
      }
    });

    test('the error triple {code, message, hint} crosses the port structurally', () async {
      await client.start();
      await client.call('identity.create');
      try {
        await client.call('room.open', {'room_id': 'blake3:${'0' * 64}'});
        fail('room.open on an unknown room must reject');
      } on RequestError catch (e) {
        // A stringified error surface would flatten these to 'internal'.
        expect(e.code, 'room_unknown');
        expect(e.message, isNotEmpty);
        expect(e.hint, isNotNull);
      }
    });

    test('concurrent calls correlate by envelope id over the one frames port', () async {
      await client.start();
      await client.call('identity.create');
      // Distinct result shapes: a mis-correlated reply completes the wrong
      // completer and fails the shape checks below.
      final results = await Future.wait([
        client.call('room.create', {'name': 'Correlation A'}),
        client.call('daemon.status'),
        client.call('room.list'),
        client.call('room.create', {'name': 'Correlation B'}),
      ]);
      final ridA = (results[0] as Map)['room_id'] as String;
      final ridB = (results[3] as Map)['room_id'] as String;
      expect(ridA, startsWith('blake3:'));
      expect(ridB, startsWith('blake3:'));
      expect(ridA, isNot(ridB));
      expect((results[1] as Map)['protocol'], 1);
      expect((results[2] as Map)['rooms'], isA<List<dynamic>>());
    });

    test('echo and response correlate whichever arrives first', () async {
      await client.start();
      await client.call('identity.create');
      final created = await client.call('room.create', {'name': 'Echo Order Room'}) as Map;
      final rid = created['room_id'] as String;
      await client.call('room.open', {'room_id': rid});

      final echoId = Completer<String>();
      final sub = client.pushes.listen((p) {
        final event = p.data['event'];
        if (p.name == 'room.event' && event is Map && !echoId.isCompleted) {
          echoId.complete(event['event_id'] as String);
        }
      });
      final result = await client.call('message.send', {'room_id': rid, 'body': 'ordering probe'});
      // PROTOCOL.md ordering hazard 2: EITHER order is conformant. MockClient
      // pins echo-first and the daemon usually responds first; the FFI port
      // hop makes the order genuinely racy, so assert only the invariant that
      // holds in both orders — one echo push whose event_id matches the
      // response's. Reconciliation above the seam depends on exactly this.
      final echoed = await echoId.future.timeout(const Duration(seconds: 3));
      expect(echoed, (result as Map)['event_id']);
      await sub.cancel();
    });
  });
}
