/// The desktop-sidecar half of the Phase 0 process-supervision contract
/// (docs/PROTOCOL.md, "Process supervision"), from the client side: spawn
/// `jeliyad --supervised`, read its machine-readable `ready` (or
/// `already_running`) line, and locate the auth token in the portfile.
///
/// The app owns the daemon: it is spawned with `--supervised`, so the daemon
/// self-terminates if this process dies (its stdin, our pipe, closes). A second
/// launch on the same data dir reports `already_running`; the supervisor adopts
/// it transparently by reading the incumbent's portfile. A second CLIENT that
/// should never spawn at all (a diagnostic tool, a script riding along a
/// running app) uses [SidecarSupervisor.attach] + [SidecarSupervisor.attachToRunning]
/// instead: portfile + health check, no binary needed.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'methods.dart';
import 'models.dart';
import 'protocol.dart';

/// The parsed `daemon.json` portfile (docs/PROTOCOL.md, "The portfile") — the
/// canonical discovery point for native clients. `port`/`pid`/`protocol`/
/// `auth_token`/`data_dir` are required (a portfile missing them is unreadable
/// and treated as absent); the remaining schema-1 fields are parsed tolerantly
/// so a truncated or older portfile still yields the essentials.
class Portfile {
  Portfile({
    required this.port,
    required this.pid,
    required this.protocol,
    required this.authToken,
    required this.dataDir,
    this.schema,
    this.http,
    this.ws,
    this.version,
    this.startedAtMs,
  });

  factory Portfile.fromJson(Map<String, dynamic> json) => Portfile(
        port: json['port'] as int,
        pid: json['pid'] as int,
        protocol: json['protocol'] as int,
        authToken: json['auth_token'] as String,
        dataDir: json['data_dir'] as String,
        schema: json['schema'] is int ? json['schema'] as int : null,
        http: json['http'] is String ? json['http'] as String : null,
        ws: json['ws'] is String ? json['ws'] as String : null,
        version: json['version'] is String ? json['version'] as String : null,
        startedAtMs: json['started_at_ms'] is num ? (json['started_at_ms'] as num).toInt() : null,
      );

  final int port;
  final int pid;
  final int protocol;
  final String authToken;

  /// The daemon's own (canonicalized) data dir — compare against this, not the
  /// path the supervisor was configured with, which may spell the same
  /// directory differently (e.g. macOS `/var` vs `/private/var`).
  final String dataDir;

  /// Portfile schema major (`1` today).
  final int? schema;

  /// The daemon's advertised HTTP origin, e.g. `http://127.0.0.1:54443/`.
  final String? http;

  /// The daemon's advertised control socket, e.g. `ws://127.0.0.1:54443/ws`.
  final String? ws;

  /// The daemon build version (informational; `protocol` is the contract).
  final String? version;

  /// Wall-clock start instant (ms since epoch).
  final int? startedAtMs;
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

/// A daemon speaking a protocol major this client was not built against. Per
/// the adopt-vs-respawn rule (docs/PROTOCOL.md, version skew) the caller must
/// NOT adopt and NOT spawn a second daemon: ask the incumbent to exit
/// ([SidecarSupervisor.stopDaemon]) and respawn the bundled binary.
class ProtocolMismatchError extends SidecarError {
  ProtocolMismatchError({required this.actual, required this.expected})
      : super('daemon speaks protocol $actual, this client expects $expected');

  final int actual;
  final int expected;
}

/// The post-connect protocol handshake (docs/PROTOCOL.md, Protocol version):
/// `/ws` has no connect-time negotiation and no greeting frame, and a
/// [Client]'s reconnect loop can silently attach to a DIFFERENT daemon build
/// (an upgrade restarted it under us), so a client MUST read `daemon.status`
/// after every (re)connect and treat an unsupported `protocol` as a hard
/// incompatibility. Throws [ProtocolMismatchError]; returns the status
/// otherwise, so the caller reuses the round-trip for its own re-sync.
Future<DaemonStatus> verifyDaemonProtocol(
  Client client, {
  int expected = SidecarSupervisor.expectedProtocol,
}) async {
  final status = await client.daemonStatus();
  if (status.protocol != expected) {
    throw ProtocolMismatchError(actual: status.protocol, expected: expected);
  }
  return status;
}

class SidecarSupervisor {
  SidecarSupervisor({
    required this.binaryPath,
    required this.dataDir,
    this.loopback = false,
  });

  /// Attach-only supervisor: no binary, no spawn — [attachToRunning] (the
  /// portfile + health-check path) is its one way to a [ready] daemon, and
  /// [start] always fails. For a second client riding along a daemon someone
  /// else supervises.
  SidecarSupervisor.attach({required this.dataDir})
      : binaryPath = '',
        loopback = false;

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
    // Surface daemon stderr for diagnostics without coupling to it.
    process.stderr.transform(utf8.decoder).transform(const LineSplitter()).listen((_) {});

    try {
      final ready = await _firstContractLine(process, timeout);
      if (ready.adopted) {
        // The daemon we spawned exited 0; the incumbent owns the data dir. Its
        // portfile is authoritative. `_process` is deliberately left alone:
        // if this is a re-entrant start() while we already own a live child
        // (a Retry that raced a still-running daemon), that handle must keep
        // its SIGTERM lever; if we never owned one it stays null and
        // [stopDaemon] verifies death via [healthCheck] instead of trusting a
        // signal to an already-exited probe. A held handle whose pid is NOT
        // the incumbent's is stale (our child died and someone else owns the
        // dir now) — drop it so stopDaemon uses the health-check path.
        final held = _process;
        if (held != null && held.pid != ready.pid) _process = null;
        await process.exitCode;
      } else {
        _process = process;
      }
      final pf = readPortfile();
      if (pf == null) throw SidecarError('daemon ready but no readable portfile in $dataDir');
      if (pf.protocol != expectedProtocol) {
        throw ProtocolMismatchError(actual: pf.protocol, expected: expectedProtocol);
      }
      _ready = ready;
      return ready;
    } catch (_) {
      // Never leave a spawned-but-unusable daemon running behind a failed
      // start (a protocol mismatch, a missing portfile, a timeout). Killing
      // the child we spawned is safe; if we adopted, the child already
      // exited. A previously owned child's handle (re-entrant start) is NOT
      // dropped here — only this attempt's process is cleaned up.
      process.kill(ProcessSignal.sigterm);
      if (identical(_process, process)) _process = null;
      rethrow;
    }
  }

  /// A [WsUrlResolver]-compatible callback: the current `ws://…/ws?token=…`,
  /// re-read from the portfile each call so a restart's new token heals.
  Future<Uri> wsUrl() async {
    final pf = readPortfile();
    if (pf == null) throw SidecarError('no portfile to resolve a ws url from');
    return Uri.parse('ws://127.0.0.1:${pf.port}/ws?token=${pf.authToken}');
  }

  /// The daemon's loopback HTTP origin, re-read from the portfile each call so
  /// a restart's new port heals. Prefers the portfile's own advertised `http`,
  /// falling back to the port. Throws [SidecarError] with no readable portfile.
  Uri httpBase() {
    final pf = readPortfile();
    if (pf == null) throw SidecarError('no portfile to resolve an http base from');
    final advertised = pf.http;
    if (advertised != null) {
      final parsed = Uri.tryParse(advertised);
      if (parsed != null && parsed.hasScheme && parsed.hasAuthority) return parsed;
    }
    return Uri.parse('http://127.0.0.1:${pf.port}/');
  }

  /// The daemon's per-start auth token (`/ws` and `/api/files/*` are gated on
  /// it), re-read from the portfile each call; null with no readable portfile.
  String? get authToken => readPortfile()?.authToken;

  /// `GET /api/health` against the portfile's advertised port, verifying the
  /// answering process IS the portfile's daemon — same `pid`, same `data_dir`
  /// (both compared against the portfile's own values, which the daemon wrote,
  /// so path-canonicalization differences cannot false-negative). This is the
  /// stale-portfile guard PROTOCOL.md mandates before adopting ("never trust
  /// it blind"). Returns false on any failure; never throws.
  Future<bool> healthCheck({Duration timeout = const Duration(seconds: 2)}) async {
    final pf = readPortfile();
    if (pf == null) return false;
    final health = await _getHealth(pf.port, timeout);
    if (health == null) return false;
    return health['ok'] == true && health['pid'] == pf.pid && health['data_dir'] == pf.dataDir;
  }

  /// Attach to an already-running daemon from its portfile alone — no binary,
  /// no spawn (pair with [SidecarSupervisor.attach]). Health-checks before
  /// adopting and refuses a protocol major we were not built against
  /// ([ProtocolMismatchError]; adopt-vs-respawn rule). On success this
  /// supervisor behaves like a `already_running` adopter: [ready] is set,
  /// [wsUrl]/[httpBase]/[authToken] resolve, [shutdown] is a no-op — an
  /// adopted daemon is stopped through [stopDaemon].
  Future<Ready> attachToRunning({Duration timeout = const Duration(seconds: 2)}) async {
    final pf = readPortfile();
    if (pf == null) {
      throw SidecarError('no portfile in $dataDir — is jeliyad running?');
    }
    if (!await healthCheck(timeout: timeout)) {
      throw SidecarError(
          'stale portfile in $dataDir: no healthy daemon answered on port ${pf.port}');
    }
    if (pf.protocol != expectedProtocol) {
      throw ProtocolMismatchError(actual: pf.protocol, expected: expectedProtocol);
    }
    final ready = Ready(pf.port, pf.pid, adopted: true);
    _ready = ready;
    return ready;
  }

  /// Graceful stop: SIGTERM the daemon (which closes its rooms and removes the
  /// portfile) and await exit. A no-op if we adopted an incumbent we did not
  /// spawn (the spawned child already exited) — use [stopDaemon] for those.
  Future<void> shutdown() async {
    final p = _process;
    if (p == null) return;
    p.kill(ProcessSignal.sigterm);
    await p.exitCode.timeout(const Duration(seconds: 12), onTimeout: () {
      p.kill(ProcessSignal.sigkill);
      return -1;
    });
    _process = null;
    _ready = null;
  }

  /// Stop the daemon through the protocol: `daemon.shutdown` over [client] —
  /// the one lever that reaches an ADOPTED daemon, where [shutdown] has no
  /// process to signal — falling back to SIGTERM when we own the spawned
  /// process. All shutdown triggers run the same graceful teardown
  /// (docs/PROTOCOL.md, Shutdown), so for an owned daemon this converges on
  /// [shutdown]'s bounded exit wait; for an adopted one it polls [healthCheck]
  /// until the daemon actually goes dark and throws [SidecarError] if it never
  /// does within [timeout].
  Future<void> stopDaemon(Client client, {Duration timeout = const Duration(seconds: 12)}) async {
    try {
      // Replies `{ shutting_down: true }` first, then exits.
      await client.daemonShutdown();
    } catch (_) {
      // The transport may already be down, or the daemon's exit raced the
      // reply. The owned path below still has SIGTERM; the adopted path
      // verifies actual death rather than trusting the reply anyway.
    }
    if (_process != null) {
      // Owned: a redundant SIGTERM during graceful teardown is harmless, and
      // this reuses the bounded exit wait + SIGKILL escalation.
      await shutdown();
      return;
    }
    final deadline = DateTime.now().add(timeout);
    while (await healthCheck(timeout: const Duration(seconds: 1))) {
      if (DateTime.now().isAfter(deadline)) {
        throw SidecarError(
            'adopted daemon is still healthy ${timeout.inSeconds}s after daemon.shutdown');
      }
      await Future<void>.delayed(const Duration(milliseconds: 150));
    }
    _ready = null;
  }

  /// The current portfile, or null when missing/unreadable/torn (the write is
  /// atomic, so a readable portfile is never half of one).
  Portfile? readPortfile() {
    try {
      final raw = File('$dataDir/daemon.json').readAsStringSync();
      return Portfile.fromJson(jsonDecode(raw) as Map<String, dynamic>);
    } catch (_) {
      return null;
    }
  }

  Future<Map<String, dynamic>?> _getHealth(int port, Duration timeout) async {
    final http = HttpClient()..connectionTimeout = timeout;
    try {
      final request =
          await http.getUrl(Uri.parse('http://127.0.0.1:$port/api/health')).timeout(timeout);
      final response = await request.close().timeout(timeout);
      final body = await utf8.decoder.bind(response).join().timeout(timeout);
      if (response.statusCode != 200) return null;
      final decoded = jsonDecode(body);
      return decoded is Map ? decoded.cast<String, dynamic>() : null;
    } catch (_) {
      return null;
    } finally {
      http.close(force: true);
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
