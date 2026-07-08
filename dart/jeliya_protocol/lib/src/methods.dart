/// Typed RPC surface over the transport-agnostic [Client] — the Dart
/// counterpart of protocol.ts `MethodMap` (one wrapper per PROTOCOL.md method)
/// plus typed push streams. Works identically over the WebSocket sidecar
/// transport, the mock fixture client, and a future FFI transport.
library;

import 'models.dart';
import 'protocol.dart';

Map<String, dynamic> _asMap(dynamic v) =>
    v is Map ? v.cast<String, dynamic>() : const <String, dynamic>{};

List<T> _asList<T>(dynamic v, String key, T Function(Map<String, dynamic>) fromJson) {
  final raw = _asMap(v)[key];
  if (raw is! List) return const [];
  return List.unmodifiable(
    raw.whereType<Map>().map((m) => fromJson(m.cast<String, dynamic>())),
  );
}

/// One typed method per PROTOCOL.md RPC. Every wrapper throws [RequestError]
/// on a failed response (codes in [ErrorCodes]) — cross-cutting codes
/// (`invalid_params`, `internal`, `identity_missing`, `room_unknown`,
/// `room_not_open`, `not_a_member`) can come back from any relevant call.
extension JeliyaMethods on Client {
  // -- daemon & identity ------------------------------------------------------

  /// `daemon.status` — read this once after every (re)connect and hard-fail an
  /// unsupported [DaemonStatus.protocol] (no connect-time negotiation exists).
  Future<DaemonStatus> daemonStatus() async =>
      DaemonStatus.fromJson(_asMap(await call('daemon.status')));

  /// `daemon.shutdown` — the daemon replies `{ shutting_down: true }`, then
  /// runs the graceful teardown and exits.
  Future<void> daemonShutdown() => call('daemon.shutdown');

  /// `identity.create` — errors [ErrorCodes.identityExists] if one exists.
  Future<Identity> identityCreate() async =>
      Identity.fromJson(_asMap(await call('identity.create')));

  // -- rooms ------------------------------------------------------------------

  /// `room.create` — returns the new `room_id`.
  Future<String> roomCreate(String name) async =>
      _asMap(await call('room.create', {'name': name}))['room_id'] as String? ?? '';

  /// `room.list`.
  Future<List<RoomSummary>> roomList() async =>
      _asList(await call('room.list'), 'rooms', RoomSummary.fromJson);

  /// `room.open` — spawns the room's node session and starts pushes. Succeeds
  /// locally regardless of peer reachability (unreachable hints surface as a
  /// stale timeline that later syncs, not as an error). [peers] are optional
  /// `<endpoint_id>@<ip:port>` dial hints merged into the persisted hint set.
  Future<RoomOpenResult> roomOpen(String roomId, {List<String>? peers}) async =>
      RoomOpenResult.fromJson(_asMap(await call('room.open', {
        'room_id': roomId,
        if (peers != null) 'peers': peers,
      })));

  /// `room.close` — closes only this daemon's live session; membership stays
  /// active.
  Future<void> roomClose(String roomId) => call('room.close', {'room_id': roomId});

  /// `room.leave` — authors `member.left` and closes the local session;
  /// returns the `event_id`. Owners are rejected until ownership transfer
  /// exists.
  Future<String> roomLeave(String roomId) async =>
      _asMap(await call('room.leave', {'room_id': roomId}))['event_id'] as String? ?? '';

  /// `room.timeline` — chronological. Pushes are lossy, so re-read this (or
  /// `room.open`) after any reconnect.
  Future<List<TimelineEvent>> roomTimeline(String roomId, {int? limit}) async =>
      _asList(
        await call('room.timeline', {
          'room_id': roomId,
          if (limit != null) 'limit': limit,
        }),
        'events',
        TimelineEvent.fromJson,
      );

  /// `room.members`.
  Future<List<Member>> roomMembers(String roomId) async =>
      _asList(await call('room.members', {'room_id': roomId}), 'members', Member.fromJson);

  /// `invite.create` — returns the ticket. Mints on a closed room too.
  /// [expiry] is a number of seconds or a duration string (`"24h"`, `"90m"`,
  /// `"3600"`); omitted means single-use, not time-boxed. Distinctive errors:
  /// [ErrorCodes.notAMember] (caller is not the room admin),
  /// [ErrorCodes.invalidParams] (self-invite, bad role, non-64-hex invitee).
  Future<String> inviteCreate({
    required String roomId,
    required String identityId,
    required String role,
    Object? expiry,
  }) async =>
      _asMap(await call('invite.create', {
        'room_id': roomId,
        'identity_id': identityId,
        'role': role,
        if (expiry != null) 'expiry': expiry,
      }))['ticket'] as String? ??
      '';

  /// `room.join` — returns the joined `room_id`. Distinctive errors:
  /// [ErrorCodes.badTicket], [ErrorCodes.ticketExpired],
  /// [ErrorCodes.peerUnreachable] (no reachable discovery hint).
  Future<String> roomJoin(String ticket, {String? name, List<String>? peers}) async =>
      _asMap(await call('room.join', {
        'ticket': ticket,
        if (name != null) 'name': name,
        if (peers != null) 'peers': peers,
      }))['room_id'] as String? ??
      '';

  // -- messages & agent status ------------------------------------------------

  /// `message.send` — returns the `event_id`. No idempotency key exists: a
  /// retry after [ErrorCodes.connectionLost] may author a duplicate event.
  Future<String> messageSend(String roomId, String body) async =>
      _asMap(await call('message.send', {'room_id': roomId, 'body': body}))['event_id']
          as String? ??
      '';

  /// `status.post` — returns the `event_id`. [progress] is 0..=100;
  /// [artifacts] is at most 16 valid file ids.
  Future<String> statusPost({
    required String roomId,
    required String label,
    String? message,
    num? progress,
    List<String>? artifacts,
  }) async =>
      _asMap(await call('status.post', {
        'room_id': roomId,
        'label': label,
        if (message != null) 'message': message,
        if (progress != null) 'progress': progress,
        if (artifacts != null) 'artifacts': artifacts,
      }))['event_id'] as String? ??
      '';

  // -- files --------------------------------------------------------------------

  /// `file.share` — [path] MUST resolve inside the daemon data dir (native
  /// clients stage arbitrary user files into `<data_dir>/uploads/` first; see
  /// PROTOCOL.md "Native file sharing"). File size ≤ 100 MiB.
  Future<FileShareResult> fileShare({
    required String roomId,
    required String path,
    String? name,
    String? mime,
  }) async =>
      FileShareResult.fromJson(_asMap(await call('file.share', {
        'room_id': roomId,
        'path': path,
        if (name != null) 'name': name,
        if (mime != null) 'mime': mime,
      })));

  /// `file.list`.
  Future<List<FileEntry>> fileList(String roomId) async =>
      _asList(await call('file.list', {'room_id': roomId}), 'files', FileEntry.fromJson);

  /// `file.fetch` — writes into [saveDir] (default `<data_dir>/downloads`).
  /// Distinctive errors: [ErrorCodes.fileUnavailable],
  /// [ErrorCodes.fileUnauthorized], [ErrorCodes.hashMismatch] (a hard stop:
  /// discard, never render, no retry).
  Future<FileFetchResult> fileFetch({
    required String roomId,
    required String fileId,
    String? saveDir,
  }) async =>
      FileFetchResult.fromJson(_asMap(await call('file.fetch', {
        'room_id': roomId,
        'file_id': fileId,
        if (saveDir != null) 'save_dir': saveDir,
      })));

  // -- pipes ----------------------------------------------------------------------

  /// `pipe.expose` — [target] MUST be a numeric loopback `ip:port` (a hostname
  /// is [ErrorCodes.invalidParams]; a non-loopback ip is
  /// [ErrorCodes.pipeDenied]); [peerIdentity] MUST be 64-hex.
  Future<PipeExposeResult> pipeExpose({
    required String roomId,
    required String target,
    required String peerIdentity,
  }) async =>
      PipeExposeResult.fromJson(_asMap(await call('pipe.expose', {
        'room_id': roomId,
        'target': target,
        'peer_identity': peerIdentity,
      })));

  /// `pipe.list`.
  Future<List<PipeEntry>> pipeList(String roomId) async =>
      _asList(await call('pipe.list', {'room_id': roomId}), 'pipes', PipeEntry.fromJson);

  /// `pipe.connect` — returns the `local_addr` to point a browser at.
  /// Distinctive errors: [ErrorCodes.invalidParams] (owner self-connect or
  /// unknown pipe), [ErrorCodes.peerUnreachable] (owner offline),
  /// [ErrorCodes.pipeDenied] (refused).
  Future<String> pipeConnect({required String roomId, required String pipeId}) async =>
      _asMap(await call('pipe.connect', {'room_id': roomId, 'pipe_id': pipeId}))['local_addr']
          as String? ??
      '';

  /// `pipe.close` — returns the `event_id` of the `pipe_closed` event.
  Future<String> pipeClose({required String roomId, required String pipeId}) async =>
      _asMap(await call('pipe.close', {'room_id': roomId, 'pipe_id': pipeId}))['event_id']
          as String? ??
      '';

  // -- peers ------------------------------------------------------------------------

  /// `peers.status`.
  Future<List<PeerStatus>> peersStatus(String roomId) async =>
      _asList(await call('peers.status', {'room_id': roomId}), 'peers', PeerStatus.fromJson);

  // -- agents (fleet reads) -----------------------------------------------------------

  /// `agents.fleet` — aggregated across all locally known rooms; every count
  /// derives from folded events + live peer state, never estimated.
  Future<FleetResult> agentsFleet() async =>
      FleetResult.fromJson(_asMap(await call('agents.fleet')));

  /// `agent.history` — one point per real `agent_status` event, chronological
  /// ([limit] newest, daemon default 100).
  Future<List<HistoryPoint>> agentHistory({
    required String roomId,
    required String identityId,
    int? limit,
  }) async =>
      _asList(
        await call('agent.history', {
          'room_id': roomId,
          'identity_id': identityId,
          if (limit != null) 'limit': limit,
        }),
        'points',
        HistoryPoint.fromJson,
      );

  // -- typed push streams ---------------------------------------------------------------

  /// Decoded `room.event` pushes. Events with unknown kinds still flow (fold
  /// what you understand, pass over the rest); pushes with other, unknown
  /// names are ignored, per the forward-compat rules.
  Stream<RoomEventPush> get roomEvents =>
      pushes.where((p) => p.name == 'room.event').map((p) => RoomEventPush.fromJson(p.data));

  /// Decoded `peers.changed` pushes — the full replacement peer list for a
  /// room on any peer connection-state change.
  Stream<PeersChangedPush> get peersChanged =>
      pushes.where((p) => p.name == 'peers.changed').map((p) => PeersChangedPush.fromJson(p.data));
}
