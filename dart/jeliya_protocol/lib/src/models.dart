/// Typed view-models for every PROTOCOL.md shape, ported 1:1 from
/// `ui/src/lib/protocol.ts` (the binding reference). Parsing follows the
/// normative forward-compatibility rules:
///
/// 1. `fromJson` ignores unknown keys everywhere.
/// 2. [TimelineEvent] parses unknown `kind` values without throwing and keeps
///    the raw frame accessible ([TimelineEvent.raw]) so unknown events
///    round-trip untouched.
/// 3. Normative and compatibility nullabilities are honored exactly:
///    `RoomSummary.name`/`role`/`status`,
///    `PipeRef.target`/`authorizedPeer` (both null on `pipe_closed`),
///    `MemberRef.role` (omitted on `member_left`), `PeerStatus.identityId`
///    (null pre-admit), `EndpointInfo.addr`/`relayUrl`, absent
///    `artifacts` ⇒ `[]`.
library;

import 'protocol.dart';

// -- tolerant JSON helpers ----------------------------------------------------
//
// Wire frames come from JSON, mocks, or fixtures; helpers coerce gently instead
// of hard-casting so a mistyped optional field degrades rather than throws.

String _string(dynamic v, [String fallback = '']) => v is String ? v : fallback;
String? _stringOrNull(dynamic v) => v is String ? v : null;
int _int(dynamic v, [int fallback = 0]) => v is num ? v.toInt() : fallback;
int? _intOrNull(dynamic v) => v is num ? v.toInt() : null;
num? _numOrNull(dynamic v) => v is num ? v : null;
bool _bool(dynamic v, [bool fallback = false]) => v is bool ? v : fallback;
Map<String, dynamic> _map(dynamic v) =>
    v is Map ? v.cast<String, dynamic>() : const <String, dynamic>{};
List<String> _stringList(dynamic v) =>
    v is List ? List.unmodifiable(v.whereType<String>()) : const [];
List<T> _list<T>(dynamic v, T Function(Map<String, dynamic>) fromJson) => v is List
    ? List.unmodifiable(v.whereType<Map>().map((m) => fromJson(m.cast<String, dynamic>())))
    : const [];

// -- string-enum vocabularies -------------------------------------------------
//
// PROTOCOL.md string unions stay plain [String]s on the models (a higher
// protocol may add values; rejecting them is non-conformant). The known values
// live here as constants.

/// `owner | member | agent` (protocol.ts `Role`).
abstract final class Roles {
  static const String owner = 'owner';
  static const String member = 'member';
  static const String agent = 'agent';
  static const List<String> all = [owner, member, agent];
}

/// `active | invited | left | removed` (`Member.status`, PROTOCOL.md
/// room.open view-model).
abstract final class MemberStatuses {
  static const String active = 'active';
  static const String invited = 'invited';
  static const String left = 'left';
  static const String removed = 'removed';
  static const List<String> all = [active, invited, left, removed];
}

/// The nine v1 `TimelineEvent.kind` values (protocol.ts `TimelineKind`).
abstract final class TimelineKinds {
  static const String roomCreated = 'room_created';
  static const String memberInvited = 'member_invited';
  static const String memberJoined = 'member_joined';
  static const String memberLeft = 'member_left';
  static const String message = 'message';
  static const String agentStatus = 'agent_status';
  static const String fileShared = 'file_shared';
  static const String pipeOpened = 'pipe_opened';
  static const String pipeClosed = 'pipe_closed';
  static const List<String> all = [
    roomCreated,
    memberInvited,
    memberJoined,
    memberLeft,
    message,
    agentStatus,
    fileShared,
    pipeOpened,
    pipeClosed,
  ];
}

/// `connected | connecting | offline` (protocol.ts `PeerState`).
abstract final class PeerStates {
  static const String connected = 'connected';
  static const String connecting = 'connecting';
  static const String offline = 'offline';
  static const List<String> all = [connected, connecting, offline];
}

/// `direct | relay` — [PeerStatus.path] is null when no path is known.
abstract final class PeerPaths {
  static const String direct = 'direct';
  static const String relay = 'relay';
  static const List<String> all = [direct, relay];
}

/// `open | closed` (protocol.ts `PipeState`).
abstract final class PipeStates {
  static const String open = 'open';
  static const String closed = 'closed';
  static const List<String> all = [open, closed];
}

/// `loopback | real` (`daemon.status` `mode`).
abstract final class DaemonModes {
  static const String loopback = 'loopback';
  static const String real = 'real';
  static const List<String> all = [loopback, real];
}

/// Derived agent liveness (protocol.ts `Liveness`) — read-time truth, never
/// stored or extrapolated (honesty rule 4).
abstract final class LivenessValues {
  static const String onlineIdle = 'online-idle';
  static const String working = 'working';
  static const String offline = 'offline';
  static const String stale = 'stale';
  static const List<String> all = [onlineIdle, working, offline, stale];
}

// -- error taxonomy -----------------------------------------------------------

/// The daemon error taxonomy (docs/PROTOCOL.md, Envelope) plus the two
/// client-synthesized codes every client MUST mint identically.
abstract final class ErrorCodes {
  static const String invalidParams = 'invalid_params';
  static const String identityMissing = 'identity_missing';
  static const String identityExists = 'identity_exists';
  static const String notAMember = 'not_a_member';
  static const String roomUnknown = 'room_unknown';
  static const String roomNotOpen = 'room_not_open';
  static const String badTicket = 'bad_ticket';
  static const String ticketExpired = 'ticket_expired';
  static const String fileUnavailable = 'file_unavailable';
  static const String fileUnauthorized = 'file_unauthorized';
  static const String hashMismatch = 'hash_mismatch';
  static const String pipeDenied = 'pipe_denied';
  static const String peerUnreachable = 'peer_unreachable';

  /// Sent by the daemon for unexpected failures — and also the client-side
  /// fallback code (the overlap is intended, see PROTOCOL.md).
  static const String internal = 'internal';

  /// Client-synthesized only: the transport failed before a response arrived.
  /// The request may or may not have executed (at-least-once). Reserved.
  static const String connectionLost = 'connection_lost';

  /// Client-synthesized only (share staging, before any wire call): the
  /// picked file exceeds [maxSharedFileBytes] / could not be read. Distinct
  /// codes so the UI can key specific translatable copy instead of parsing
  /// English message text.
  static const String fileTooLarge = 'file_too_large';
  static const String fileUnreadable = 'file_unreadable';

  /// The 14 codes the daemon can put on the wire.
  static const List<String> wire = [
    invalidParams,
    identityMissing,
    identityExists,
    notAMember,
    roomUnknown,
    roomNotOpen,
    badTicket,
    ticketExpired,
    fileUnavailable,
    fileUnauthorized,
    hashMismatch,
    pipeDenied,
    peerUnreachable,
    internal,
  ];

  /// Codes minted client-side, never sent by the daemon.
  static const List<String> clientSynthesized = [
    connectionLost,
    internal,
    fileTooLarge,
    fileUnreadable,
  ];
}

/// Coerce any thrown object into a [RequestError] — the Dart port of
/// protocol.ts `errorShape`. A [RequestError] passes through unchanged;
/// anything else becomes code [ErrorCodes.internal].
RequestError errorShape(Object? e) {
  if (e is RequestError) return e;
  return RequestError(ErrorCodes.internal, e == null ? 'unknown error' : e.toString());
}

// -- identity & daemon --------------------------------------------------------

/// `{ identity_id, device_id }`.
class Identity {
  const Identity({required this.identityId, required this.deviceId});

  factory Identity.fromJson(Map<String, dynamic> json) => Identity(
        identityId: _string(json['identity_id']),
        deviceId: _string(json['device_id']),
      );

  final String identityId;
  final String deviceId;

  Map<String, dynamic> toJson() => {'identity_id': identityId, 'device_id': deviceId};
}

/// `{ endpoint_id, addr, relay_url }`. `addr` is a dialable
/// `<endpoint_id>@<ip:port>` string when known, else null. Also used for the
/// `room.open` result endpoint (which carries no `relay_url`).
class EndpointInfo {
  const EndpointInfo({required this.endpointId, this.addr, this.relayUrl});

  factory EndpointInfo.fromJson(Map<String, dynamic> json) => EndpointInfo(
        endpointId: _string(json['endpoint_id']),
        addr: _stringOrNull(json['addr']),
        relayUrl: _stringOrNull(json['relay_url']),
      );

  final String endpointId;
  final String? addr;
  final String? relayUrl;

  Map<String, dynamic> toJson() =>
      {'endpoint_id': endpointId, 'addr': addr, 'relay_url': relayUrl};
}

/// `daemon.status` result — the version handshake a client MUST read after
/// every (re)connect (PROTOCOL.md, Protocol version).
class DaemonStatus {
  const DaemonStatus({
    required this.version,
    required this.protocol,
    required this.pid,
    required this.port,
    required this.dataDir,
    required this.mode,
    this.identity,
    this.endpoint,
    this.roomsOpen = const [],
  });

  factory DaemonStatus.fromJson(Map<String, dynamic> json) => DaemonStatus(
        version: _string(json['version']),
        protocol: _int(json['protocol']),
        pid: _int(json['pid']),
        port: _int(json['port']),
        dataDir: _string(json['data_dir']),
        mode: _string(json['mode']),
        identity: json['identity'] is Map ? Identity.fromJson(_map(json['identity'])) : null,
        endpoint: json['endpoint'] is Map ? EndpointInfo.fromJson(_map(json['endpoint'])) : null,
        roomsOpen: _stringList(json['rooms_open']),
      );

  final String version;

  /// Major protocol version spoken on `/ws`; hard-fail an unsupported value.
  final int protocol;
  final int pid;
  final int port;
  final String dataDir;

  /// [DaemonModes.loopback] or [DaemonModes.real].
  final String mode;

  /// Null until `identity.create` has run on this daemon (onboarding gate).
  final Identity? identity;

  /// Null when the daemon has no identity/endpoint yet.
  final EndpointInfo? endpoint;
  final List<String> roomsOpen;

  Map<String, dynamic> toJson() => {
        'version': version,
        'protocol': protocol,
        'pid': pid,
        'port': port,
        'data_dir': dataDir,
        'mode': mode,
        'identity': identity?.toJson(),
        'endpoint': endpoint?.toJson(),
        'rooms_open': roomsOpen,
      };
}

// -- rooms & members ----------------------------------------------------------

/// One `room.list` row.
class RoomSummary {
  const RoomSummary({
    required this.roomId,
    this.name,
    this.role,
    this.status,
    required this.memberCount,
    required this.open,
  });

  factory RoomSummary.fromJson(Map<String, dynamic> json) => RoomSummary(
        roomId: _string(json['room_id']),
        name: _stringOrNull(json['name']),
        role: _stringOrNull(json['role']),
        status: _stringOrNull(json['status']),
        memberCount: _int(json['member_count']),
        open: _bool(json['open']),
      );

  final String roomId;

  /// Null for a joined room whose genesis (name-bearing) event has not synced.
  final String? name;

  /// Compatibility-nullable for older protocol-v1 daemons. The v0.5 daemon
  /// requires an identity and emits a role for every authorized room row.
  final String? role;

  /// This identity's `active|left|removed` status. Nullable only for
  /// compatibility with older protocol-v1 implementations.
  final String? status;
  final int memberCount;
  final bool open;

  Map<String, dynamic> toJson() => {
        'room_id': roomId,
        'name': name,
        'role': role,
        'status': status,
        'member_count': memberCount,
        'open': open,
      };
}

/// One `room.members` row.
class Member {
  const Member({required this.identityId, required this.role, required this.status});

  factory Member.fromJson(Map<String, dynamic> json) => Member(
        identityId: _string(json['identity_id']),
        role: _string(json['role']),
        status: _string(json['status']),
      );

  final String identityId;
  final String role;
  final String status;

  Map<String, dynamic> toJson() =>
      {'identity_id': identityId, 'role': role, 'status': status};
}

// -- timeline -----------------------------------------------------------------

/// The event author `{ identity_id, device_id, role }`.
class Sender {
  const Sender({required this.identityId, required this.deviceId, required this.role});

  factory Sender.fromJson(Map<String, dynamic> json) => Sender(
        identityId: _string(json['identity_id']),
        deviceId: _string(json['device_id']),
        role: _string(json['role']),
      );

  final String identityId;
  final String deviceId;
  final String role;

  Map<String, dynamic> toJson() =>
      {'identity_id': identityId, 'device_id': deviceId, 'role': role};
}

/// `file` payload of a `file_shared` event.
class FileRef {
  const FileRef({required this.fileId, required this.name, required this.size, required this.mime});

  factory FileRef.fromJson(Map<String, dynamic> json) => FileRef(
        fileId: _string(json['file_id']),
        name: _string(json['name']),
        size: _int(json['size']),
        mime: _string(json['mime']),
      );

  final String fileId;
  final String name;
  final int size;
  final String mime;

  Map<String, dynamic> toJson() =>
      {'file_id': fileId, 'name': name, 'size': size, 'mime': mime};
}

/// `pipe` payload of a `pipe_opened` / `pipe_closed` event.
class PipeRef {
  const PipeRef({required this.pipeId, this.target, this.authorizedPeer});

  factory PipeRef.fromJson(Map<String, dynamic> json) => PipeRef(
        pipeId: _string(json['pipe_id']),
        target: _stringOrNull(json['target']),
        authorizedPeer: _stringOrNull(json['authorized_peer']),
      );

  final String pipeId;

  /// Null on a `pipe_closed` event.
  final String? target;

  /// Null when no peer is authorized; a comma-joined list for multi-identity
  /// authorization; null on a `pipe_closed` event.
  final String? authorizedPeer;

  Map<String, dynamic> toJson() =>
      {'pipe_id': pipeId, 'target': target, 'authorized_peer': authorizedPeer};
}

/// `member` payload of `member_invited` / `member_joined` / `member_left`.
class MemberRef {
  const MemberRef({required this.identityId, this.role});

  factory MemberRef.fromJson(Map<String, dynamic> json) => MemberRef(
        identityId: _string(json['identity_id']),
        role: _stringOrNull(json['role']),
      );

  final String identityId;

  /// Omitted (null) on `member_left`.
  final String? role;

  Map<String, dynamic> toJson() =>
      {'identity_id': identityId, if (role != null) 'role': role};
}

/// One validated room event, folded for display. Kind-specific fields are
/// non-null only for that kind. An unrecognized [kind] still parses (forward
/// compat rule 2) — render it inert or skip it; [raw] preserves the full frame
/// so unknown events round-trip through [toJson] untouched.
class TimelineEvent {
  TimelineEvent({
    required this.eventId,
    required this.roomId,
    required this.ts,
    required this.sender,
    required this.kind,
    this.body,
    this.label,
    this.statusMessage,
    this.progress,
    this.artifacts = const [],
    this.file,
    this.pipe,
    this.member,
    Map<String, dynamic>? raw,
  }) : raw = raw == null ? const {} : Map.unmodifiable(raw);

  factory TimelineEvent.fromJson(Map<String, dynamic> json) => TimelineEvent(
        eventId: _string(json['event_id']),
        roomId: _string(json['room_id']),
        ts: _int(json['ts']),
        sender: Sender.fromJson(_map(json['sender'])),
        kind: _string(json['kind']),
        body: _stringOrNull(json['body']),
        label: _stringOrNull(json['label']),
        statusMessage: _stringOrNull(json['status_message']),
        progress: _numOrNull(json['progress']),
        // Absent means [] (PROTOCOL.md TimelineEvent field notes).
        artifacts: _stringList(json['artifacts']),
        file: json['file'] is Map ? FileRef.fromJson(_map(json['file'])) : null,
        pipe: json['pipe'] is Map ? PipeRef.fromJson(_map(json['pipe'])) : null,
        member: json['member'] is Map ? MemberRef.fromJson(_map(json['member'])) : null,
        raw: json,
      );

  final String eventId;
  final String roomId;

  /// The event's signed timestamp in ms — NOT arrival time. It can be older
  /// than events already delivered (late backlog; see PROTOCOL.md Pushes).
  final int ts;
  final Sender sender;

  /// One of [TimelineKinds.all], or an unknown future kind (never rejected).
  final String kind;

  /// kind: message
  final String? body;

  /// kind: agent_status
  final String? label;
  final String? statusMessage;
  final num? progress;

  /// kind: agent_status — absent on the wire means empty, never null.
  final List<String> artifacts;

  /// kind: file_shared
  final FileRef? file;

  /// kind: pipe_opened / pipe_closed
  final PipeRef? pipe;

  /// kind: member_invited / member_joined / member_left
  final MemberRef? member;

  /// The unmodified wire frame this event was parsed from (empty for
  /// hand-constructed events). Keeps unknown keys and unknown kinds intact.
  final Map<String, dynamic> raw;

  /// Whether [kind] is one of the nine v1 kinds this client understands.
  bool get isKnownKind => TimelineKinds.all.contains(kind);

  /// The wire shape. Returns [raw] verbatim when this event was parsed from a
  /// frame (lossless round-trip, unknown keys included); otherwise builds the
  /// frame from the typed fields, emitting kind-specific keys only when set
  /// and `artifacts` only when non-empty (matching the daemon).
  Map<String, dynamic> toJson() {
    if (raw.isNotEmpty) return Map.of(raw);
    return {
      'event_id': eventId,
      'room_id': roomId,
      'ts': ts,
      'sender': sender.toJson(),
      'kind': kind,
      if (body != null) 'body': body,
      if (label != null) 'label': label,
      if (statusMessage != null) 'status_message': statusMessage,
      if (progress != null) 'progress': progress,
      if (artifacts.isNotEmpty) 'artifacts': artifacts,
      if (file != null) 'file': file!.toJson(),
      if (pipe != null) 'pipe': pipe!.toJson(),
      if (member != null) 'member': member!.toJson(),
    };
  }
}

// -- peers ---------------------------------------------------------------------

/// One `peers.status` / `peers.changed` row.
class PeerStatus {
  const PeerStatus({
    required this.endpointId,
    required this.state,
    this.path,
    this.identityId,
  });

  factory PeerStatus.fromJson(Map<String, dynamic> json) => PeerStatus(
        endpointId: _string(json['endpoint_id']),
        state: _string(json['state']),
        path: _stringOrNull(json['path']),
        identityId: _stringOrNull(json['identity_id']),
      );

  final String endpointId;

  /// One of [PeerStates.all].
  final String state;

  /// [PeerPaths.direct], [PeerPaths.relay], or null when unknown.
  final String? path;

  /// The peer's membership identity once the SDK has bound the device (on
  /// admit); null before/during admission — not just for strangers.
  final String? identityId;

  Map<String, dynamic> toJson() =>
      {'endpoint_id': endpointId, 'state': state, 'path': path, 'identity_id': identityId};
}

// -- files ---------------------------------------------------------------------

/// One `file.list` row. `available` means "a currently-connected *other*
/// provider can serve this now" — it never counts the local copy and is always
/// false while the room is closed. `providers` is the historical provider
/// count, not the online count.
class FileEntry {
  const FileEntry({
    required this.fileId,
    required this.name,
    required this.size,
    required this.mime,
    required this.senderId,
    required this.ts,
    required this.available,
    required this.providers,
    this.fetched = false,
    this.localPath,
    this.localBytes,
    this.fetchedAtMs,
  });

  factory FileEntry.fromJson(Map<String, dynamic> json) => FileEntry(
        fileId: _string(json['file_id']),
        name: _string(json['name']),
        size: _int(json['size']),
        mime: _string(json['mime']),
        senderId: _string(json['sender_id']),
        ts: _int(json['ts']),
        available: _bool(json['available']),
        providers: _int(json['providers']),
        fetched: _bool(json['fetched']),
        localPath: _stringOrNull(json['local_path']),
        localBytes: _intOrNull(json['local_bytes']),
        fetchedAtMs: _intOrNull(json['fetched_at_ms']),
      );

  final String fileId;
  final String name;
  final int size;
  final String mime;
  final String senderId;
  final int ts;
  final bool available;
  final int providers;

  /// Persisted "the daemon reports a prior local copy" marker — weaker than a
  /// this-session `verified` fetch (see PROTOCOL.md client conventions).
  final bool fetched;
  final String? localPath;
  final int? localBytes;
  final int? fetchedAtMs;

  Map<String, dynamic> toJson() => {
        'file_id': fileId,
        'name': name,
        'size': size,
        'mime': mime,
        'sender_id': senderId,
        'ts': ts,
        'available': available,
        'providers': providers,
        if (fetched) 'fetched': fetched,
        if (localPath != null) 'local_path': localPath,
        if (localBytes != null) 'local_bytes': localBytes,
        if (fetchedAtMs != null) 'fetched_at_ms': fetchedAtMs,
      };
}

// -- pipes ---------------------------------------------------------------------

/// One `pipe.list` row.
class PipeEntry {
  const PipeEntry({
    required this.pipeId,
    required this.target,
    required this.openedBy,
    this.authorizedPeer,
    required this.state,
    required this.connected,
  });

  factory PipeEntry.fromJson(Map<String, dynamic> json) => PipeEntry(
        pipeId: _string(json['pipe_id']),
        target: _string(json['target']),
        openedBy: _string(json['opened_by']),
        authorizedPeer: _stringOrNull(json['authorized_peer']),
        state: _string(json['state']),
        connected: _bool(json['connected']),
      );

  final String pipeId;
  final String target;
  final String openedBy;

  /// Null when no peer is authorized; a comma-joined list for multi-identity.
  final String? authorizedPeer;

  /// One of [PipeStates.all].
  final String state;
  final bool connected;

  Map<String, dynamic> toJson() => {
        'pipe_id': pipeId,
        'target': target,
        'opened_by': openedBy,
        'authorized_peer': authorizedPeer,
        'state': state,
        'connected': connected,
      };
}

// -- fleet reads ----------------------------------------------------------------

/// One room an agent belongs to (`FleetAgent.rooms` row).
class FleetAgentRoom {
  const FleetAgentRoom({required this.roomId, this.name});

  factory FleetAgentRoom.fromJson(Map<String, dynamic> json) => FleetAgentRoom(
        roomId: _string(json['room_id']),
        name: _stringOrNull(json['name']),
      );

  final String roomId;
  final String? name;

  Map<String, dynamic> toJson() => {'room_id': roomId, 'name': name};
}

/// The newest `agent_status` by an identity across its rooms.
class FleetAgentLatest {
  const FleetAgentLatest({
    required this.label,
    this.message,
    this.progress,
    required this.ts,
    required this.roomId,
  });

  factory FleetAgentLatest.fromJson(Map<String, dynamic> json) => FleetAgentLatest(
        label: _string(json['label']),
        message: _stringOrNull(json['message']),
        progress: _numOrNull(json['progress']),
        ts: _int(json['ts']),
        roomId: _string(json['room_id']),
      );

  final String label;
  final String? message;
  final num? progress;
  final int ts;
  final String roomId;

  Map<String, dynamic> toJson() =>
      {'label': label, 'message': message, 'progress': progress, 'ts': ts, 'room_id': roomId};
}

/// One `agents.fleet` agent row. `liveness` is derived at read time from real
/// peer state + real events — never fabricated (honesty rule 4).
class FleetAgent {
  const FleetAgent({
    required this.identityId,
    this.rooms = const [],
    required this.liveness,
    this.latest,
    this.lastSeenTs,
  });

  factory FleetAgent.fromJson(Map<String, dynamic> json) => FleetAgent(
        identityId: _string(json['identity_id']),
        rooms: _list(json['rooms'], FleetAgentRoom.fromJson),
        liveness: _string(json['liveness']),
        latest: json['latest'] is Map ? FleetAgentLatest.fromJson(_map(json['latest'])) : null,
        lastSeenTs: _intOrNull(json['last_seen_ts']),
      );

  final String identityId;
  final List<FleetAgentRoom> rooms;

  /// One of [LivenessValues.all].
  final String liveness;

  /// Null if this identity has never posted an `agent_status`.
  final FleetAgentLatest? latest;

  /// ts of the newest event of any kind by this identity — an event
  /// timestamp, never "now".
  final int? lastSeenTs;

  Map<String, dynamic> toJson() => {
        'identity_id': identityId,
        'rooms': rooms.map((r) => r.toJson()).toList(),
        'liveness': liveness,
        'latest': latest?.toJson(),
        'last_seen_ts': lastSeenTs,
      };
}

/// `agents.fleet` result.
class FleetResult {
  const FleetResult({
    required this.active,
    required this.working,
    required this.total,
    required this.roomsTotal,
    required this.roomsCovered,
    this.agents = const [],
  });

  factory FleetResult.fromJson(Map<String, dynamic> json) => FleetResult(
        active: _int(json['active']),
        working: _int(json['working']),
        total: _int(json['total']),
        roomsTotal: _int(json['rooms_total']),
        roomsCovered: _int(json['rooms_covered']),
        agents: _list(json['agents'], FleetAgent.fromJson),
      );

  final int active;
  final int working;
  final int total;
  final int roomsTotal;
  final int roomsCovered;
  final List<FleetAgent> agents;

  Map<String, dynamic> toJson() => {
        'active': active,
        'working': working,
        'total': total,
        'rooms_total': roomsTotal,
        'rooms_covered': roomsCovered,
        'agents': agents.map((a) => a.toJson()).toList(),
      };
}

/// One `agent.history` point — one per real `agent_status` event, no
/// interpolation.
class HistoryPoint {
  const HistoryPoint({required this.ts, required this.label, this.progress});

  factory HistoryPoint.fromJson(Map<String, dynamic> json) => HistoryPoint(
        ts: _int(json['ts']),
        label: _string(json['label']),
        progress: _numOrNull(json['progress']),
      );

  final int ts;
  final String label;
  final num? progress;

  Map<String, dynamic> toJson() => {'ts': ts, 'label': label, 'progress': progress};
}

// -- method result shapes -------------------------------------------------------

/// `room.open` result: the room's live endpoint, roster, and the full
/// chronological timeline baseline that live pushes splice into.
class RoomOpenResult {
  const RoomOpenResult({
    required this.endpoint,
    this.members = const [],
    this.timeline = const [],
  });

  factory RoomOpenResult.fromJson(Map<String, dynamic> json) => RoomOpenResult(
        endpoint: EndpointInfo.fromJson(_map(json['endpoint'])),
        members: _list(json['members'], Member.fromJson),
        timeline: _list(json['timeline'], TimelineEvent.fromJson),
      );

  /// `relay_url` is not part of this result; [EndpointInfo.relayUrl] is null.
  final EndpointInfo endpoint;
  final List<Member> members;
  final List<TimelineEvent> timeline;
}

/// `file.share` result.
class FileShareResult {
  const FileShareResult({required this.fileId, required this.eventId});

  factory FileShareResult.fromJson(Map<String, dynamic> json) => FileShareResult(
        fileId: _string(json['file_id']),
        eventId: _string(json['event_id']),
      );

  final String fileId;
  final String eventId;
}

/// `file.fetch` result — `verified` is always true on success (a hash failure
/// is the `hash_mismatch` error, never a silent partial).
class FileFetchResult {
  const FileFetchResult({required this.path, required this.bytes, required this.verified});

  factory FileFetchResult.fromJson(Map<String, dynamic> json) => FileFetchResult(
        path: _string(json['path']),
        bytes: _int(json['bytes']),
        verified: _bool(json['verified']),
      );

  final String path;
  final int bytes;
  final bool verified;
}

/// `pipe.expose` result.
class PipeExposeResult {
  const PipeExposeResult({required this.pipeId, required this.eventId});

  factory PipeExposeResult.fromJson(Map<String, dynamic> json) => PipeExposeResult(
        pipeId: _string(json['pipe_id']),
        eventId: _string(json['event_id']),
      );

  final String pipeId;
  final String eventId;
}

// -- typed pushes -----------------------------------------------------------------

/// `room.event` push data: one new validated event (own or remote), at most
/// once — pushes are lossy, so re-sync after any reconnect.
class RoomEventPush {
  const RoomEventPush({required this.roomId, required this.event});

  factory RoomEventPush.fromJson(Map<String, dynamic> json) => RoomEventPush(
        roomId: _string(json['room_id']),
        event: TimelineEvent.fromJson(_map(json['event'])),
      );

  final String roomId;
  final TimelineEvent event;
}

/// `peers.changed` push data: the full replacement peer list for one room.
class PeersChangedPush {
  const PeersChangedPush({required this.roomId, this.peers = const []});

  factory PeersChangedPush.fromJson(Map<String, dynamic> json) => PeersChangedPush(
        roomId: _string(json['room_id']),
        peers: _list(json['peers'], PeerStatus.fromJson),
      );

  final String roomId;
  final List<PeerStatus> peers;
}
