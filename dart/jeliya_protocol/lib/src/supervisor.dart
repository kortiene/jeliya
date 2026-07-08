/// The desktop-sidecar half of the Phase 0 process-supervision contract
/// (docs/PROTOCOL.md, "Process supervision"), from the client side: spawn
/// `jeliyad --supervised`, read its machine-readable `ready` (or
/// `already_running`) line, and locate the auth token in the portfile.
///
/// The app owns the daemon: it is spawned with `--supervised`, so the daemon
/// self-terminates if this process dies (its stdin, our pipe, closes). A second
/// launch on the same data dir reports `already_running`; the supervisor adopts
/// it transparently by reading the incumbent's portfile.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

/// The parsed `daemon.json` portfile.
class Portfile {
  Portfile(this.port, this.pid, this.protocol, this.authToken, this.dataDir);

  factory Portfile.fromJson(Map<String, dynamic> json) => Portfile(
        json['port'] as int,
        json['pid'] as int,
        json['protocol'] as int,
        json['auth_token'] as String,
        json['data_dir'] as String,
      );

  final int port;
  final int pid;
  final int protocol;
  final String authToken;
  final String dataDir;
}

/// The outcome of a spawn: which daemon we are talking to and whether we
/// started it or adopted a running one.
class Ready {
  Ready(this.port, this.pid, {required this.adopted});
  final int port;
  final int pid;
  final bool adopted;
}

class SidecarError implements Exception {
  SidecarError(this.message);
  final String message;
  @override
  String toString() => 'SidecarError: $message';
}

class SidecarSupervisor {
  SidecarSupervisor({
    required this.binaryPath,
    required this.dataDir,
    this.loopback = false,
  });

  final String binaryPath;
  final String dataDir;
  final bool loopback;

  Process? _process;
  Ready? _ready;

  Ready? get ready => _ready;

  /// The major protocol version this client was built against; adoption is
  /// refused across a different major (docs/PROTOCOL.md, version skew).
  static const int expectedProtocol = 1;

  /// Spawn (or adopt) the daemon and return once it is ready. [port] `0` lets
  /// the OS choose; read the real one from [Ready.port].
  Future<Ready> start({int port = 0, Duration timeout = const Duration(seconds: 15)}) async {
    if (!File(binaryPath).existsSync()) {
      throw SidecarError('jeliyad binary not found at $binaryPath (run `cargo build`)');
    }
    await Directory(dataDir).create(recursive: true);
    final args = <String>[
      '--supervised',
      if (loopback) '--loopback',
      '--port', '$port',
      '--data-dir', dataDir,
    ];
    final process = await Process.start(binaryPath, args);
    _process = process;
    // Surface daemon stderr for diagnostics without coupling to it.
    process.stderr.transform(utf8.decoder).transform(const LineSplitter()).listen((_) {});

    try {
      final ready = await _firstContractLine(process, timeout);
      if (ready.adopted) {
        // The daemon we spawned exited 0; the incumbent owns the data dir. Its
        // portfile is authoritative.
        await process.exitCode;
      }
      final pf = _readPortfile();
      if (pf == null) throw SidecarError('daemon ready but no readable portfile in $dataDir');
      if (pf.protocol != expectedProtocol) {
        throw SidecarError('daemon speaks protocol ${pf.protocol}, this client expects $expectedProtocol');
      }
      _ready = ready;
      return ready;
    } catch (_) {
      // Never leave a spawned-but-unusable daemon running behind a failed
      // start (a protocol mismatch, a missing portfile, a timeout). Killing the
      // child we spawned is safe; if we adopted, the child already exited.
      process.kill(ProcessSignal.sigterm);
      _process = null;
      rethrow;
    }
  }

  /// A [WsUrlResolver]-compatible callback: the current `ws://…/ws?token=…`,
  /// re-read from the portfile each call so a restart's new token heals.
  Future<Uri> wsUrl() async {
    final pf = _readPortfile();
    if (pf == null) throw SidecarError('no portfile to resolve a ws url from');
    return Uri.parse('ws://127.0.0.1:${pf.port}/ws?token=${pf.authToken}');
  }

  /// Graceful stop: SIGTERM the daemon (which closes its rooms and removes the
  /// portfile) and await exit. A no-op if we adopted an incumbent we did not
  /// spawn (the spawned child already exited).
  Future<void> shutdown() async {
    final p = _process;
    if (p == null) return;
    p.kill(ProcessSignal.sigterm);
    await p.exitCode.timeout(const Duration(seconds: 12), onTimeout: () {
      p.kill(ProcessSignal.sigkill);
      return -1;
    });
    _process = null;
  }

  Portfile? _readPortfile() {
    try {
      final raw = File('$dataDir/daemon.json').readAsStringSync();
      return Portfile.fromJson(jsonDecode(raw) as Map<String, dynamic>);
    } catch (_) {
      return null;
    }
  }

  Future<Ready> _firstContractLine(Process process, Duration timeout) {
    final completer = Completer<Ready>();
    late StreamSubscription<String> sub;
    final timer = Timer(timeout, () {
      if (!completer.isCompleted) {
        sub.cancel();
        completer.completeError(SidecarError('no ready line on stdout within ${timeout.inSeconds}s'));
      }
    });
    sub = process.stdout
        .transform(utf8.decoder)
        .transform(const LineSplitter())
        .listen(
      (line) {
        final trimmed = line.trim();
        if (!trimmed.startsWith('{')) return;
        try {
          final obj = jsonDecode(trimmed) as Map<String, dynamic>;
          final event = obj['event'];
          if (event == 'ready' || event == 'already_running') {
            timer.cancel();
            if (!completer.isCompleted) {
              completer.complete(Ready(obj['port'] as int, obj['pid'] as int, adopted: event == 'already_running'));
            }
            // Keep draining stdout after this (do NOT cancel the sub, or the
            // daemon's stdout pipe can fill and block it); further lines are
            // ignored since the completer is already done.
          }
        } catch (_) {/* keep reading */}
      },
      // The `already_running` adopt line is immediately followed by the child's
      // exit, and dart:io does NOT guarantee `process.exitCode` completes after
      // the buffered stdout is delivered. Gating "exited before ready" on the
      // stream's onDone (which fires only after every data event has drained)
      // instead of on exitCode is what makes adoption reliable.
      onDone: () {
        timer.cancel();
        if (!completer.isCompleted) {
          completer.completeError(SidecarError('daemon exited before printing a ready line'));
        }
      },
    );
    return completer.future;
  }
}
