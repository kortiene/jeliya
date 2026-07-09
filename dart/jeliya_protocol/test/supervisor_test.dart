// Validates the sidecar supervisor's adoption path — the exact case the
// stdout/exitCode race (Phase 2 review finding #3) broke ~100% of the time
// under the JIT VM that `dart test` uses. A second supervisor on the same data
// dir must ADOPT the incumbent (parse its `already_running` line before the
// spawned child's exit is observed), not fail with "exited before ready".

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

void main() {
  final binary = '${_repoRoot().path}/target/debug/jeliyad';

  if (!File(binary).existsSync()) {
    test('supervisor adoption (daemon binary missing)', () {}, skip: 'jeliyad not built');
    return;
  }

  test('a second supervisor on the same data dir adopts the incumbent', () async {
    final dataDir = Directory.systemTemp.createTempSync('jeliya-adopt-');
    final owner = SidecarSupervisor(binaryPath: binary, dataDir: dataDir.path, loopback: true);
    SidecarSupervisor? adopter;
    try {
      final first = await owner.start(port: 0);
      expect(first.adopted, isFalse, reason: 'first launch owns the daemon');

      // Run the adopt path several times: the race is timing-dependent, so a
      // single pass could pass by luck — repeat to make a regression loud.
      for (var i = 0; i < 5; i++) {
        adopter = SidecarSupervisor(binaryPath: binary, dataDir: dataDir.path, loopback: true);
        final second = await adopter.start(port: 0);
        expect(second.adopted, isTrue, reason: 'attempt $i: second launch must adopt, not fail');
        expect(second.pid, equals(first.pid), reason: 'adopts the incumbent pid');
        await adopter.shutdown(); // the adopter never spawned a live daemon
        adopter = null;
      }
    } finally {
      await adopter?.shutdown();
      await owner.shutdown();
      try {
        dataDir.deleteSync(recursive: true);
      } catch (_) {}
    }
  });
}
