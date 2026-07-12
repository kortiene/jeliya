// The Phase 2 crux: the Dart protocol client is held to the EXACT same golden
// corpus as the reference TypeScript client and the daemon (Phase 1). Spawns a
// real jeliyad via the sidecar supervisor, connects the Dart WsClient with the
// portfile token, and replays ui/src/lib/conformance/corpus.json — proving the
// transport-agnostic architecture works end to end over the WebSocket seam.
//
//   dart test        (from dart/jeliya_protocol/)
//
// Skips with a clear reason if the daemon binary is not built.

import 'dart:convert';
import 'dart:io';

import 'package:jeliya_protocol/jeliya_protocol.dart';
import 'package:test/test.dart';

/// Walk up from the current dir to the repo root (the dir containing
/// `ui/src/lib/conformance/corpus.json`).
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

void main() {
  final repo = _repoRoot();
  final binary = '${repo.path}/target/debug/jeliyad';
  final corpusFile = File('${repo.path}/ui/src/lib/conformance/corpus.json');

  if (!File(binary).existsSync()) {
    test('conformance (daemon binary missing)', () {
      // A silent skip on a conformance suite reads as green while covering
      // nothing; mark it explicitly.
      markTestSkipped('jeliyad not built at $binary — run `cargo build`');
    }, skip: 'jeliyad not built (run `cargo build`)');
    return;
  }

  final corpus = jsonDecode(corpusFile.readAsStringSync()) as Map<String, dynamic>;
  final scenarios = (corpus['scenarios'] as List)
      .cast<Map<String, dynamic>>()
      .where((s) => !((s['tags'] as List?)?.contains('mockOnly') ?? false))
      .toList();

  group('conformance: Dart client vs real daemon', () {
    late SidecarSupervisor supervisor;
    late WsClient client;
    late Directory dataDir;

    setUpAll(() async {
      dataDir = Directory.systemTemp.createTempSync('jeliya-dart-conf-');
      supervisor = SidecarSupervisor(binaryPath: binary, dataDir: dataDir.path, loopback: true);
      final ready = await supervisor.start(port: 0);
      expect(ready.port, greaterThan(0));
      expect(ready.adopted, isFalse);
      client = WsClient(supervisor.wsUrl);
      await client.start().timeout(const Duration(seconds: 10));
      // Shared precondition: an identity exists (a fresh daemon has none).
      await client.call('identity.create');
    });

    tearDownAll(() async {
      await client.stop();
      await supervisor.shutdown();
      try {
        dataDir.deleteSync(recursive: true);
      } catch (_) {}
    });

    for (final scenario in scenarios) {
      test(scenario['name'] as String, () async {
        final preIdentity = (scenario['tags'] as List?)?.contains('preIdentity') ?? false;
        SidecarSupervisor? freshSupervisor;
        WsClient? freshClient;
        Directory? freshDataDir;
        try {
          if (preIdentity) {
            freshDataDir = Directory.systemTemp.createTempSync('jeliya-dart-conf-fresh-');
            freshSupervisor = SidecarSupervisor(
                binaryPath: binary, dataDir: freshDataDir.path, loopback: true);
            await freshSupervisor.start(port: 0);
            freshClient = WsClient(freshSupervisor.wsUrl);
            await freshClient.start().timeout(const Duration(seconds: 10));
          }
          final results = await replayScenario(freshClient ?? client, scenario,
              pushWaitMs: 3000);
          final failures = results.where((r) => !r.ok).toList();
          expect(
              failures
                  .map((f) => 'step ${f.step} (${f.method}): ${f.detail}')
                  .toList(),
              isEmpty);
        } finally {
          await freshClient?.stop();
          await freshSupervisor?.shutdown();
          try {
            freshDataDir?.deleteSync(recursive: true);
          } catch (_) {}
        }
      });
    }
  });
}
