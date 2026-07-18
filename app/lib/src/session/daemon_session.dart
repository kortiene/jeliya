/// The app-scoped session: owns the SidecarSupervisor + Client lifecycle, the
/// Boot bring-up machine, the bootstrap phase machine
/// (boot | no-identity | no-rooms | ready), reconnect-driven re-sync (on
/// mobile, foreground-resume-driven — the in-process engine never
/// reconnects), the current [RoomStore], local names, and the diagnostics
/// error memory.
///
/// Preserves the walking-skeleton supervision seam exactly (phase3-shell.json
/// keep-list): spawn-or-adopt via `supervisor.start(port: 0)`, pass
/// `supervisor.wsUrl` (the method itself) as the [WsUrlResolver] so reconnects
/// re-read the portfile, bring-up order supervisor.start → Client → to
/// client.start → daemon.status bootstrap, teardown order `client.stop()`
/// BEFORE `supervisor.shutdown()` (hoisted to [AppLifecycleListener]
/// `onExitRequested` so Cmd-Q tears down gracefully; `--supervised`
/// stdin-death stays the crash backstop).
///
/// Widget tests skip the supervisor entirely by injecting a [Client]
/// (`DaemonSession(client: MockClient())` with a [PrefsStore.inMemory]).
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' show AppExitResponse;

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart' hide ConnectionState;
import 'package:jeliya_protocol/jeliya_protocol.dart';

import '../l10n/gen/app_strings.dart' show AppStrings;
import 'prefs_store.dart';
import 'room_list.dart' show LifecycleFilter;
import 'room_store.dart';

/// The app version reported in diagnostics (kept in sync with pubspec.yaml).
const String kAppVersion = '1.0.0';

/// Boot bring-up phases (the walking skeleton's machine, kept verbatim):
/// process/transport lifecycle, distinct from the protocol-level
/// [BootstrapPhase].
enum Boot { starting, spawning, connecting, ready, failed }

/// WHAT is happening / WHY it failed — structured facts the boot screen
/// composes localized copy from at render time (the session never holds
/// user-facing strings, so a live locale switch re-renders everything).
enum BootStage {
  none,
  spawning,
  evicting,
  adopted,
  daemonUp,
  failedBinaryMissing,
  failedMismatch,
  failedStart,
  failedTimeout,
  failedGeneric,
}

/// Protocol bootstrap phases (App.tsx `Phase`): what the UI routes on.
enum BootstrapPhase { boot, noIdentity, noRooms, ready }

/// Builds the transport given the portfile-reading URL resolver — injectable
/// so tests can substitute a fake transport without a supervisor.
typedef ClientFactory = Client Function(WsUrlResolver resolveUrl);

/// Resolve the jeliyad binary: `JELIYAD_BIN` env override → bundled sidecar
/// (next to the app executable) → debug-only repo fallback → an installed
/// `jeliyad` on Linux's `PATH`. Null when nothing is found
/// ([DaemonSession] surfaces this as a [Boot.failed] with a hint).
String? resolveJeliyadBinary() => resolveJeliyadBinaryFrom(
  environment: Platform.environment,
  resolvedExecutable: Platform.resolvedExecutable,
  currentDirectory: Directory.current.path,
  debugMode: kDebugMode,
  isLinux: Platform.isLinux,
  fileExists: _isRegularFile,
  fileIsExecutable: _isExecutableFile,
);

bool _isRegularFile(String path) {
  try {
    return FileStat.statSync(path).type == FileSystemEntityType.file;
  } on FileSystemException {
    return false;
  }
}

bool _isExecutableFile(String path) {
  try {
    final stat = FileStat.statSync(path);
    if (stat.type != FileSystemEntityType.file) return false;
    // Linux and macOS both require at least one execute bit before exec(2) can
    // use a candidate. Windows has no corresponding permission bits.
    if (!Platform.isWindows && stat.mode & 0x49 == 0) return false;
    // FileStat cannot see ownership, so exec bits alone cannot prove THIS user
    // may run the file — a root-only 0700 binary still shows an owner-exec
    // bit. Probing read access rejects such entries so the PATH scan keeps
    // trying later candidates instead of returning one exec(2) must refuse.
    File(path).openSync().closeSync();
    return true;
  } on FileSystemException {
    return false;
  }
}

/// Pure binary-resolution seam used by platform tests. The bundle candidates
/// deliberately precede both developer and system fallbacks: a packaged app
/// must run the sidecar it shipped and was tested with, not an unrelated
/// `jeliyad` installed on the host. To honor that, bundle and repo candidates
/// resolve on EXISTENCE ([fileExists]) — a shipped sidecar that lost its
/// execute bit is still selected so the spawn failure names its exact path —
/// while the last-resort PATH scan requires a usable file
/// ([fileIsExecutable]) and skips entries this user cannot run.
@visibleForTesting
String? resolveJeliyadBinaryFrom({
  required Map<String, String> environment,
  required String resolvedExecutable,
  required String currentDirectory,
  required bool debugMode,
  required bool isLinux,
  required bool Function(String path) fileExists,
  required bool Function(String path) fileIsExecutable,
}) {
  final env = environment['JELIYAD_BIN'];
  if (env != null && env.isNotEmpty) return env;

  // macOS: Jeliya.app/Contents/MacOS/<exe> → Contents/{Resources,Helpers}.
  // Linux: the generated bundle puts application-owned executables beside the
  // runner; lib/jeliya is also supported for distro-style layouts.
  final exeDir = File(resolvedExecutable).parent.path;
  final bundleCandidates = isLinux
      ? ['$exeDir/jeliyad', '$exeDir/../lib/jeliya/jeliyad']
      : [
          '$exeDir/../Resources/jeliyad',
          '$exeDir/../Helpers/jeliyad',
          '$exeDir/jeliyad',
        ];
  for (final candidate in bundleCandidates) {
    if (fileExists(candidate)) return candidate;
  }

  if (debugMode) {
    // Repo debug build, relative to a typical checkout — debug builds only.
    // First existing candidate wins; TAC/bantaba is the pre-rename checkout
    // directory name, kept for machines that never re-cloned.
    final home = environment['HOME'] ?? '.';
    for (final candidate in [
      '$home/TAC/jeliya/target/debug/jeliyad',
      '$home/TAC/bantaba/target/debug/jeliyad',
      '$currentDirectory/../target/debug/jeliyad',
    ]) {
      if (fileExists(candidate)) return candidate;
    }
  }

  // Linux users may already have installed the standalone daemon. This is a
  // last resort, after the bundled and repo-matched binaries, so PATH cannot
  // silently replace a release's tested sidecar.
  final path = environment['PATH'];
  if (isLinux && path != null && path.isNotEmpty) {
    for (final directory in path.split(':')) {
      if (directory.isEmpty) continue;
      final candidate = directory.endsWith('/')
          ? '${directory}jeliyad'
          : '$directory/jeliyad';
      if (fileIsExecutable(candidate)) return candidate;
    }
  }
  return null;
}

/// The user's REAL home from a raw `$HOME` value. Inside the App Sandbox
/// `$HOME` points at the app container (`~/Library/Containers/<id>/Data`),
/// but the shared data dir ships as a home-RELATIVE sandbox exception that
/// macOS resolves against the real home — so unwrap the container prefix.
@visibleForTesting
String realHomeFrom(String home) {
  const marker = '/Library/Containers/';
  final i = home.indexOf(marker);
  return i > 0 ? home.substring(0, i) : home;
}

/// The daemon data dir: `JELIYA_DATA_DIR` env override (test automation and
/// side-by-side profiles) → the platform data root. Linux follows XDG
/// (`$XDG_DATA_HOME`, falling back to `~/.local/share`); macOS continues to
/// use `~/Library/Application Support`. Release uses `Jeliya`, while debug
/// uses `JeliyaAppDev` so development runs never touch real user data.
String defaultDataDir() => defaultDataDirFrom(
  environment: Platform.environment,
  fallbackHome: Directory.systemTemp.path,
  debugMode: kDebugMode,
  isLinux: Platform.isLinux,
);

/// Why Linux data-dir resolution can fail: the XDG Base Directory spec
/// requires an absolute base, and identity material must never land relative
/// to the launch directory. Shared between the resolution seam's [StateError]
/// and the boot screen's technical detail.
@visibleForTesting
const String linuxDataDirRequirement =
    'Linux data directory requires an absolute XDG_DATA_HOME or HOME';

/// Pure data-directory resolution seam used by platform tests.
@visibleForTesting
String defaultDataDirFrom({
  required Map<String, String> environment,
  required String fallbackHome,
  required bool debugMode,
  required bool isLinux,
}) {
  final override = environment['JELIYA_DATA_DIR'];
  if (override != null && override.isNotEmpty) return override;

  final name = debugMode ? 'JeliyaAppDev' : 'Jeliya';
  if (isLinux) {
    final xdgDataHome = environment['XDG_DATA_HOME'];
    // The XDG Base Directory specification requires an absolute value. Ignore
    // a relative value rather than storing identity material relative to the
    // app's launch directory.
    final String base;
    if (xdgDataHome != null && xdgDataHome.startsWith('/')) {
      base = xdgDataHome;
    } else {
      final home = environment['HOME'];
      if (home == null || !home.startsWith('/')) {
        throw StateError(linuxDataDirRequirement);
      }
      base = '$home/.local/share';
    }
    return base.endsWith('/') ? '$base$name' : '$base/$name';
  }
  final configuredHome = environment['HOME'];
  final home = configuredHome == null || configuredHome.isEmpty
      ? fallbackHome
      : configuredHome;
  return '${realHomeFrom(home)}/Library/Application Support/$name';
}

/// Linux desktop is a real peer-to-peer node. macOS retains its existing
/// loopback-only sidecar policy (including its current sandbox assumptions).
@visibleForTesting
bool desktopSidecarUsesLoopback({required bool isLinux}) => !isLinux;

/// Opt-in marker used only by the Linux source-package lifecycle gate. Keeping
/// the path fixed under the already-isolated daemon data directory avoids an
/// environment-controlled arbitrary write path. The marker never contains the
/// portfile's WebSocket authentication token.
@visibleForTesting
const String linuxPackageReadinessFileName = 'flutter-session-ready.json';

@visibleForTesting
String? linuxPackageReadinessPathFrom({
  required Map<String, String> environment,
  required bool isLinux,
  required String dataDir,
}) {
  if (!isLinux || environment['JELIYA_LINUX_PACKAGE_GATE'] != '1') {
    return null;
  }
  return dataDir.endsWith('/')
      ? '$dataDir$linuxPackageReadinessFileName'
      : '$dataDir/$linuxPackageReadinessFileName';
}

@visibleForTesting
Map<String, Object> linuxPackageReadinessPayload({
  required String boot,
  required String phase,
  required String connection,
  required String frame,
  required int protocol,
  required int appPid,
  required int daemonPid,
  required int daemonPort,
}) => {
  'schema': 1,
  'boot': boot,
  'phase': phase,
  'connection': connection,
  'frame': frame,
  'protocol': protocol,
  'app_pid': appPid,
  'daemon_pid': daemonPid,
  'daemon_port': daemonPort,
};

class DaemonSession extends ChangeNotifier {
  /// Production: no [client] — the session spawns/adopts jeliyad and builds a
  /// [WsClient] over the supervisor's portfile resolver. Tests: pass a
  /// [client] (e.g. `MockClient` from `jeliya_protocol/testing.dart`) and the
  /// supervisor is skipped entirely; pass [prefs] = [PrefsStore.inMemory] to
  /// keep tests off the disk.
  ///
  /// Mobile: also pass [stageAndShare] so user-file shares go through the
  /// engine's staging convention — with an in-process engine there is no
  /// supervisor to derive it from. Null (desktop, tests) derives it from the
  /// supervisor when one exists.
  DaemonSession({
    Client? client,
    ClientFactory? clientFactory,
    String? binaryPath,
    String? dataDir,
    PrefsStore? prefs,
    StageAndShare? stageAndShare,
  })  : _injectedClient = client,
        _clientFactory = clientFactory ?? WsClient.new,
        _binaryPathOverride = binaryPath,
        _stageAndShareOverride = stageAndShare,
        dataDir = dataDir ?? _defaultDataDirOrEmpty(),
        prefs = prefs ??
            PrefsStore(
              '${dataDir ?? _defaultDataDirOrEmpty()}/app_prefs.json',
            );

  /// A misconfigured environment (no absolute HOME/XDG_DATA_HOME on Linux)
  /// must not throw out of the constructor at widget-build time; the empty
  /// sentinel defers the failure to [start], which reports it through the
  /// regular [Boot.failed] surface with a Retry path. PrefsStore performs no
  /// I/O until `load`, so the sentinel never touches disk.
  static String _defaultDataDirOrEmpty() {
    try {
      return defaultDataDir();
    } on StateError {
      return '';
    }
  }

  final Client? _injectedClient;
  final ClientFactory _clientFactory;
  final String? _binaryPathOverride;
  final StageAndShare? _stageAndShareOverride;

  /// The daemon data dir this session supervises against.
  final String dataDir;

  /// Local prefs: last room, per-room drafts, peer aliases.
  final PrefsStore prefs;

  SidecarSupervisor? _supervisor;
  Client? _client;
  AppLifecycleListener? _lifecycle;

  Boot _boot = Boot.starting;
  BootStage _bootStage = BootStage.none;
  int? _bootPid;
  int? _bootPort;
  int? _bootMismatchActual;
  int? _bootMismatchExpected;
  String _bootTechnical = '';
  BootstrapPhase _phase = BootstrapPhase.boot;
  ConnectionState _conn = ConnectionState.disconnected;
  DaemonStatus? _status;
  List<RoomSummary> _rooms = const [];
  // Room-list view state (issue #64), the counterpart of App.tsx's lifted
  // roomQuery / roomFilter: session-local so search and filter survive
  // entering a room and returning (both shells switch panes without dropping
  // it), but need not outlive the process. Pin/archive and the unread
  // last-seen marks are device-local storage owned by [prefs].
  String _roomQuery = '';
  LifecycleFilter _roomFilter = LifecycleFilter.all;
  RoomStore? _room;
  final Map<String, PendingMessages> _pendingByRoom = {};
  DiagnosticEvent? _lastDiagnosticError;

  int _bootstrapEpoch = 0;
  bool _started = false;
  bool _disposed = false;
  bool _tearingDown = false;
  bool _linuxPackageReadinessWritten = false;
  bool _linuxPackageReadinessScheduled = false;
  final List<StreamSubscription<Object?>> _subs = [];

  // -- read surface --------------------------------------------------------------

  /// The transport, once bring-up has constructed it. Feature code should
  /// prefer the typed extension methods (`JeliyaMethods`) over raw `call`.
  Client? get client => _client;

  /// The supervisor, when this session owns one (null in mock/test mode).
  SidecarSupervisor? get supervisor => _supervisor;

  Boot get boot => _boot;

  /// Structured boot progress/failure facts (composed into copy at render).
  BootStage get bootStage => _bootStage;
  int? get bootPid => _bootPid;
  int? get bootPort => _bootPort;
  int? get bootMismatchActual => _bootMismatchActual;
  int? get bootMismatchExpected => _bootMismatchExpected;

  /// Raw exception text behind a [Boot.failed] summary ('' otherwise) —
  /// deliberately untranslated, like ErrorNote's technical details.
  String get bootTechnical => _bootTechnical;

  BootstrapPhase get phase => _phase;
  ConnectionState get conn => _conn;
  DaemonStatus? get status => _status;
  List<RoomSummary> get rooms => _rooms;

  /// The room-list search query (name / short-id substring) — session-local
  /// view state the sidebar and mobile rooms list both read.
  String get roomQuery => _roomQuery;

  set roomQuery(String value) {
    if (_roomQuery == value) return;
    _roomQuery = value;
    notifyListeners();
  }

  /// The room-list lifecycle filter (all / active / departed) — session-local
  /// view state shared by both shells.
  LifecycleFilter get roomFilter => _roomFilter;

  set roomFilter(LifecycleFilter value) {
    if (_roomFilter == value) return;
    _roomFilter = value;
    notifyListeners();
  }

  /// Whether [room] has a signed event newer than this device's last-seen
  /// mark (docs/room-attention.md, decision 3). Device-local; never a
  /// delivery/read receipt. PrefsStore owns the marks; this only projects.
  bool isRoomUnread(RoomSummary room) =>
      roomUnread(room, prefs.lastSeenFor(room.roomId));

  /// Toggle a room's device-local pin / archive (issue #64). [prefs] stays the
  /// single persistence owner; this wrapper adds the SESSION notification both
  /// shells rebuild on (the sidebar/rooms list reads the room-list projection
  /// off the session, not off prefs directly), mirroring how [setAlias] routes
  /// a prefs write through the session so the tree re-renders in place.
  void togglePinned(String roomId) {
    prefs.togglePinned(roomId);
    notifyListeners();
  }

  void toggleArchived(String roomId) {
    prefs.toggleArchived(roomId);
    notifyListeners();
  }

  /// The current open room's store, or null (no room selected).
  RoomStore? get room => _room;

  String? get currentRoomId => _room?.roomId;

  /// Own identity id (null before onboarding).
  String? get selfId => _status?.identity?.identityId;

  /// Own endpoint id (null until the daemon reports one).
  String? get endpointId => _status?.endpoint?.endpointId;

  /// The last recorded action error, shown in Settings and the diagnostics
  /// report.
  DiagnosticEvent? get lastDiagnosticError => _lastDiagnosticError;

  /// `client.describe()` — the transport target line on the boot screen and
  /// connection banner.
  String get transportDescription => _client?.describe() ?? '';

  // -- bring-up --------------------------------------------------------------------

  /// Bring the session up. Safe to call again after a [Boot.failed] (Retry).
  Future<void> start() async {
    if (_started && _boot != Boot.failed) return;
    _started = true;
    try {
      await prefs.load();
      if (_disposed) return;
      final injected = _injectedClient;
      Client client;
      if (injected != null) {
        client = injected;
        // In-process the transport never drops independently of the process,
        // so the reconnect-driven re-sync in [_onConnectionState] can never
        // fire on mobile; foreground resume is its honest replacement (pushes
        // missed while the OS held the app suspended are reconciled by
        // re-running the bootstrap). Host tests inject too — the platform
        // gate keeps them listener-free.
        if (Platform.isAndroid || Platform.isIOS) {
          _lifecycle ??= AppLifecycleListener(onResume: _onResumed);
        }
      } else {
        if (dataDir.isEmpty) {
          // Constructor-time data-dir resolution failed (see
          // [_defaultDataDirOrEmpty]); classify it like any other failed
          // start so the boot screen renders guidance plus this cause.
          _setBoot(
            Boot.failed,
            BootStage.failedStart,
            technical: linuxDataDirRequirement,
          );
          return;
        }
        _setBoot(Boot.spawning, BootStage.spawning);
        final binary = _binaryPathOverride ?? resolveJeliyadBinary();
        if (binary == null) {
          // Classified directly: the boot screen composes the translatable
          // guidance from this stage. Boot.failed keeps the Retry path alive
          // (see the guard in [start]).
          _setBoot(Boot.failed, BootStage.failedBinaryMissing);
          return;
        }
        final supervisor =
            _supervisor ??
            SidecarSupervisor(
              binaryPath: binary,
              dataDir: dataDir,
              loopback: desktopSidecarUsesLoopback(isLinux: Platform.isLinux),
            );
        Ready ready;
        try {
          ready = await supervisor.start(port: 0);
        } on ProtocolMismatchError {
          // Version-skew rule (docs/PROTOCOL.md): never adopt a foreign-
          // protocol incumbent and never race it with a second daemon —
          // evict it and respawn our bundled binary. One retry only: if the
          // fresh spawn also mismatches, OUR binary is the skewed one and
          // the failure surfaces on the boot screen.
          _setBoot(Boot.spawning, BootStage.evicting);
          await supervisor.evictIncumbent();
          ready = await supervisor.start(port: 0);
        }
        _supervisor = supervisor;
        if (_disposed) {
          // Disposed mid-spawn: dispose()'s teardown ran before the
          // supervisor existed, so unwind the daemon we just started here
          // (stdin-death would only reap it at app exit).
          unawaited(supervisor.shutdown());
          return;
        }
        _setBoot(
          Boot.connecting,
          ready.adopted ? BootStage.adopted : BootStage.daemonUp,
          pid: ready.pid,
          port: ready.port,
        );
        // The method itself, so reconnects re-read the portfile (token/port
        // changes heal) — the Phase 0 supervision seam.
        client = _client ?? _clientFactory(supervisor.wsUrl);
        // Graceful Cmd-Q teardown; stdin-death remains the crash backstop.
        _lifecycle ??= AppLifecycleListener(onExitRequested: _onExitRequested);
      }
      if (_client == null) {
        _client = client;
        _subs.add(client.states.listen(_onConnectionState));
        _subs.add(client.roomEvents.listen(_onRoomEvent));
        _subs.add(client.peersChanged.listen(_onPeersChanged));
      }
      _conn = client.state;
      notifyListeners();
      await client.start().timeout(const Duration(seconds: 10));
      if (_disposed) {
        // Disposed mid-connect: stop the transport and the daemon this call
        // brought up — dispose()'s teardown may have run before either existed.
        unawaited(client.stop());
        unawaited(_supervisor?.shutdown());
        return;
      }
      // Retain the supervised process facts established at daemonUp/adopted.
      // They remain useful diagnostics after transport bring-up and let the
      // opt-in Linux package marker bind the rendered session to its portfile.
      _setBoot(Boot.ready, BootStage.none, pid: _bootPid, port: _bootPort);
      // If the connected transition has not been delivered through the states
      // stream yet, synthesize it so the bootstrap runs exactly once (the
      // wasConnected check in _onConnectionState dedupes the stream's copy).
      if (client.state == ConnectionState.connected &&
          _conn != ConnectionState.connected) {
        _onConnectionState(ConnectionState.connected);
      }
    } catch (e) {
      if (_disposed) return;
      // Classified stage; the boot screen composes translatable copy and
      // '$e' renders as the mono technical line (ProtocolMismatchError
      // before SidecarError — it is a subtype).
      switch (e) {
        case ProtocolMismatchError(:final actual, :final expected):
          _setBoot(Boot.failed, BootStage.failedMismatch,
              mismatchActual: actual, mismatchExpected: expected,
              technical: '$e');
        case SidecarError _:
          _setBoot(Boot.failed, BootStage.failedStart, technical: '$e');
        case TimeoutException _:
          _setBoot(Boot.failed, BootStage.failedTimeout, technical: '$e');
        default:
          _setBoot(Boot.failed, BootStage.failedGeneric, technical: '$e');
      }
    }
  }

  void _setBoot(
    Boot boot,
    BootStage stage, {
    int? pid,
    int? port,
    int? mismatchActual,
    int? mismatchExpected,
    String technical = '',
  }) {
    if (_disposed) return;
    _boot = boot;
    _bootStage = stage;
    _bootPid = pid;
    _bootPort = port;
    _bootMismatchActual = mismatchActual;
    _bootMismatchExpected = mismatchExpected;
    _bootTechnical = technical;
    notifyListeners();
    _maybeScheduleLinuxPackageReadiness();
  }

  void _onConnectionState(ConnectionState state) {
    if (_disposed) return;
    final wasConnected = _conn == ConnectionState.connected;
    _conn = state;
    notifyListeners();
    // The bootstrap sequence re-runs on EVERY transition to 'connected'
    // (reconnect re-sync — pushes are lossy).
    if (state == ConnectionState.connected && !wasConnected) {
      unawaited(_runBootstrap());
    }
  }

  /// Mobile foreground resume → bootstrap re-sync (the reconnect trigger in
  /// [_onConnectionState] never fires with an in-process engine). Gated on
  /// 'connected' like the reconnect path: a stopped/failed engine has nothing
  /// to re-sync against.
  void _onResumed() {
    if (_disposed || _conn != ConnectionState.connected) return;
    unawaited(_runBootstrap());
  }

  // -- bootstrap (App.tsx bootstrap effect) -------------------------------------------

  /// daemon.status → identity gate → room.list → phase, then pick the room to
  /// open: current room if still active, else the persisted last room if
  /// still active, else the first active room; clears selection when none.
  Future<void> _runBootstrap() async {
    final client = _client;
    if (client == null) return;
    final epoch = ++_bootstrapEpoch;
    try {
      final status = await verifyDaemonProtocol(client);
      if (_disposed || epoch != _bootstrapEpoch) return;
      _status = status;
      if (status.identity == null) {
        _phase = BootstrapPhase.noIdentity;
        notifyListeners();
        _maybeScheduleLinuxPackageReadiness();
        return;
      }
      final rooms = await client.roomList();
      if (_disposed || epoch != _bootstrapEpoch) return;
      _rooms = rooms;
      _seedUnread();
      if (rooms.isEmpty) {
        _phase = BootstrapPhase.noRooms;
        notifyListeners();
        _maybeScheduleLinuxPackageReadiness();
        return;
      }
      _phase = BootstrapPhase.ready;
      final active = rooms
          .where((r) => r.status != 'left' && r.status != 'removed')
          .toList();
      if (active.isEmpty) {
        _closeRoomStore();
        notifyListeners();
        _maybeScheduleLinuxPackageReadiness();
        return;
      }
      bool isActive(String? rid) =>
          rid != null && active.any((r) => r.roomId == rid);
      final target = isActive(currentRoomId)
          ? currentRoomId!
          : isActive(prefs.lastRoomId)
              ? prefs.lastRoomId!
              : active.first.roomId;
      notifyListeners();
      _maybeScheduleLinuxPackageReadiness();
      unawaited(openRoom(target));
    } on ProtocolMismatchError catch (e) {
      // Version skew is a HARD stop, not a transient: the connection stays
      // 'connected', so no reconnect will ever re-run this bootstrap. Route
      // to the boot screen's failed state (Retry re-checks the daemon).
      if (_disposed || epoch != _bootstrapEpoch) return;
      recordError('daemon.status', e);
      _setBoot(Boot.failed, BootStage.failedMismatch,
          mismatchActual: e.actual, mismatchExpected: e.expected,
          technical: '$e');
      _phase = BootstrapPhase.boot;
      notifyListeners();
    } catch (_) {
      // daemon.status failed (connection dropped mid-flight) — the reconnect
      // cycle re-triggers this bootstrap.
    }
  }

  /// Emit the package gate's proof only after the real desktop supervisor,
  /// authenticated WebSocket connection, and initial daemon.status/room-list
  /// bootstrap have all converged. A fresh gate profile legitimately routes to
  /// noIdentity; that is still a completed bootstrap, not a daemon-health-only
  /// success. Atomic replacement prevents the Node gate from observing JSON
  /// while it is being written.
  void _maybeScheduleLinuxPackageReadiness() {
    if (_linuxPackageReadinessWritten ||
        _linuxPackageReadinessScheduled ||
        _supervisor == null ||
        _boot != Boot.ready ||
        _phase == BootstrapPhase.boot ||
        _conn != ConnectionState.connected ||
        _status == null ||
        _bootPid == null ||
        _bootPort == null) {
      return;
    }
    final path = linuxPackageReadinessPathFrom(
      environment: Platform.environment,
      isLinux: Platform.isLinux,
      dataDir: dataDir,
    );
    if (path == null) return;

    _linuxPackageReadinessScheduled = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _linuxPackageReadinessScheduled = false;
      _writeLinuxPackageReadiness(path);
    });
    // addPostFrameCallback does not request a frame by itself. The gate wants
    // positive proof that Flutter rendered the resolved bootstrap phase, so
    // make sure this opt-in launch has a frame for the callback to follow.
    if (!WidgetsBinding.instance.hasScheduledFrame) {
      WidgetsBinding.instance.scheduleFrame();
    }
  }

  void _writeLinuxPackageReadiness(String path) {
    // State may have changed while Flutter rendered the resolved bootstrap
    // phase. Re-check everything in the post-frame callback before writing.
    if (_linuxPackageReadinessWritten ||
        _disposed ||
        _supervisor == null ||
        _boot != Boot.ready ||
        _phase == BootstrapPhase.boot ||
        _conn != ConnectionState.connected ||
        _status == null ||
        _bootPid == null ||
        _bootPort == null) {
      return;
    }

    final target = File(path);
    final temporary = File('$path.tmp.$pid');
    try {
      final payload = linuxPackageReadinessPayload(
        boot: _boot.name,
        phase: _phase.name,
        connection: _conn.name,
        frame: 'rendered',
        protocol: _status!.protocol,
        appPid: pid,
        daemonPid: _bootPid!,
        daemonPort: _bootPort!,
      );
      temporary.writeAsStringSync('${jsonEncode(payload)}\n', flush: true);
      temporary.renameSync(target.path);
      _linuxPackageReadinessWritten = true;
    } catch (error) {
      // This opt-in proof must not perturb the normal application lifecycle.
      // The package gate captures stderr and will fail with this diagnostic if
      // it cannot observe the marker.
      try {
        if (temporary.existsSync()) temporary.deleteSync();
      } catch (_) {
        // Best-effort cleanup of a gate-only temporary file.
      }
      debugPrint('Could not write Linux package readiness marker: $error');
    }
  }

  /// Onboarding steps call this after identity.create / room.create /
  /// room.join succeed (App.tsx `onAdvance` → bootNonce bump).
  void advanceOnboarding() => unawaited(_runBootstrap());

  // -- room selection ----------------------------------------------------------------

  /// Open [roomId]: replaces the current [RoomStore] (the old one is disposed,
  /// so its in-flight results are dropped — the roomIdRef guard) and persists
  /// the last-room pref. Re-opening the SAME room also re-runs `room.open`
  /// (reconnect re-sync relies on this).
  Future<void> openRoom(String roomId) async {
    final client = _client;
    if (client == null) return;
    _room?.dispose();
    final supervisor = _supervisor;
    final store = RoomStore(
      client: client,
      roomId: roomId,
      pending: _pendingByRoom.putIfAbsent(roomId, PendingMessages.new),
      recordError: recordError,
      selfId: () => selfId,
      // Null without a supervisor: `/api/files/local` preview URLs need the
      // daemon's HTTP origin, which the mobile in-process engine does not
      // serve — fetched files land on disk but cannot be previewed there yet
      // (RoomStore tolerates the null; an engine local_file accessor is the
      // tracked follow-up).
      localFileUrl: supervisor == null
          ? null
          : (rid, fid) =>
              supervisor.localFileUrl(roomId: rid, fileId: fid).toString(),
      stageAndShare: _stageAndShareOverride ??
          (supervisor == null
              ? null
              : ({required roomId, required sourcePath, name, mime}) =>
                  supervisor.shareUserFile(client,
                      roomId: roomId,
                      sourcePath: sourcePath,
                      name: name,
                      mime: mime)),
      onRoomsChanged: () => unawaited(refreshRooms()),
    );
    _room = store;
    prefs.lastRoomId = roomId;
    notifyListeners();
    await store.open();
    // Viewing a room clears its unread: advance the last-seen mark to the
    // newest event now loaded (docs/room-attention.md, decision 3;
    // markRoomSeen never moves the mark backward). Guarded on the store still
    // being current — if the user switched away before it loaded, they did not
    // view it, so it correctly stays unread (mirrors the reference roomIdRef
    // guard and the store's own dispose seam).
    if (_disposed || _room != store) return;
    final newestTs = store.timeline.fold<int>(0, (m, e) => e.ts > m ? e.ts : m);
    if (newestTs > 0) {
      prefs.markRoomSeen(roomId, newestTs);
      // Re-notify so the room list clears the just-opened room's unread dot in
      // place (the mark advanced after the pre-open notifyListeners above).
      notifyListeners();
    }
  }

  /// Sidebar/fleet room clicks: departed rooms (left/removed) are never
  /// opened; a different active room switches via [openRoom].
  void selectRoom(String roomId) {
    RoomSummary? summary;
    for (final r in _rooms) {
      if (r.roomId == roomId) {
        summary = r;
        break;
      }
    }
    if (summary?.status == 'left' || summary?.status == 'removed') return;
    if (roomId != currentRoomId) unawaited(openRoom(roomId));
  }

  /// After a successful `room.leave` (App.tsx `leaveCurrentRoom`): clears
  /// every piece of room state (incl. closingPipes via store disposal),
  /// removes the last-room pref, refreshes rooms, re-fetches daemon.status.
  void leaveCurrentRoom() {
    _closeRoomStore();
    prefs.lastRoomId = null;
    notifyListeners();
    unawaited(refreshRooms());
    unawaited(refreshStatus());
  }

  void _closeRoomStore() {
    _room?.dispose();
    _room = null;
  }

  // -- refresh helpers -----------------------------------------------------------------

  /// `room.list` — errors swallowed (transient; the next push retries).
  Future<void> refreshRooms() async {
    final client = _client;
    if (client == null) return;
    try {
      final rooms = await client.roomList();
      if (_disposed) return;
      _rooms = rooms;
      _seedUnread();
      notifyListeners();
    } catch (_) {/* transient */}
  }

  /// Establish the device-local unread baseline the first time each listed
  /// room appears (docs/room-attention.md, decision 3): a backlog synced
  /// before this device ever saw the room must not read as unread.
  /// [PrefsStore.seedRoomSeen] writes only when no mark exists, so a returning
  /// user's advanced marks — the whole basis of an honest unread dot — are
  /// untouched. Called after every `_rooms = …` assignment.
  void _seedUnread() {
    for (final room in _rooms) {
      final ts = room.lastEventTs;
      if (ts != null) prefs.seedRoomSeen(room.roomId, ts);
    }
  }

  /// `daemon.status` — errors swallowed.
  Future<void> refreshStatus() async {
    final client = _client;
    if (client == null) return;
    try {
      final status = await client.daemonStatus();
      if (_disposed) return;
      _status = status;
      notifyListeners();
    } catch (_) {/* transient */}
  }

  // -- push routing ------------------------------------------------------------------

  void _onRoomEvent(RoomEventPush push) {
    final store = _room;
    // Only the current room folds pushes (App.tsx); other rooms re-baseline
    // from room.open when next selected.
    if (store != null && store.roomId == push.roomId) {
      store.handleRoomEvent(push.event);
      // You are viewing this room, so an event arriving here is seen, not
      // unread — advance the mark as it arrives (docs/room-attention.md,
      // decision 3). markRoomSeen never moves the mark backward.
      prefs.markRoomSeen(push.roomId, push.event.ts);
    }
  }

  void _onPeersChanged(PeersChangedPush push) {
    final store = _room;
    if (store != null && store.roomId == push.roomId) {
      store.handlePeersChanged(push.peers);
    }
  }

  // -- names (local aliases; NEVER wire data) --------------------------------------------

  /// Display-name resolution order: for self the device-local label falls back
  /// to the localized 'You' (never the raw hex id — docs/self-label.md); for
  /// peers it is local alias → shortId (against a real daemon there are no
  /// seeded suggestions). Takes the ambient catalog so the self-name follows
  /// the text locale.
  String displayName(AppStrings s, String identityId) {
    if (isSelf(identityId)) return prefs.aliasFor(identityId) ?? s.commonYou;
    return prefs.aliasFor(identityId) ?? shortId(identityId);
  }

  bool isSelf(String identityId) => selfId != null && identityId == selfId;

  /// The device-local self label: just the self identity's own alias, '' when
  /// unset (docs/self-label.md). Editing it from onboarding/settings reuses the
  /// same local-alias write — no new store, no wire call.
  String get selfLabel {
    final id = selfId;
    return id == null ? '' : (prefs.aliasFor(id) ?? '');
  }

  /// Set/clear the self label (empty ⇒ falls back to 'You'); a no-op before the
  /// identity is known. Reuses [setAlias], which trims and clears.
  void setSelfLabel(String label) {
    final id = selfId;
    if (id != null) setAlias(id, label);
  }

  /// Store/clear a local alias (Rename modal Save / Clear alias).
  void setAlias(String identityId, String? name) {
    prefs.setAlias(identityId, name);
    notifyListeners();
  }

  // -- diagnostics -------------------------------------------------------------------------

  /// Shape [error], remember it for the diagnostics report (Settings "Last
  /// captured error"), and return the shaped error.
  RequestError recordError(String context, Object error) {
    final err = errorShape(error);
    _lastDiagnosticError = DiagnosticEvent(
      context: context,
      code: err.code,
      message: err.message,
      hint: err.hint,
      // UTC with the trailing 'Z', like the reference's toISOString().
      at: DateTime.now().toUtc().toIso8601String(),
    );
    if (!_disposed) notifyListeners();
    return err;
  }

  /// Record a LOCAL (client-side) action failure — a clipboard write, a
  /// url_launcher launch, a prefs write, an empty-file share — for the
  /// diagnostics report WITHOUT ever letting raw platform text (file paths,
  /// invite tickets, message bodies, full identities) reach the
  /// [DiagnosticEvent]. Unlike [recordError], which shapes and stores the raw
  /// exception's message, this takes a SYNTHETIC [context] + [code] and keeps
  /// the stored message empty. Returns a [RequestError] the caller can render;
  /// its user-facing copy is composed at the call site from the catalog, never
  /// from the daemon's English text. Callers pass a fixed friendly [hint] only
  /// when it is catalog copy, never raw platform text.
  RequestError recordLocalFailure(String context, String code, {String? hint}) {
    _lastDiagnosticError = DiagnosticEvent(
      context: context,
      code: code,
      message: '', // synthetic — raw platform text must never land here
      hint: hint,
      // UTC with the trailing 'Z', matching recordError's convention.
      at: DateTime.now().toUtc().toIso8601String(),
    );
    if (!_disposed) notifyListeners();
    return RequestError(code, '', hint: hint);
  }

  /// The privacy-safe markdown report (package `buildDiagnostics` with the
  /// app-side runtime fields filled in).
  String buildDiagnosticsReport() {
    final store = _room;
    return buildDiagnostics(DiagnosticsInput(
      generatedAt: DateTime.now().toUtc().toIso8601String(),
      uiVersion: kAppVersion,
      browser: 'Flutter ${kDebugMode ? 'debug' : 'release'} (Dart ${Platform.version.split(' ').first})',
      platform: '${Platform.operatingSystem} ${Platform.operatingSystemVersion}',
      transport: transportDescription,
      connection: _conn,
      status: _status,
      rooms: _rooms,
      currentRoomId: currentRoomId,
      members: store?.members ?? const [],
      files: store?.files ?? const [],
      fetches: store?.fetches ?? const {},
      pipes: store?.pipes ?? const [],
      peers: store?.peers ?? const [],
      lastError: _lastDiagnosticError,
    ));
  }

  // -- teardown ----------------------------------------------------------------------------

  Future<AppExitResponse> _onExitRequested() async {
    await teardown();
    return AppExitResponse.exit;
  }

  /// Graceful teardown, in the contract order: `client.stop()` first (rejects
  /// queued work), THEN `supervisor.shutdown()` (SIGTERM → bounded wait →
  /// SIGKILL). Idempotent.
  Future<void> teardown() async {
    if (_tearingDown) return;
    _tearingDown = true;
    try {
      await _client?.stop();
    } catch (_) {/* transport already down */}
    try {
      await _supervisor?.shutdown();
    } catch (_) {/* daemon already gone */}
  }

  @override
  void dispose() {
    _disposed = true;
    for (final sub in _subs) {
      sub.cancel();
    }
    _subs.clear();
    _lifecycle?.dispose();
    _room?.dispose();
    unawaited(teardown());
    super.dispose();
  }
}

/// Exposes the [DaemonSession] to the widget tree; rebuilds dependents on
/// every session notification. Read with [SessionScope.of].
class SessionScope extends InheritedNotifier<DaemonSession> {
  const SessionScope({super.key, required DaemonSession session, required super.child})
      : super(notifier: session);

  static DaemonSession of(BuildContext context) {
    final scope = context.dependOnInheritedWidgetOfExactType<SessionScope>();
    assert(scope != null, 'SessionScope missing above this context');
    return scope!.notifier!;
  }
}
