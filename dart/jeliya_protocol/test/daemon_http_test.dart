// Exercises the supervisor's HTTP surface against a REAL daemon: full
// portfile parse, /api/health adoption checks, attach-without-spawn, the
// post-connect protocol re-check, the native staged-upload convention
// (docs/PROTOCOL.md, "Native file sharing"), the /api/files/local URL
// builder, and the rpc-based stop for adopted daemons.

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:jeliya_protocol/jeliya_protocol.dart';
import 'package:test/test.dart';

/// Walk up to the repo root by a CHECKED-IN marker (docs/PROTOCOL.md), not
/// the built binary: on an unbuilt checkout (CI, fresh clone) the root must
/// still resolve so the `jeliyad not built` skip guard below can run —
/// using the binary as the marker made loading THROW instead of skipping.
Directory _repoRoot() {
  var dir = Directory.current;
  for (var i = 0; i < 8; i++) {
    if (File('${dir.path}/docs/PROTOCOL.md').existsSync()) return dir;
    final parent = dir.parent;
    if (parent.path == dir.path) break;
    dir = parent;
  }
  throw StateError('could not locate repo root from ${Directory.current.path}');
}

/// A [Client] whose `daemon.status` reports a fixed protocol — the "reconnect
/// silently attached to a different daemon build" case, without a build.
class _FixedProtocolClient implements Client {
  _FixedProtocolClient(this.protocol);
  final int protocol;

  @override
  Future<dynamic> call(String method, [Map<String, dynamic>? params]) async {
    if (method == 'daemon.status') {
      return {
        'version': '9.9.9',
        'protocol': protocol,
        'pid': 1,
        'port': 1,
        'data_dir': '/nowhere',
        'mode': 'loopback',
      };
    }
    throw RequestError(ErrorCodes.internal, 'unexpected method $method');
  }

  @override
  Future<void> start() async {}
  @override
  Future<void> stop() async {}
  @override
  ConnectionState get state => ConnectionState.connected;
  @override
  Stream<ConnectionState> get states => const Stream.empty();
  @override
  Stream<Push> get pushes => const Stream.empty();
  @override
  String describe() => 'fixed-protocol test client';
}

void main() {
  final binary = '${_repoRoot().path}/target/debug/jeliyad';

  if (!File(binary).existsSync()) {
    test('daemon http surface (daemon binary missing)', () {}, skip: 'jeliyad not built');
    return;
  }

  group('against a live daemon', () {
    late Directory dataDir;
    late Directory scratch; // deliberately OUTSIDE the data dir
    late SidecarSupervisor owner;
    late Ready ready;
    late WsClient client;
    late Identity identity;
    late String roomId;

    /// Whatever survived in `<data_dir>/uploads` — must always be empty.
    List<FileSystemEntity> stagedLeftovers() {
      final uploads = Directory('${dataDir.path}/uploads');
      return uploads.existsSync() ? uploads.listSync() : const [];
    }

    setUpAll(() async {
      dataDir = Directory.systemTemp.createTempSync('jeliya-http-');
      scratch = Directory.systemTemp.createTempSync('jeliya-http-src-');
      owner = SidecarSupervisor(binaryPath: binary, dataDir: dataDir.path, loopback: true);
      ready = await owner.start(port: 0);
      client = WsClient(owner.wsUrl);
      await client.start();
      identity = await client.identityCreate();
      roomId = await client.roomCreate('HTTP surface');
      await client.roomOpen(roomId);
    });

    tearDownAll(() async {
      // Owned-daemon stopDaemon path: the RPC lands, then converges on the
      // supervised SIGTERM wait — and the daemon must actually be gone.
      await owner.stopDaemon(client);
      expect(await owner.healthCheck(), isFalse);
      await client.stop();
      for (final dir in [dataDir, scratch]) {
        try {
          dir.deleteSync(recursive: true);
        } catch (_) {}
      }
    });

    test('the portfile parses the full schema-1 shape', () {
      final pf = owner.readPortfile();
      expect(pf, isNotNull);
      expect(pf!.schema, 1);
      expect(pf.port, ready.port);
      expect(pf.pid, ready.pid);
      expect(pf.protocol, SidecarSupervisor.expectedProtocol);
      expect(RegExp(r'^[0-9a-f]{64}$').hasMatch(pf.authToken), isTrue);
      expect(pf.dataDir, isNotEmpty);
      expect(Uri.parse(pf.http!).port, ready.port);
      expect(Uri.parse(pf.ws!).path, '/ws');
      expect(pf.version, isNotEmpty);
      expect(pf.startedAtMs, greaterThan(0));
    });

    test('healthCheck confirms the running daemon and rejects an empty dir', () async {
      expect(await owner.healthCheck(), isTrue);
      final empty = Directory.systemTemp.createTempSync('jeliya-http-empty-');
      addTearDown(() => empty.deleteSync(recursive: true));
      final blind = SidecarSupervisor.attach(dataDir: empty.path);
      expect(await blind.healthCheck(), isFalse);
    });

    test('httpBase and authToken come from the portfile', () {
      final pf = owner.readPortfile()!;
      final base = owner.httpBase();
      expect(base.scheme, 'http');
      expect(base.port, ready.port);
      expect(owner.authToken, pf.authToken);
    });

    test('verifyDaemonProtocol passes the live daemon, hard-fails a skewed one', () async {
      final status = await verifyDaemonProtocol(client);
      expect(status.protocol, SidecarSupervisor.expectedProtocol);
      expect(status.pid, ready.pid);

      await expectLater(
        verifyDaemonProtocol(_FixedProtocolClient(99)),
        throwsA(isA<ProtocolMismatchError>()
            .having((e) => e.actual, 'actual', 99)
            .having((e) => e.expected, 'expected', SidecarSupervisor.expectedProtocol)),
      );
    });

    test('attach-without-spawn adopts the incumbent with no binary', () async {
      final attached = SidecarSupervisor.attach(dataDir: dataDir.path);
      final adopted = await attached.attachToRunning();
      expect(adopted.adopted, isTrue);
      expect(adopted.pid, ready.pid);
      expect(adopted.port, ready.port);
      expect(attached.ready, same(adopted));
      expect((await attached.wsUrl()).port, ready.port);
      // shutdown() has no process to signal on an attach-only supervisor; it
      // must be a harmless no-op (the incumbent stays up).
      await attached.shutdown();
      expect(await owner.healthCheck(), isTrue);
    });

    test('attachToRunning refuses a protocol-skewed daemon before adopting', () async {
      // A healthy daemon answering the port, but a portfile protocol we were
      // not built against: adopt-vs-respawn says refuse loudly.
      final skewedDir = Directory.systemTemp.createTempSync('jeliya-http-skew-');
      addTearDown(() => skewedDir.deleteSync(recursive: true));
      final pf = jsonDecode(File('${dataDir.path}/daemon.json').readAsStringSync())
          as Map<String, dynamic>;
      pf['protocol'] = 99;
      File('${skewedDir.path}/daemon.json').writeAsStringSync(jsonEncode(pf));
      final attached = SidecarSupervisor.attach(dataDir: skewedDir.path);
      await expectLater(
        attached.attachToRunning(),
        throwsA(isA<ProtocolMismatchError>().having((e) => e.actual, 'actual', 99)),
      );
      expect(attached.ready, isNull);
    });

    test('staged upload end-to-end: copy in, share, staged copy gone', () async {
      final body = List.generate(64, (i) => 'staged upload line $i').join('\n');
      final source = File('${scratch.path}/hello.txt')..writeAsStringSync(body);

      final shared = await owner.shareUserFile(
        client,
        roomId: roomId,
        sourcePath: source.path,
        mime: 'text/plain',
      );
      expect(shared.fileId, isNotEmpty);
      expect(shared.eventId, isNotEmpty);
      expect(stagedLeftovers(), isEmpty, reason: 'the staged copy is deleted after file.share');
      expect(source.existsSync(), isTrue, reason: 'the user file is copied, never moved');

      final files = await client.fileList(roomId);
      final entry = files.singleWhere((f) => f.fileId == shared.fileId);
      expect(entry.name, 'hello.txt');
      expect(entry.size, body.length);
      expect(entry.mime, 'text/plain');
      expect(entry.senderId, identity.identityId);
    });

    test('a 250-char legal filename is sanitized and capped like the daemon does',
        () async {
      final longName = '${'x' * 250}.bin';
      final source = File('${scratch.path}/long-src.bin')..writeAsStringSync('bytes');
      final shared = await owner.shareUserFile(
        client,
        roomId: roomId,
        sourcePath: source.path,
        name: 'we:ird/$longName', // unsafe chars + over-long, both legal on disk
      );
      expect(shared.fileId, isNotEmpty);
      expect(stagedLeftovers(), isEmpty);
      final files = await client.fileList(roomId);
      final entry = files.singleWhere((f) => f.fileId == shared.fileId);
      // Basename survives ('/' splits), ':' is mapped to '_', 180-char cap.
      expect(entry.name.length, lessThanOrEqualTo(180));
      expect(entry.name, isNot(contains(':')));
    });

    test('the staged copy is deleted even when file.share fails', () async {
      final source = File('${scratch.path}/orphan.txt')..writeAsStringSync('never shared');
      await expectLater(
        owner.shareUserFile(client, roomId: '${roomId}f00', sourcePath: source.path),
        throwsA(isA<RequestError>()),
      );
      expect(stagedLeftovers(), isEmpty, reason: 'a failed share must not leak staged bytes');
    });

    test('oversized files fail typed, before any copy', () async {
      // A sparse file: one truncate call, no 100 MiB write.
      final big = File('${scratch.path}/big.bin');
      final raf = big.openSync(mode: FileMode.write);
      raf.truncateSync(maxSharedFileBytes + 1);
      raf.closeSync();

      await expectLater(
        stageAndShareFile(
          client,
          dataDir: dataDir.path,
          roomId: roomId,
          sourcePath: big.path,
        ),
        throwsA(isA<RequestError>()
            .having((e) => e.code, 'code', ErrorCodes.fileTooLarge)
            .having((e) => e.message, 'message', contains('share limit'))),
      );
      expect(stagedLeftovers(), isEmpty, reason: 'the limit is enforced before copying');
    });

    test('localFileUrl builds a token-bearing /api/files/local URL the daemon accepts',
        () async {
      final url = owner.localFileUrl(roomId: roomId, fileId: 'blake3:absent');
      expect(url.path, '/api/files/local');
      expect(url.queryParameters['room_id'], roomId);
      expect(url.queryParameters['file_id'], 'blake3:absent');
      expect(url.queryParameters['token'], owner.authToken);

      final http = HttpClient();
      addTearDown(() => http.close(force: true));

      // The right token gets past the gate; with no verified local copy the
      // answer is the standard JSON error envelope, never a 401.
      final ok = await (await http.getUrl(url)).close();
      final okBody = await utf8.decoder.bind(ok).join();
      expect(ok.statusCode, isNot(anyOf(401, 403)));
      final envelope = jsonDecode(okBody) as Map<String, dynamic>;
      expect(envelope['ok'], isFalse);
      expect((envelope['error'] as Map)['code'], isA<String>());

      // A wrong token is refused outright.
      final bad = url.replace(queryParameters: {
        ...url.queryParameters,
        'token': '0' * 64,
      });
      final refused = await (await http.getUrl(bad)).close();
      await refused.drain<void>();
      expect(refused.statusCode, 401);
    });
  });

  group('without a live daemon', () {
    test('attachToRunning refuses a missing portfile', () async {
      final empty = Directory.systemTemp.createTempSync('jeliya-http-none-');
      addTearDown(() => empty.deleteSync(recursive: true));
      final attached = SidecarSupervisor.attach(dataDir: empty.path);
      await expectLater(
        attached.attachToRunning(),
        throwsA(isA<SidecarError>().having((e) => e.message, 'message', contains('no portfile'))),
      );
    });

    test('attachToRunning refuses a stale portfile (health check, never blind trust)',
        () async {
      final stale = Directory.systemTemp.createTempSync('jeliya-http-stale-');
      addTearDown(() => stale.deleteSync(recursive: true));
      // A port that answered once but is dead now.
      final probe = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
      final deadPort = probe.port;
      await probe.close();
      File('${stale.path}/daemon.json').writeAsStringSync(jsonEncode({
        'schema': 1,
        'pid': 999999,
        'port': deadPort,
        'http': 'http://127.0.0.1:$deadPort/',
        'ws': 'ws://127.0.0.1:$deadPort/ws',
        'version': '0.0.0',
        'protocol': SidecarSupervisor.expectedProtocol,
        'data_dir': stale.path,
        'auth_token': 'a' * 64,
        'started_at_ms': DateTime.now().millisecondsSinceEpoch,
      }));
      final attached = SidecarSupervisor.attach(dataDir: stale.path);
      expect(await attached.healthCheck(), isFalse);
      await expectLater(
        attached.attachToRunning(),
        throwsA(isA<SidecarError>().having((e) => e.message, 'message', contains('stale'))),
      );
    });
  });

  group('rpc-based stop', () {
    test('stopDaemon shuts an ADOPTED daemon down via daemon.shutdown', () async {
      final dir = Directory.systemTemp.createTempSync('jeliya-http-stop-');
      final spawner = SidecarSupervisor(binaryPath: binary, dataDir: dir.path, loopback: true);
      WsClient? adoptedClient;
      try {
        await spawner.start(port: 0);
        final adopter = SidecarSupervisor.attach(dataDir: dir.path);
        final adopted = await adopter.attachToRunning();
        expect(adopted.adopted, isTrue);

        adoptedClient = WsClient(adopter.wsUrl);
        await adoptedClient.start();
        // shutdown() cannot reach an adopted daemon (no process handle); the
        // RPC is the lever — and stopDaemon verifies the daemon actually died
        // instead of trusting the reply.
        await adopter.stopDaemon(adoptedClient);
        expect(await adopter.healthCheck(), isFalse);
        expect(adopter.ready, isNull);
        expect(File('${dir.path}/daemon.json').existsSync(), isFalse,
            reason: 'graceful teardown removes the portfile');
      } finally {
        await adoptedClient?.stop();
        await spawner.shutdown(); // safe on an already-exited child
        try {
          dir.deleteSync(recursive: true);
        } catch (_) {}
      }
    });
  });
}
