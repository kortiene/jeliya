/// In-memory fixture [Client] for widget tests and demos: the whole app runs
/// with no daemon. Ported 1:1 from `ui/src/lib/mock.ts` (the reference mock
/// oracle) — it answers every PROTOCOL.md method from the same fixtures that
/// echo the design mockups, and passes the same conformance corpus
/// (`ui/src/lib/conformance/corpus.json`, mock-compatible scenarios).
///
/// Deliberate deviations from mock.ts, kept small and test-oriented:
///
/// 1. **Echo beats response.** The `room.event` echo of an own write
///    (`message.send` / `status.post` / `file.share` / …) is delivered to push
///    listeners *before* the method response resolves. PROTOCOL.md (ordering
///    hazard 2) makes this ordering normative-possible, and the corpus accepts
///    either order — the mock exercises the harder one so app-side
///    pending-message reconciliation by `event_id` is honestly tested.
///    (mock.ts emits the push 30ms *after* the response.)
/// 2. **Deterministic by construction.** No random latencies (fixed,
///    injectable [MockClient.callLatency] etc.), an injectable millisecond
///    clock ([MockClient.new] `now`), and a per-instance event counter — two
///    clients built with the same clock produce byte-identical fixtures.
/// 3. `?mock=fresh` becomes the [MockClient.new] `fresh` flag, and the
///    `suggestedNames` global seeding becomes data ([MockPeople.suggestedNames])
///    for the app layer to consume.
/// 4. The live-update demo simulation can be disabled
///    (`simulateLiveActivity: false`) so widget tests stay timer-free.
library;

import 'dart:async';
import 'dart:math' as math;

import '../src/models.dart';
import '../src/protocol.dart';

// -- deterministic ids ----------------------------------------------------------

/// Port of mock.ts `hex(seed, len)`: absorb the whole seed, then squeeze —
/// distinct seeds give distinct ids. Uses 32-bit wrapping arithmetic like JS
/// `Math.imul`/`>>> 0`; relies on native 64-bit ints (Dart VM / Flutter
/// native), which is all this package targets.
String _hex(String seed, int len) {
  const mask = 0xFFFFFFFF;
  int imul(int a, int b) => (a * b) & mask;
  var h1 = 2166136261;
  var h2 = 0x9e3779b9;
  for (var i = 0; i < seed.length; i++) {
    final c = seed.codeUnitAt(i);
    h1 = imul(h1 ^ c, 16777619);
    h2 = (imul(h2 ^ c, 2246822519) + 0x9e3779b9) & mask;
  }
  final out = StringBuffer();
  var i = 0;
  while (out.length < len) {
    h1 = imul(h1 ^ (h2 >> 13) ^ i, 16777619);
    h2 = (imul(h2 ^ (h1 >> 11), 2246822519) + i) & mask;
    out.write(((h1 ^ h2) & mask).toRadixString(16).padLeft(8, '0'));
    i += 1;
  }
  return out.toString().substring(0, len);
}

const String _base32 = 'abcdefghijklmnopqrstuvwxyz234567';

String _base32ish(String seed, int len) {
  final h = _hex(seed, len * 2);
  final out = StringBuffer();
  for (var i = 0; i < len; i++) {
    out.write(_base32[int.parse(h.substring(i * 2, i * 2 + 2), radix: 16) % 32]);
  }
  return out.toString();
}

String _fid(String seed) => 'file_${_hex(seed, 32)}';
String _pid(String seed) => _hex(seed, 32);

/// Parse an invite expiry (a number of seconds, or a duration string like
/// `"24h"` / `"90m"` / `"3600"`) into seconds, or null for none — mirrors the
/// daemon's `expiry_spec` (rpc.rs) via mock.ts `parseExpirySeconds`.
int? _parseExpirySeconds(Object? expiry) {
  if (expiry is num) return expiry > 0 ? expiry.toInt() : null;
  if (expiry is! String) return null;
  final m = RegExp(r'^(\d+)\s*([smhd])?$', caseSensitive: false).firstMatch(expiry.trim());
  if (m == null) return null;
  final n = int.parse(m.group(1)!);
  final unit = (m.group(2) ?? 's').toLowerCase();
  final mult = unit == 'd'
      ? 86400
      : unit == 'h'
          ? 3600
          : unit == 'm'
              ? 60
              : 1;
  return n > 0 ? n * mult : null;
}

// -- people -----------------------------------------------------------------------

/// One fixture participant (mock.ts `Person`): a deterministic identity,
/// device, and endpoint id plus a local-only display name.
class MockPerson {
  const MockPerson({
    required this.identityId,
    required this.deviceId,
    required this.endpointId,
    required this.role,
    required this.name,
  });

  final String identityId;
  final String deviceId;
  final String endpointId;

  /// One of [Roles.all].
  final String role;

  /// Display name for the app's local alias store — never wire data.
  final String name;
}

MockPerson _person(String seed, String role, String name) => MockPerson(
      identityId: _hex('$seed-identity', 64),
      deviceId: _hex('$seed-device', 64),
      endpointId: _hex('$seed-endpoint', 64),
      role: role,
      name: name,
    );

/// The fixture cast. `alex` is the local identity a non-fresh [MockClient]
/// boots with.
abstract final class MockPeople {
  static final MockPerson alex = _person('alex', Roles.owner, 'Alex K.');
  static final MockPerson maya = _person('maya', Roles.member, 'Maya R.');
  static final MockPerson sam = _person('sam', Roles.member, 'Sam D.');
  static final MockPerson backendAgent = _person('backend-agent', Roles.agent, 'Backend Agent');
  static final MockPerson frontendAgent = _person('frontend-agent', Roles.agent, 'Frontend Agent');
  static final MockPerson qaAgent = _person('qa-agent', Roles.agent, 'QA Agent');
  static final MockPerson researchAgent = _person('research-agent', Roles.agent, 'Research Agent');

  /// A failed-status agent for the fleet's Needs Attention section (#69). Kept
  /// out of [everyone] deliberately: it lives only in the Agent Workspace room,
  /// so the MVP room's roster/agent counts are unchanged.
  static final MockPerson deployAgent = _person('deploy-agent', Roles.agent, 'Deploy Agent');

  static final List<MockPerson> everyone = List.unmodifiable(<MockPerson>[
    alex,
    maya,
    sam,
    backendAgent,
    frontendAgent,
    qaAgent,
    researchAgent,
  ]);

  /// `identity_id → display name` — the Dart counterpart of mock.ts seeding
  /// names.ts `suggestedNames`. The protocol has no display names; an app can
  /// seed its local alias store from this so demo content echoes the mockups.
  static final Map<String, String> suggestedNames = Map.unmodifiable({
    for (final p in everyone) p.identityId: p.name,
    deployAgent.identityId: deployAgent.name,
  });
}

// -- room fixture -------------------------------------------------------------------

final String _mainRoomId = 'blake3:${_hex('room-build-iroh-rooms-mvp', 64)}';

final String _fRelease = _fid('release-notes.txt');
final String _fProtocol = _fid('room-protocol.md');
final String _fWireframe = _fid('wireframe.png');
final String _fPrd = _fid('PRD_v0.2.pdf');
final String _fReport = _fid('test-report.json');

final String _pipePreview = _pid('pipe-frontend-preview');
final String _pipeLogs = _pid('pipe-logs-stream');

class _MockRoom {
  _MockRoom({
    required this.roomId,
    required this.name,
    required this.myRole,
    required this.members,
    required this.timeline,
    List<_MockFile>? files,
    List<_MockPipe>? pipes,
    List<PeerStatus>? peers,
  })  : files = files ?? [],
        pipes = pipes ?? [],
        peers = peers ?? [];

  final String roomId;
  final String name;
  final String myRole;
  List<Member> members;
  final List<TimelineEvent> timeline;
  final List<_MockFile> files;
  final List<_MockPipe> pipes;
  List<PeerStatus> peers;
  bool open = false;
  final List<Timer> timers = [];
  bool simulated = false;
}

/// Mutable `file.list` row (mock.ts mutates `fetched`/`local_*` on fetch);
/// serialized through [FileEntry.toJson] so the wire shape stays canonical.
class _MockFile {
  _MockFile({
    required this.fileId,
    required this.name,
    required this.size,
    required this.mime,
    required this.senderId,
    required this.ts,
    required this.available,
    required this.providers,
  });

  final String fileId;
  final String name;
  final int size;
  final String mime;
  final String senderId;
  final int ts;
  final bool available;
  final int providers;
  bool fetched = false;
  String? localPath;
  int? localBytes;
  int? fetchedAtMs;

  Map<String, dynamic> toJson() => FileEntry(
        fileId: fileId,
        name: name,
        size: size,
        mime: mime,
        senderId: senderId,
        ts: ts,
        available: available,
        providers: providers,
        fetched: fetched,
        localPath: localPath,
        localBytes: localBytes,
        fetchedAtMs: fetchedAtMs,
      ).toJson();
}

/// Mutable `pipe.list` row (`state`/`connected` flip on connect/close).
class _MockPipe {
  _MockPipe({
    required this.pipeId,
    required this.target,
    required this.openedBy,
    this.authorizedPeer,
    this.connected = false,
  });

  final String pipeId;
  final String target;
  final String openedBy;
  final String? authorizedPeer;
  String state = PipeStates.open;
  bool connected;

  Map<String, dynamic> toJson() => PipeEntry(
        pipeId: pipeId,
        target: target,
        openedBy: openedBy,
        authorizedPeer: authorizedPeer,
        state: state,
        connected: connected,
      ).toJson();
}

/// What a minted `invite.create` ticket actually redeems — looked up by
/// `room.join` so it lands in the room it was really minted for instead of a
/// fabricated one. `expiresAt: null` means no expiry was requested.
class _TicketEntry {
  _TicketEntry({
    required this.roomId,
    required this.identityId,
    required this.role,
    this.expiresAt,
  });

  final String roomId;
  final String identityId;
  final String role;
  final int? expiresAt;
}

// -- fleet liveness derivation (docs/agent-orchestration.md §1.2) --------------------
//
// Mirrors the daemon's read-time rule exactly: primary signal = a currently
// connected peer in an OPEN room, secondary = the ts of the latest real
// agent_status event. A working-class latest label is never sufficient on its
// own — peer state overrides the last posted label.

const int _staleWorkingMs = 20 * 60000;

const Map<String, int> _livenessRank = {
  LivenessValues.working: 0,
  LivenessValues.onlineIdle: 1,
  LivenessValues.stale: 2,
  LivenessValues.offline: 3,
};

/// Working-class iff the label is exactly `working`; unknown labels are
/// idle-class (§1.1).
bool _isWorkingClass(String? label) => label == 'working';

String _deriveLiveness(bool connected, TimelineEvent? latest, int now) {
  if (!connected) {
    // Rows 1–3: no live peer — never online, never working.
    if (latest != null && _isWorkingClass(latest.label)) return LivenessValues.stale;
    return LivenessValues.offline;
  }
  if (latest != null && _isWorkingClass(latest.label)) {
    // Rows 4–5: connected + working-class label — fresh means working.
    return now - latest.ts <= _staleWorkingMs ? LivenessValues.working : LivenessValues.stale;
  }
  // Row 6: connected, idle-class latest (or no status yet).
  return LivenessValues.onlineIdle;
}

class _FleetEntry {
  final List<FleetAgentRoom> rooms = [];
  String liveness = LivenessValues.offline;
  FleetAgentLatest? latest;
  int? lastSeenTs;
}

// -- the client -----------------------------------------------------------------------

/// The in-memory mock [Client]. Answers every PROTOCOL.md method from
/// fixtures; never touches the network or spawns a process.
///
/// The mock answers [call] regardless of connection state (parity with
/// mock.ts) — [start]/[stop] only drive the [states] transitions an app's
/// connection banner watches.
class MockClient implements Client {
  /// [fresh] starts with no identity and no rooms so the onboarding flow can
  /// be exercised too (mock.ts `?mock=fresh`). [now] is the millisecond-epoch
  /// clock every id, timestamp, and expiry check reads — inject a fixed or
  /// manual clock for fully deterministic fixtures.
  MockClient({
    bool fresh = false,
    int Function()? now,
    this.connectLatency = const Duration(milliseconds: 200),
    this.callLatency = const Duration(milliseconds: 60),
    this.fetchLatency = const Duration(milliseconds: 900),
    this.simulateLiveActivity = true,
  }) : _now = now ?? _wallClock {
    if (!fresh) {
      _identity = Identity(
        identityId: MockPeople.alex.identityId,
        deviceId: MockPeople.alex.deviceId,
      );
      final main = _buildMainRoom();
      _rooms[main.roomId] = main;
      final workspace = _buildSideRoom(
        'agent-workspace',
        'Agent Workspace',
        [
          MockPeople.alex,
          MockPeople.backendAgent,
          MockPeople.frontendAgent,
          MockPeople.qaAgent,
          MockPeople.deployAgent,
        ],
        Roles.owner,
        'Scratch room for agent runs. Post statuses here.',
      );
      // Failed-runner fixture (#69): a connected agent whose latest signed
      // status is a failure. It must surface in Needs Attention — the exact
      // red-tone case the old blue-only filter silently dropped.
      workspace.timeline.add(
        _ev(workspace.roomId, _at(10, 6), MockPeople.deployAgent, TimelineKinds.agentStatus,
            label: 'deploy_failed',
            statusMessage: 'Deploy to staging failed: image build returned a non-zero exit.'),
      );
      final review = _buildSideRoom(
        'product-review',
        'Product Review',
        [MockPeople.maya, MockPeople.alex, MockPeople.sam, MockPeople.backendAgent, MockPeople.qaAgent],
        Roles.member,
        'Weekly product review — drop artifacts before Friday.',
      );
      final design = _buildSideRoom(
        'design-system',
        'Design System',
        [MockPeople.sam, MockPeople.alex, MockPeople.maya, MockPeople.frontendAgent],
        Roles.member,
        'Tokens v2 exploration lives here.',
      );
      final research = _buildSideRoom(
        'research-lab',
        'Research Lab',
        [MockPeople.alex, MockPeople.researchAgent],
        Roles.owner,
        'P2P performance benchmarks and optimization research.',
      );
      // Crashed-runner fixture: the latest status is working-class but the
      // peer is gone — the fleet view must report `stale`, never `working`.
      research.peers = [
        for (final p in research.peers)
          p.endpointId == MockPeople.researchAgent.endpointId
              ? PeerStatus(endpointId: p.endpointId, state: PeerStates.offline, identityId: p.identityId)
              : p,
      ];
      research.timeline.addAll([
        _ev(research.roomId, _at(8, 40), MockPeople.researchAgent, TimelineKinds.agentStatus,
            label: 'working',
            statusMessage: 'Collecting NAT traversal benchmarks across relay paths.',
            progress: 20),
        _ev(research.roomId, _at(9, 10), MockPeople.researchAgent, TimelineKinds.agentStatus,
            label: 'working',
            statusMessage: 'Benchmarks 12/30 done; profiling gossip fanout next.',
            progress: 45),
        _ev(research.roomId, _at(9, 35), MockPeople.researchAgent, TimelineKinds.agentStatus,
            label: 'working',
            statusMessage: 'Profiling run 3 in flight — writing up interim notes.',
            progress: 60),
      ]);
      for (final room in [workspace, review, design, research]) {
        _rooms[room.roomId] = room;
      }
    }
  }

  /// The "Build Iroh Rooms MVP" fixture room id — stable across instances.
  static final String mainRoomId = _mainRoomId;

  static int _wallClock() => DateTime.now().millisecondsSinceEpoch;

  /// Simulated connect handshake time before [start] settles `connected`.
  final Duration connectLatency;

  /// Fixed per-call answer latency (mock.ts used 60–180ms of jitter).
  final Duration callLatency;

  /// Simulated `file.fetch` transfer time (mock.ts used 900–1400ms).
  final Duration fetchLatency;

  /// Whether opening the main fixture room starts the live-update demo timers
  /// (new agent statuses at 6s/26s, QA peer flap at 13s/19s). Turn off in
  /// widget tests that must stay timer-free.
  final bool simulateLiveActivity;

  final int Function() _now;
  final RegExp _hex64Re = RegExp(r'^[0-9a-f]{64}$', caseSensitive: false);

  Identity? _identity;
  final Map<String, _MockRoom> _rooms = {};
  final Map<String, _TicketEntry> _tickets = {};
  int _portSeq = 41732;
  int _eventSeq = 0;

  ConnectionState _state = ConnectionState.disconnected;
  final StreamController<ConnectionState> _states = StreamController.broadcast();
  final StreamController<Push> _pushes = StreamController.broadcast();
  Timer? _startTimer;
  Completer<void>? _connecting;
  final Map<Completer<dynamic>, Timer> _inFlight = {};

  @override
  ConnectionState get state => _state;
  @override
  Stream<ConnectionState> get states => _states.stream;
  @override
  Stream<Push> get pushes => _pushes.stream;
  @override
  String describe() => 'mock fixtures (in-memory) — no daemon';

  @override
  Future<void> start() {
    if (_state == ConnectionState.connected) return Future.value();
    final existing = _connecting;
    if (existing != null) return existing.future;
    final completer = Completer<void>();
    _connecting = completer;
    _setState(ConnectionState.connecting);
    _startTimer = Timer(connectLatency, () {
      _startTimer = null;
      _connecting = null;
      _setState(ConnectionState.connected);
      completer.complete();
    });
    return completer.future;
  }

  @override
  Future<void> stop() async {
    _startTimer?.cancel();
    _startTimer = null;
    final connecting = _connecting;
    _connecting = null;
    for (final room in _rooms.values) {
      for (final t in room.timers) {
        t.cancel();
      }
      room.timers.clear();
    }
    _setState(ConnectionState.disconnected);
    // In-flight calls fail like WsClient's on a dead socket (contract: a
    // stopped client rejects with connection_lost, never resolves silently).
    final inFlight = Map.of(_inFlight);
    _inFlight.clear();
    for (final entry in inFlight.entries) {
      entry.value.cancel();
      if (!entry.key.isCompleted) {
        entry.key.completeError(
            RequestError('connection_lost', 'client stopped', hint: 'is jeliyad running?'));
      }
    }
    // Never leave a start() awaiter hanging (same contract as WsClient.stop).
    if (connecting != null && !connecting.isCompleted) connecting.complete();
  }

  @override
  Future<dynamic> call(String method, [Map<String, dynamic>? params]) {
    final completer = Completer<dynamic>();
    // Contract parity with WsClient: a client that is neither connected nor
    // connecting fails immediately with connection_lost instead of serving
    // fixtures from beyond the grave.
    if (_state == ConnectionState.disconnected && _connecting == null) {
      completer.completeError(
          RequestError('connection_lost', 'client is stopped', hint: 'call start() first'));
      return completer.future;
    }
    late final Timer timer;
    timer = Timer(callLatency, () {
      _inFlight.remove(completer);
      try {
        completer.complete(_dispatch(method, params ?? const {}));
      } catch (e, st) {
        completer.completeError(e, st);
      }
    });
    _inFlight[completer] = timer;
    return completer.future;
  }

  // -- fixtures ----------------------------------------------------------------

  /// ms epoch for "yesterday" at h:m local time. Anchoring on yesterday (not
  /// today) guarantees every fixture timestamp is strictly before now
  /// regardless of what time the demo is opened. Relative ordering between
  /// fixtures is unaffected since they all shift by the same 24h.
  int _at(int h, int m, [int s = 0]) {
    final d = DateTime.fromMillisecondsSinceEpoch(_now() - 24 * 60 * 60 * 1000);
    return DateTime(d.year, d.month, d.day, h, m, s).millisecondsSinceEpoch;
  }

  TimelineEvent _ev(
    String roomId,
    int ts,
    MockPerson sender,
    String kind, {
    String? body,
    String? label,
    String? statusMessage,
    num? progress,
    List<String> artifacts = const [],
    FileRef? file,
    PipeRef? pipe,
    MemberRef? member,
  }) {
    _eventSeq += 1;
    return TimelineEvent(
      eventId: _hex('$roomId:$kind:$_eventSeq', 64),
      roomId: roomId,
      ts: ts,
      sender: Sender(identityId: sender.identityId, deviceId: sender.deviceId, role: sender.role),
      kind: kind,
      body: body,
      label: label,
      statusMessage: statusMessage,
      progress: progress,
      artifacts: artifacts,
      file: file,
      pipe: pipe,
      member: member,
    );
  }

  Member _member(MockPerson p, [String status = 'active']) =>
      Member(identityId: p.identityId, role: p.role, status: status);

  _MockRoom _buildMainRoom() {
    final r = _mainRoomId;
    final alex = MockPeople.alex;
    final maya = MockPeople.maya;
    final sam = MockPeople.sam;
    final backend = MockPeople.backendAgent;
    final frontend = MockPeople.frontendAgent;
    final qa = MockPeople.qaAgent;
    final timeline = <TimelineEvent>[
      _ev(r, _at(8, 45), alex, TimelineKinds.roomCreated),
      _ev(r, _at(8, 50), maya, TimelineKinds.memberJoined,
          member: MemberRef(identityId: maya.identityId, role: Roles.member)),
      _ev(r, _at(8, 52), sam, TimelineKinds.memberJoined,
          member: MemberRef(identityId: sam.identityId, role: Roles.member)),
      _ev(r, _at(8, 54), backend, TimelineKinds.memberJoined,
          member: MemberRef(identityId: backend.identityId, role: Roles.agent)),
      _ev(r, _at(8, 55), frontend, TimelineKinds.memberJoined,
          member: MemberRef(identityId: frontend.identityId, role: Roles.agent)),
      _ev(r, _at(8, 56), qa, TimelineKinds.memberJoined,
          member: MemberRef(identityId: qa.identityId, role: Roles.agent)),
      _ev(r, _at(8, 58), backend, TimelineKinds.fileShared,
          file: FileRef(fileId: _fRelease, name: 'release-notes.txt', size: 8 * 1024, mime: 'text/plain')),
      _ev(r, _at(9, 12), alex, TimelineKinds.fileShared,
          file: FileRef(fileId: _fProtocol, name: 'room-protocol.md', size: 12 * 1024, mime: 'text/markdown')),
      _ev(r, _at(9, 48), sam, TimelineKinds.fileShared,
          file: FileRef(fileId: _fWireframe, name: 'wireframe.png', size: 320 * 1024, mime: 'image/png')),
      _ev(r, _at(9, 5), backend, TimelineKinds.agentStatus,
          label: 'working',
          statusMessage: 'Scaffolding room invite flow and peer discovery.',
          progress: 15),
      _ev(r, _at(9, 25), frontend, TimelineKinds.agentStatus,
          label: 'working',
          statusMessage: 'Blocking out the room shell and timeline components.',
          progress: 20),
      _ev(r, _at(9, 40), backend, TimelineKinds.agentStatus,
          label: 'working',
          statusMessage: 'File manifest sync wired; starting invite flow tests.',
          progress: 35),
      _ev(r, _at(9, 55), qa, TimelineKinds.agentStatus,
          label: 'working',
          statusMessage: 'Writing test suite v1 against the invite + sync paths.',
          progress: 40),
      _ev(r, _at(10, 2), alex, TimelineKinds.message,
          body:
              'Kicked off the rooms protocol spec and initial backend scaffolding. Next up: agent orchestration + pipe manager.'),
      _ev(r, _at(10, 15), backend, TimelineKinds.agentStatus,
          label: 'working',
          statusMessage:
              'Implemented room invite flow, peer discovery, and file manifest sync. Running integration tests…',
          progress: 60),
      _ev(r, _at(10, 20), backend, TimelineKinds.pipeOpened,
          pipe: PipeRef(pipeId: _pipeLogs, target: '127.0.0.1:4000', authorizedPeer: alex.identityId)),
      _ev(r, _at(10, 28), maya, TimelineKinds.message,
          body: "Here's the updated PRD with rooms, pipes, and agent runtime."),
      _ev(r, _at(10, 28, 20), maya, TimelineKinds.fileShared,
          file: FileRef(fileId: _fPrd, name: 'PRD_v0.2.pdf', size: 1887437, mime: 'application/pdf')),
      _ev(r, _at(10, 34), frontend, TimelineKinds.agentStatus,
          label: 'preview_ready',
          statusMessage: 'UI scaffold is up with live data. Exposed preview on a pipe.'),
      _ev(r, _at(10, 34, 30), frontend, TimelineKinds.pipeOpened,
          pipe: PipeRef(pipeId: _pipePreview, target: '127.0.0.1:3000', authorizedPeer: alex.identityId)),
      _ev(r, _at(10, 45), qa, TimelineKinds.agentStatus,
          label: 'awaiting_review',
          statusMessage: 'Completed test suite v1. Summary attached.',
          artifacts: [_fReport]),
      _ev(r, _at(10, 45, 30), qa, TimelineKinds.fileShared,
          file: FileRef(fileId: _fReport, name: 'test-report.json', size: 85 * 1024, mime: 'application/json')),
      // A fresh working-class status so the fleet dashboard has one truthfully
      // "working" agent (connected peer + fresh working label, §1.2 row 4).
      _ev(r, math.max(_at(10, 50), _now() - 90000), backend, TimelineKinds.agentStatus,
          label: 'working',
          statusMessage: 'Sync convergence suite running (14/24 green).',
          progress: 68),
    ];

    final files = <_MockFile>[
      _MockFile(fileId: _fRelease, name: 'release-notes.txt', size: 8 * 1024, mime: 'text/plain', senderId: backend.identityId, ts: _at(8, 58), available: false, providers: 1),
      _MockFile(fileId: _fProtocol, name: 'room-protocol.md', size: 12 * 1024, mime: 'text/markdown', senderId: alex.identityId, ts: _at(9, 12), available: true, providers: 3),
      _MockFile(fileId: _fWireframe, name: 'wireframe.png', size: 320 * 1024, mime: 'image/png', senderId: sam.identityId, ts: _at(9, 48), available: true, providers: 2),
      _MockFile(fileId: _fPrd, name: 'PRD_v0.2.pdf', size: 1887437, mime: 'application/pdf', senderId: maya.identityId, ts: _at(10, 28), available: true, providers: 4),
      _MockFile(fileId: _fReport, name: 'test-report.json', size: 85 * 1024, mime: 'application/json', senderId: qa.identityId, ts: _at(10, 45), available: true, providers: 2),
    ];

    final pipes = <_MockPipe>[
      _MockPipe(pipeId: _pipePreview, target: '127.0.0.1:3000', openedBy: frontend.identityId, authorizedPeer: alex.identityId, connected: true),
      _MockPipe(pipeId: _pipeLogs, target: '127.0.0.1:4000', openedBy: backend.identityId, authorizedPeer: alex.identityId),
    ];

    final peers = <PeerStatus>[
      PeerStatus(endpointId: maya.endpointId, state: PeerStates.connected, path: PeerPaths.direct, identityId: maya.identityId),
      PeerStatus(endpointId: sam.endpointId, state: PeerStates.connected, path: PeerPaths.relay, identityId: sam.identityId),
      PeerStatus(endpointId: backend.endpointId, state: PeerStates.connected, path: PeerPaths.direct, identityId: backend.identityId),
      PeerStatus(endpointId: frontend.endpointId, state: PeerStates.connected, path: PeerPaths.direct, identityId: frontend.identityId),
      PeerStatus(endpointId: qa.endpointId, state: PeerStates.offline, identityId: qa.identityId),
    ];

    return _MockRoom(
      roomId: r,
      name: 'Build Iroh Rooms MVP',
      myRole: Roles.owner,
      // Roster status is membership only (active|invited|left|removed) — QA's
      // offline-ness is peer/fleet liveness, never a member.status value.
      members: [for (final p in MockPeople.everyone) _member(p)],
      timeline: timeline,
      files: files,
      pipes: pipes,
      peers: peers,
    );
  }

  _MockRoom _buildSideRoom(
    String seed,
    String name,
    List<MockPerson> people,
    String myRole,
    String blurb,
  ) {
    final r = 'blake3:${_hex('room-$seed', 64)}';
    final owner = people.first;
    final timeline = <TimelineEvent>[
      _ev(r, _at(8, 30), owner, TimelineKinds.roomCreated),
      for (var i = 1; i < people.length; i++)
        _ev(r, _at(8, 32 + (i - 1)), people[i], TimelineKinds.memberJoined,
            member: MemberRef(identityId: people[i].identityId, role: people[i].role)),
      _ev(r, _at(9, 5), owner, TimelineKinds.message, body: blurb),
    ];
    return _MockRoom(
      roomId: r,
      name: name,
      myRole: myRole,
      members: [for (final p in people) _member(p)],
      timeline: timeline,
      peers: [
        for (final p in people)
          if (p.identityId != MockPeople.alex.identityId)
            PeerStatus(endpointId: p.endpointId, state: PeerStates.connected, path: PeerPaths.direct, identityId: p.identityId),
      ],
    );
  }

  // -- internals ----------------------------------------------------------------

  void _setState(ConnectionState next) {
    if (_state == next) return;
    _state = next;
    _states.add(next);
  }

  void _emit(String name, Map<String, dynamic> data) {
    _pushes.add(Push(name, data));
  }

  Never _err(String code, String message, [String? hint]) =>
      throw RequestError(code, message, hint: hint);

  Identity _needIdentity() {
    final identity = _identity;
    if (identity == null) {
      _err(ErrorCodes.identityMissing, 'no identity on this daemon', 'run identity.create first');
    }
    return identity;
  }

  _MockRoom _needRoom(Object? roomId) {
    if (roomId is! String || roomId.isEmpty) {
      _err(ErrorCodes.invalidParams, 'room_id is required');
    }
    final room = _rooms[roomId];
    if (room == null) {
      final shown = roomId.length > 18 ? roomId.substring(0, 18) : roomId;
      _err(ErrorCodes.roomUnknown, 'no room $shown… on this daemon', 'room.list shows known rooms');
    }
    return room;
  }

  _MockRoom _needOpenRoom(Object? roomId) {
    final room = _needRoom(roomId);
    if (!room.open) {
      _err(ErrorCodes.roomNotOpen, 'room "${room.name}" is not open', 'call room.open first');
    }
    return room;
  }

  MockPerson _me(_MockRoom room) {
    final identity = _needIdentity();
    MockPerson? existing;
    for (final p in MockPeople.everyone) {
      if (p.identityId == identity.identityId) {
        existing = p;
        break;
      }
    }
    String? memberRole;
    for (final m in room.members) {
      if (m.identityId == identity.identityId) {
        memberRole = m.role;
        break;
      }
    }
    final role = memberRole ?? room.myRole;
    return existing != null
        ? MockPerson(
            identityId: existing.identityId,
            deviceId: existing.deviceId,
            endpointId: existing.endpointId,
            role: role,
            name: existing.name)
        : MockPerson(
            identityId: identity.identityId,
            deviceId: identity.deviceId,
            endpointId: _hex('self-endpoint', 64),
            role: role,
            name: 'You');
  }

  /// Append + push. The push is handed to the broadcast stream synchronously,
  /// so listeners receive the echo of an own write BEFORE the method response
  /// resolves (see the library doc, deviation 1). Pushes flow only for open
  /// rooms, exactly once per event.
  void _ingest(_MockRoom room, TimelineEvent event) {
    room.timeline.add(event);
    if (room.open) {
      _emit('room.event', {'room_id': room.roomId, 'event': event.toJson()});
    }
  }

  void _pushPeers(_MockRoom room) {
    if (room.open) {
      _emit('peers.changed', {
        'room_id': room.roomId,
        'peers': [for (final p in room.peers) p.toJson()],
      });
    }
  }

  Map<String, dynamic> _summary(_MockRoom room) {
    final identity = _identity;
    Member? mine;
    if (identity != null) {
      for (final m in room.members) {
        if (m.identityId == identity.identityId) {
          mine = m;
          break;
        }
      }
    }
    // Recency is the newest signed event's ts — a daemon projection
    // (docs/room-attention.md, decision 2). The real daemon does not emit this
    // yet (the identified, deferrable follow-up); the mock derives it so the
    // room-list recency slice of #64 can build against the real shape now.
    final newest = _newestEvent(room.timeline);
    return RoomSummary(
      roomId: room.roomId,
      name: room.name,
      role: mine?.role ?? room.myRole,
      status: mine?.status,
      memberCount: room.members.length,
      open: room.open,
      lastEventTs: newest?.ts,
      lastEventKind: newest?.kind,
    ).toJson();
  }

  /// The room's newest event by ts (the recency source of
  /// docs/room-attention.md, decision 2), or null for an empty timeline.
  static TimelineEvent? _newestEvent(List<TimelineEvent> timeline) {
    TimelineEvent? newest;
    for (final e in timeline) {
      if (newest == null || e.ts > newest.ts) newest = e;
    }
    return newest;
  }

  String get _selfEndpointId => _hex('self-endpoint', 64);

  String _endpointAddr() => '$_selfEndpointId@127.0.0.1:52731';

  /// Chronological view — a STABLE sort by ts (equal-ts fixtures keep their
  /// authored order, like JS `Array.sort` in mock.ts).
  List<TimelineEvent> _sortedTimeline(_MockRoom room) {
    final indexed = room.timeline.asMap().entries.toList()
      ..sort((a, b) {
        final byTs = a.value.ts.compareTo(b.value.ts);
        return byTs != 0 ? byTs : a.key.compareTo(b.key);
      });
    return [for (final e in indexed) e.value];
  }

  /// Live-update demo: only for the MVP room, only once per open.
  void _simulate(_MockRoom room) {
    if (!simulateLiveActivity || room.roomId != _mainRoomId || room.simulated) return;
    room.simulated = true;
    final qa = MockPeople.qaAgent;
    void later(int ms, void Function() fn) {
      room.timers.add(Timer(Duration(milliseconds: ms), () {
        if (room.open) fn();
      }));
    }

    later(6000, () {
      _ingest(
          room,
          _ev(room.roomId, _now(), MockPeople.backendAgent, TimelineKinds.agentStatus,
              label: 'working',
              statusMessage: 'Integration tests passing (17/24). Sync convergence suite up next.',
              progress: 72));
    });
    later(13000, () {
      room.peers = [
        for (final p in room.peers)
          p.endpointId == qa.endpointId
              ? PeerStatus(endpointId: p.endpointId, state: PeerStates.connecting, identityId: p.identityId)
              : p,
      ];
      _pushPeers(room);
    });
    later(19000, () {
      room.peers = [
        for (final p in room.peers)
          p.endpointId == qa.endpointId
              ? PeerStatus(endpointId: p.endpointId, state: PeerStates.connected, path: PeerPaths.relay, identityId: p.identityId)
              : p,
      ];
      _pushPeers(room);
      room.members = [
        for (final m in room.members)
          m.identityId == qa.identityId ? Member(identityId: m.identityId, role: m.role, status: 'active') : m,
      ];
    });
    later(26000, () {
      _ingest(
          room,
          _ev(room.roomId, _now(), MockPeople.backendAgent, TimelineKinds.agentStatus,
              label: 'tests_passed',
              statusMessage: 'All 24 integration tests passing. Ready for review.',
              progress: 100));
    });
  }

  /// `agents.fleet` fixture — computed live from the same evidence the daemon
  /// would use (folded events + peer state of open rooms), never hardcoded, so
  /// the dashboard's numbers stay real even as the mock simulation evolves.
  Map<String, dynamic> _fleet() {
    final now = _now();
    final byAgent = <String, _FleetEntry>{};
    var roomsCovered = 0;

    for (final room in _rooms.values) {
      final agents = [
        for (final m in room.members)
          if (m.role == Roles.agent) m,
      ];
      if (agents.isNotEmpty) roomsCovered += 1;
      for (final m in agents) {
        final entry = byAgent.putIfAbsent(m.identityId, _FleetEntry.new);
        entry.rooms.add(FleetAgentRoom(roomId: room.roomId, name: room.name));

        // Newest agent_status + newest event of any kind by this identity here.
        TimelineEvent? latest;
        int? lastSeen;
        for (final e in room.timeline) {
          if (e.sender.identityId != m.identityId) continue;
          if (lastSeen == null || e.ts > lastSeen) lastSeen = e.ts;
          if (e.kind == TimelineKinds.agentStatus && (latest == null || e.ts > latest.ts)) {
            latest = e;
          }
        }
        if (latest != null && (entry.latest == null || latest.ts > entry.latest!.ts)) {
          entry.latest = FleetAgentLatest(
            label: latest.label ?? '',
            message: latest.statusMessage,
            progress: latest.progress,
            ts: latest.ts,
            roomId: room.roomId,
          );
        }
        if (lastSeen != null && (entry.lastSeenTs == null || lastSeen > entry.lastSeenTs!)) {
          entry.lastSeenTs = lastSeen;
        }

        // Primary signal: is one of this identity's devices a connected peer
        // in a room this "daemon" has open? Non-open rooms are never online.
        String? ep;
        for (final p in MockPeople.everyone) {
          if (p.identityId == m.identityId) {
            ep = p.endpointId;
            break;
          }
        }
        final connected = room.open &&
            ep != null &&
            room.peers.any((p) => p.endpointId == ep && p.state == PeerStates.connected);
        final lv = _deriveLiveness(connected, latest, now);
        // Multi-room aggregate: strongest presence wins.
        if (_livenessRank[lv]! < _livenessRank[entry.liveness]!) entry.liveness = lv;
      }
    }

    final agents = [
      for (final e in byAgent.entries)
        FleetAgent(
          identityId: e.key,
          rooms: e.value.rooms,
          liveness: e.value.liveness,
          latest: e.value.latest,
          lastSeenTs: e.value.lastSeenTs,
        ),
    ];
    agents.sort((a, b) {
      final rank = _livenessRank[a.liveness]! - _livenessRank[b.liveness]!;
      if (rank != 0) return rank;
      final seen = (b.lastSeenTs ?? 0) - (a.lastSeenTs ?? 0);
      if (seen != 0) return seen > 0 ? 1 : -1;
      return a.identityId.compareTo(b.identityId);
    });

    return FleetResult(
      active: agents
          .where((a) => a.liveness == LivenessValues.working || a.liveness == LivenessValues.onlineIdle)
          .length,
      working: agents.where((a) => a.liveness == LivenessValues.working).length,
      total: agents.length,
      roomsTotal: _rooms.length,
      roomsCovered: roomsCovered,
      agents: agents,
    ).toJson();
  }

  dynamic _dispatch(String method, Map<String, dynamic> params) {
    switch (method) {
      case 'daemon.status':
        return {
          'version': '0.1.0-mock',
          // Must match the real daemon's contract (PROTOCOL.md "Protocol
          // version"): a client keys its version handshake on this.
          'protocol': 1,
          'pid': 4242,
          'port': 7420,
          'data_dir': '/mock/Jeliya',
          'mode': DaemonModes.loopback,
          'identity': _identity?.toJson(),
          'endpoint': _identity != null
              ? {'endpoint_id': _selfEndpointId, 'addr': _endpointAddr(), 'relay_url': null}
              : null,
          'rooms_open': [
            for (final r in _rooms.values)
              if (r.open) r.roomId,
          ],
        };

      case 'daemon.shutdown':
        return {'shutting_down': true};

      case 'identity.create':
        if (_identity != null) {
          _err(ErrorCodes.identityExists, 'an identity already exists on this daemon',
              'use daemon.status to read it');
        }
        _identity = Identity(
          identityId: MockPeople.alex.identityId,
          deviceId: MockPeople.alex.deviceId,
        );
        return _identity!.toJson();

      case 'room.create':
        {
          final identity = _needIdentity();
          final name = params['name'];
          if (name is! String || name.trim().isEmpty) {
            _err(ErrorCodes.invalidParams, 'room name must not be empty');
          }
          final roomId = 'blake3:${_hex('room-$name-${_now()}', 64)}';
          final me = MockPerson(
            identityId: identity.identityId,
            deviceId: identity.deviceId,
            endpointId: MockPeople.alex.endpointId,
            role: Roles.owner,
            name: MockPeople.alex.name,
          );
          final room = _MockRoom(
            roomId: roomId,
            name: name.trim(),
            myRole: Roles.owner,
            members: [Member(identityId: me.identityId, role: Roles.owner, status: 'active')],
            timeline: [_ev(roomId, _now(), me, TimelineKinds.roomCreated)],
          );
          _rooms[roomId] = room;
          return {'room_id': roomId};
        }

      case 'room.list':
        if (_identity == null) return {'rooms': <Object?>[]};
        return {
          'rooms': [for (final r in _rooms.values) _summary(r)],
        };

      case 'room.open':
        {
          final room = _needRoom(params['room_id']);
          final identity = _needIdentity();
          Member? mine;
          for (final m in room.members) {
            if (m.identityId == identity.identityId) {
              mine = m;
              break;
            }
          }
          if (mine == null || mine.status != 'active') {
            _err(ErrorCodes.notAMember, 'this identity is not an active member of "${room.name}"',
                'ask the room admin for an invite');
          }
          room.open = true;
          _simulate(room);
          return {
            'endpoint': {'endpoint_id': _selfEndpointId, 'addr': _endpointAddr()},
            'members': [for (final m in room.members) m.toJson()],
            'timeline': [for (final e in _sortedTimeline(room)) e.toJson()],
          };
        }

      case 'room.close':
        {
          final room = _needRoom(params['room_id']);
          room.open = false;
          room.simulated = false;
          for (final t in room.timers) {
            t.cancel();
          }
          room.timers.clear();
          return <String, dynamic>{};
        }

      case 'room.leave':
        {
          final room = _needRoom(params['room_id']);
          final identity = _needIdentity();
          var idx = -1;
          for (var i = 0; i < room.members.length; i++) {
            if (room.members[i].identityId == identity.identityId) {
              idx = i;
              break;
            }
          }
          final mine = idx >= 0 ? room.members[idx] : null;
          if (mine == null || mine.status != 'active') {
            _err(ErrorCodes.notAMember, 'this identity is not an active member of "${room.name}"',
                'ask the room admin for an invite');
          }
          if (mine.role == Roles.owner) {
            _err(ErrorCodes.invalidParams,
                'room owners cannot leave yet; close the local room session instead');
          }
          final event = _ev(room.roomId, _now(), _me(room), TimelineKinds.memberLeft,
              member: MemberRef(identityId: identity.identityId));
          room.members[idx] = Member(identityId: mine.identityId, role: mine.role, status: 'left');
          _ingest(room, event);
          room.open = false;
          room.simulated = false;
          for (final t in room.timers) {
            t.cancel();
          }
          room.timers.clear();
          return {'event_id': event.eventId};
        }

      case 'room.timeline':
        {
          final room = _needRoom(params['room_id']);
          final events = _sortedTimeline(room);
          final limit = params['limit'];
          final tail = (limit is num && limit > 0 && events.length > limit)
              ? events.sublist(events.length - limit.toInt())
              : events;
          return {
            'events': [for (final e in tail) e.toJson()],
          };
        }

      case 'room.members':
        {
          final room = _needRoom(params['room_id']);
          return {
            'members': [for (final m in room.members) m.toJson()],
          };
        }

      case 'invite.create':
        {
          // Unlike room.open, the real daemon mints invites on CLOSED rooms too
          // (it persists member.invited directly), so use _needRoom, not
          // _needOpenRoom.
          final room = _needRoom(params['room_id']);
          final identityId = params['identity_id'];
          if (identityId is! String || !_hex64Re.hasMatch(identityId.trim())) {
            _err(ErrorCodes.invalidParams, "identity_id must be the invitee's hex identity id",
                'ask the invitee to copy their identity id from their onboarding screen or sidebar footer');
          }
          final role = params['role'] is String ? params['role'] as String : Roles.member;
          final me = _me(room);
          _ingest(
              room,
              _ev(room.roomId, _now(), me, TimelineKinds.memberInvited,
                  member: MemberRef(identityId: identityId.trim(), role: role)));
          final ticket =
              'roomtkt1${_base32ish('${room.roomId}:$identityId:${_now()}', 96)}';
          // `expiry` accepts a duration string ("24h"/"3600") or a number of
          // seconds (see docs/PROTOCOL.md); no expiry means single-use, not
          // time-boxed.
          final expirySecs = _parseExpirySeconds(params['expiry']);
          final expiresAt = expirySecs != null ? _now() + expirySecs * 1000 : null;
          _tickets[ticket] = _TicketEntry(
            roomId: room.roomId,
            identityId: identityId.trim(),
            role: role,
            expiresAt: expiresAt,
          );
          return {'ticket': ticket};
        }

      case 'room.join':
        {
          final identity = _needIdentity();
          final ticket = (params['ticket'] is String ? params['ticket'] as String : '').trim();
          if (!ticket.startsWith('roomtkt1') || ticket.length < 24) {
            _err(ErrorCodes.badTicket, 'ticket is not a valid roomtkt1 token',
                'ask the inviter for a fresh ticket');
          }
          final entry = _tickets[ticket];
          if (entry == null) {
            // A well-formed roomtkt1 string nobody minted on this daemon is
            // never something a real person hand-types — they only ever paste
            // one they received — so this is a genuine failure, not a demo
            // shortcut.
            _err(ErrorCodes.badTicket, 'this ticket was never issued on this daemon',
                'ask the inviter for a fresh ticket');
          }
          if (entry.expiresAt != null && _now() > entry.expiresAt!) {
            _tickets.remove(ticket);
            _err(ErrorCodes.ticketExpired, 'this ticket has expired',
                'ask the inviter to generate a fresh one');
          }
          final room = _rooms[entry.roomId];
          if (room == null) {
            _err(ErrorCodes.roomUnknown, 'the invited room no longer exists on this daemon',
                'ask the inviter for a fresh ticket');
          }
          // Single-use: redeeming the same ticket twice should fail like a real
          // spent one, not silently succeed again.
          _tickets.remove(ticket);
          if (!room.members.any((m) => m.identityId == identity.identityId)) {
            room.members.add(Member(identityId: identity.identityId, role: entry.role, status: 'active'));
            final joiner = MockPerson(
              identityId: identity.identityId,
              deviceId: identity.deviceId,
              endpointId: MockPeople.alex.endpointId,
              role: entry.role,
              name: MockPeople.alex.name,
            );
            _ingest(
                room,
                _ev(room.roomId, _now(), joiner, TimelineKinds.memberJoined,
                    member: MemberRef(identityId: identity.identityId, role: entry.role)));
          }
          return {'room_id': room.roomId};
        }

      case 'message.send':
        {
          final room = _needOpenRoom(params['room_id']);
          final body = params['body'];
          if (body is! String || body.trim().isEmpty) {
            _err(ErrorCodes.invalidParams, 'message body must not be empty');
          }
          final event = _ev(room.roomId, _now(), _me(room), TimelineKinds.message, body: body);
          _ingest(room, event);
          return {'event_id': event.eventId};
        }

      case 'status.post':
        {
          final room = _needOpenRoom(params['room_id']);
          final label = params['label'];
          if (label is! String || label.isEmpty) {
            _err(ErrorCodes.invalidParams, 'label is required');
          }
          final artifacts = params['artifacts'];
          final event = _ev(room.roomId, _now(), _me(room), TimelineKinds.agentStatus,
              label: label,
              statusMessage: params['message'] is String ? params['message'] as String : null,
              progress: params['progress'] is num ? params['progress'] as num : null,
              artifacts: artifacts is List ? artifacts.whereType<String>().toList() : const []);
          _ingest(room, event);
          return {'event_id': event.eventId};
        }

      case 'file.share':
        {
          final room = _needOpenRoom(params['room_id']);
          final rawPath = params['path'];
          if (rawPath is! String || rawPath.trim().isEmpty) {
            _err(ErrorCodes.invalidParams, 'path is required');
          }
          final path = rawPath.trim();
          final trimmedName = params['name'] is String ? (params['name'] as String).trim() : '';
          final baseName = path.split('/').last;
          final name = trimmedName.isNotEmpty ? trimmedName : (baseName.isNotEmpty ? baseName : path);
          const mimeByExt = <String, String>{
            'pdf': 'application/pdf',
            'md': 'text/markdown',
            'txt': 'text/plain',
            'json': 'application/json',
            'png': 'image/png',
            'jpg': 'image/jpeg',
          };
          final ext = name.contains('.') ? name.substring(name.lastIndexOf('.') + 1).toLowerCase() : '';
          final mime = (params['mime'] is String ? params['mime'] as String : null) ??
              mimeByExt[ext] ??
              'application/octet-stream';
          final size = 24000 + (path.length * 137) % 900000;
          final fileId = _fid('$path-${_now()}');
          final now = _now();
          room.files.add(_MockFile(
            fileId: fileId,
            name: name,
            size: size,
            mime: mime,
            senderId: _needIdentity().identityId,
            ts: now,
            available: false,
            providers: 1,
          ));
          final event = _ev(room.roomId, now, _me(room), TimelineKinds.fileShared,
              file: FileRef(fileId: fileId, name: name, size: size, mime: mime));
          _ingest(room, event);
          return {'file_id': fileId, 'event_id': event.eventId};
        }

      case 'file.list':
        {
          final room = _needRoom(params['room_id']);
          return {
            'files': [for (final f in room.files) f.toJson()],
          };
        }

      case 'file.fetch':
        {
          final room = _needOpenRoom(params['room_id']);
          _MockFile? file;
          for (final f in room.files) {
            if (f.fileId == params['file_id']) {
              file = f;
              break;
            }
          }
          if (file == null) {
            _err(ErrorCodes.fileUnavailable, 'unknown file_id for this room',
                'file.list shows shareable files');
          }
          final found = file;
          final saveDir = params['save_dir'] is String ? params['save_dir'] as String : null;
          // Simulate the transfer: resolve/reject after a delay.
          return Future<Map<String, dynamic>>.delayed(fetchLatency, () {
            if (!found.available) {
              throw RequestError(
                ErrorCodes.fileUnavailable,
                'no connected peer is currently providing these bytes',
                hint: 'providers seen: ${found.providers} (offline) — retry when the sender is online',
              );
            }
            // Default matches the daemon: <data_dir>/downloads (PROTOCOL.md).
            final dir = saveDir ?? '/mock/Jeliya/downloads';
            final path = '$dir/${found.name}';
            found.fetched = true;
            found.localPath = path;
            found.localBytes = found.size;
            found.fetchedAtMs = _now();
            return {'path': path, 'bytes': found.size, 'verified': true};
          });
        }

      case 'pipe.expose':
        {
          final room = _needOpenRoom(params['room_id']);
          final rawTarget = params['target'];
          if (rawTarget is! String || !RegExp(r'^[\w.-]+:\d+$').hasMatch(rawTarget.trim())) {
            _err(ErrorCodes.invalidParams, 'target must look like 127.0.0.1:3000');
          }
          final peerIdentity = params['peer_identity'];
          if (peerIdentity is! String || peerIdentity.isEmpty) {
            _err(ErrorCodes.invalidParams,
                'peer_identity is required — a pipe has exactly one authorized peer');
          }
          final target = rawTarget.trim();
          final pipeId = _pid('pipe-$target-${_now()}');
          final me = _me(room);
          room.pipes.add(_MockPipe(
            pipeId: pipeId,
            target: target,
            openedBy: me.identityId,
            authorizedPeer: peerIdentity,
          ));
          final event = _ev(room.roomId, _now(), me, TimelineKinds.pipeOpened,
              pipe: PipeRef(pipeId: pipeId, target: target, authorizedPeer: peerIdentity));
          _ingest(room, event);
          return {'pipe_id': pipeId, 'event_id': event.eventId};
        }

      case 'pipe.list':
        {
          final room = _needRoom(params['room_id']);
          return {
            'pipes': [for (final p in room.pipes) p.toJson()],
          };
        }

      case 'pipe.connect':
        {
          final room = _needOpenRoom(params['room_id']);
          _MockPipe? pipe;
          for (final x in room.pipes) {
            if (x.pipeId == params['pipe_id']) {
              pipe = x;
              break;
            }
          }
          if (pipe == null) {
            _err(ErrorCodes.pipeDenied, 'unknown pipe for this room', 'pipe.list shows live pipes');
          }
          if (pipe.state == PipeStates.closed) {
            _err(ErrorCodes.pipeDenied, 'pipe is closed', 'ask the owner to expose it again');
          }
          final myId = _needIdentity().identityId;
          if (pipe.authorizedPeer != myId && pipe.openedBy != myId) {
            _err(ErrorCodes.pipeDenied, 'you are not the authorized peer for this pipe');
          }
          pipe.connected = true;
          _portSeq += 1;
          return {'local_addr': '127.0.0.1:$_portSeq'};
        }

      case 'pipe.close':
        {
          final room = _needOpenRoom(params['room_id']);
          _MockPipe? pipe;
          for (final x in room.pipes) {
            if (x.pipeId == params['pipe_id']) {
              pipe = x;
              break;
            }
          }
          if (pipe == null) {
            _err(ErrorCodes.pipeDenied, 'unknown pipe for this room', 'pipe.list shows live pipes');
          }
          pipe.state = PipeStates.closed;
          pipe.connected = false;
          // A pipe_closed event nulls both target and authorized_peer, matching
          // the daemon's materializer (see PROTOCOL.md TimelineEvent field notes).
          final event = _ev(room.roomId, _now(), _me(room), TimelineKinds.pipeClosed,
              pipe: PipeRef(pipeId: pipe.pipeId));
          _ingest(room, event);
          return {'event_id': event.eventId};
        }

      case 'peers.status':
        {
          final room = _needRoom(params['room_id']);
          return {
            'peers': [for (final p in room.peers) p.toJson()],
          };
        }

      case 'agents.fleet':
        _needIdentity();
        return _fleet();

      case 'agent.history':
        {
          final room = _needRoom(params['room_id']);
          final rawId = params['identity_id'];
          if (rawId is! String || !_hex64Re.hasMatch(rawId.trim())) {
            _err(ErrorCodes.invalidParams, 'identity_id must be a hex identity id');
          }
          final id = rawId.trim();
          final rawLimit = params['limit'];
          final limit = rawLimit is num && rawLimit > 0 ? rawLimit.floor() : 100;
          // One point per real agent_status event, chronological — no
          // interpolation.
          final points = [
            for (final e in _sortedTimeline(room))
              if (e.kind == TimelineKinds.agentStatus && e.sender.identityId == id) e,
          ];
          final tail = points.length > limit ? points.sublist(points.length - limit) : points;
          return {
            'points': [
              for (final e in tail)
                HistoryPoint(ts: e.ts, label: e.label ?? '', progress: e.progress).toJson(),
            ],
          };
        }

      default:
        _err(ErrorCodes.invalidParams, 'unknown method $method');
    }
  }
}
