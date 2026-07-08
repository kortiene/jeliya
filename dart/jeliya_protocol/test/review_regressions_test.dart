// Regression tests for the Phase 3 adversarial-review fixes:
// - WsClient survives malformed / forward-compat frames (a daemon frame must
//   never crash the client or leave a call hanging) and describe() renders no
//   trailing '?'.
// - SidecarSupervisor process-handle discipline: a re-entrant start() that
//   adopts our own child keeps the SIGTERM lever; an adopting supervisor holds
//   NO handle so stopDaemon verifies real death via the health check.
// - MockClient honors the stopped-client contract (connection_lost).

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:jeliya_protocol/jeliya_protocol.dart';
import 'package:jeliya_protocol/testing.dart';
import 'package:test/test.dart';

Directory _repoRoot() {
  var dir = Directory.current;
  for (var i = 0; i < 8; i++) {
    if (File('${dir.path}/target/debug/jeliyad').existsSync()) return dir;
    final parent = dir.parent;
    if (parent.path == dir.path) break;
    dir = parent;
  }
  throw StateError('could not locate repo root from ${Directory.current.path}');
}

Matcher _throwsCode(String code) =>
    throwsA(isA<RequestError>().having((e) => e.code, 'code', code));

void main() {
  group('WsClient frame tolerance', () {
    late HttpServer server;
    final sockets = <WebSocket>[];

    setUp(() async {
      server = await HttpServer.bind('127.0.0.1', 0);
    });

    tearDown(() async {
      for (final ws in sockets) {
        await ws.close();
      }
      sockets.clear();
      await server.close(force: true);
    });

    test('malformed and forward-compat frames never crash or hang the client',
        () async {
      server.listen((req) async {
        final ws = await WebSocketTransformer.upgrade(req);
        sockets.add(ws);
        ws.listen((data) {
          final frame = jsonDecode(data as String) as Map<String, dynamic>;
          // Garbage first — every one of these crashed or hung the client
          // before the fix — then a (malformed-error) reply for the call.
          ws.add('not json at all');
          ws.add(jsonEncode([1, 2, 3]));
          ws.add(jsonEncode({'push': 'future.push', 'data': [1, 2, 3]}));
          ws.add(jsonEncode({'push': 'future.push', 'data': 'a string'}));
          ws.add(jsonEncode({'id': 'not-an-int', 'ok': true}));
          ws.add(jsonEncode({
            'id': frame['id'],
            'ok': false,
            'error': {'code': 42, 'message': null},
          }));
        });
      });

      final client = WsClient(() async => Uri.parse('ws://127.0.0.1:${server.port}/'));
      await client.start();
      final pushes = <Push>[];
      final sub = client.pushes.listen(pushes.add);

      // The mistyped error object still completes the call — as `internal`.
      await expectLater(client.call('anything'), _throwsCode('internal'));

      // The unknown pushes were delivered with an empty payload, not dropped
      // silently and not thrown (ignore-unknown forward-compat rule).
      await Future<void>.delayed(const Duration(milliseconds: 100));
      expect(pushes.where((p) => p.name == 'future.push').length, 2);
      for (final p in pushes) {
        expect(p.data, isEmpty);
      }
      await sub.cancel();
      await client.stop();
    });

    test('describe() renders the transport URL without a trailing ?', () async {
      server.listen((req) async {
        sockets.add(await WebSocketTransformer.upgrade(req));
      });
      final client = WsClient(
          () async => Uri.parse('ws://127.0.0.1:${server.port}/ws?token=sekrit'));
      await client.start();
      expect(client.describe(), 'ws://127.0.0.1:${server.port}/ws');
      expect(client.describe(), isNot(contains('?')));
      expect(client.describe(), isNot(contains('sekrit')));
      await client.stop();
    });
  });

  group('MockClient stopped-client contract', () {
    MockClient newMock() => MockClient(
          simulateLiveActivity: false,
          connectLatency: const Duration(milliseconds: 5),
          callLatency: const Duration(milliseconds: 20),
        );

    test('a call issued after stop() fails with connection_lost', () async {
      final mock = newMock();
      await mock.start();
      await mock.stop();
      await expectLater(mock.call('daemon.status'), _throwsCode('connection_lost'));
    });

    test('a never-started mock refuses calls with connection_lost', () async {
      await expectLater(newMock().call('daemon.status'), _throwsCode('connection_lost'));
    });

    test('an in-flight call fails with connection_lost at stop()', () async {
      final mock = newMock();
      await mock.start();
      final inFlight = mock.call('daemon.status');
      // Attach the expectation BEFORE stop() so the error is never unhandled.
      final expectation = expectLater(inFlight, _throwsCode('connection_lost'));
      await mock.stop();
      await expectation;
    });
  });

  group('supervisor process-handle discipline (live daemon)', () {
    final binary = '${_repoRoot().path}/target/debug/jeliyad';
    if (!File(binary).existsSync()) {
      test('(daemon binary missing)', () {}, skip: 'jeliyad not built');
      return;
    }

    late Directory dataDir;

    setUp(() {
      dataDir = Directory.systemTemp.createTempSync('jeliya-regress-');
    });

    tearDown(() {
      try {
        dataDir.deleteSync(recursive: true);
      } catch (_) {/* best effort */}
    });

    test('re-entrant start() adopts our own child and keeps the SIGTERM lever',
        () async {
      final sup = SidecarSupervisor(
          binaryPath: binary, dataDir: dataDir.path, loopback: true);
      final r1 = await sup.start(port: 0);
      expect(r1.adopted, isFalse);

      // A Retry that raced a healthy daemon: the probe adopts our own child.
      final r2 = await sup.start(port: 0);
      expect(r2.adopted, isTrue);
      expect(r2.pid, r1.pid);

      // Before the fix the exited probe replaced the child handle and this
      // shutdown() signalled a corpse, orphaning the daemon.
      await sup.shutdown();
      expect(await sup.healthCheck(), isFalse,
          reason: 'shutdown() must still reach the owned child after a re-entrant adopt');
    });

    test('adopting supervisor holds no handle; stopDaemon verifies real death',
        () async {
      final owner = SidecarSupervisor(
          binaryPath: binary, dataDir: dataDir.path, loopback: true);
      await owner.start(port: 0);

      final adopter = SidecarSupervisor(
          binaryPath: binary, dataDir: dataDir.path, loopback: true);
      final adopted = await adopter.start(port: 0);
      expect(adopted.adopted, isTrue);

      // No process handle: a handle-less shutdown() must NOT kill the
      // incumbent it never owned.
      await adopter.shutdown();
      expect(await adopter.healthCheck(), isTrue,
          reason: 'an adopter must not signal a process it does not own');

      // The RPC path is the adopted daemon's one lever — and it must verify
      // actual death rather than trusting the reply.
      final client = WsClient(adopter.wsUrl);
      await client.start();
      try {
        await adopter.stopDaemon(client);
      } finally {
        await client.stop();
      }
      expect(await adopter.healthCheck(), isFalse);
      await owner.shutdown(); // reap the exited child handle
    });
  });
}
