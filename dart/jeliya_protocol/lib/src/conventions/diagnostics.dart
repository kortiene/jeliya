/// Support-diagnostics report builder, ported 1:1 from
/// ui/src/lib/diagnostics.ts so both clients emit the exact same redacted
/// Markdown (support tooling parses either). The protocol-side fields come
/// from typed models; the app/OS-side fields ([DiagnosticsInput.generatedAt],
/// [DiagnosticsInput.uiVersion], [DiagnosticsInput.browser],
/// [DiagnosticsInput.platform], [DiagnosticsInput.transport]) are parameters
/// the app layer supplies.
library;

import '../models.dart';
import '../protocol.dart';
import 'fetch_state.dart';
import 'format.dart';

/// The last shaped UI error, remembered for the report (diagnostics.ts
/// `DiagnosticEvent`).
class DiagnosticEvent {
  const DiagnosticEvent({
    required this.context,
    required this.code,
    required this.message,
    this.hint,
    required this.at,
  });

  /// Where the error surfaced, e.g. `'room.open'`.
  final String context;
  final String code;
  final String message;
  final String? hint;

  /// ISO-8601 timestamp string.
  final String at;
}

/// Everything [buildDiagnostics] reads (diagnostics.ts `DiagnosticsInput`).
/// Field names mirror the reference — [browser] carries whatever runtime
/// descriptor the app has (the web UI passes `navigator.userAgent`; a desktop
/// app passes its runtime description).
class DiagnosticsInput {
  const DiagnosticsInput({
    required this.generatedAt,
    required this.uiVersion,
    required this.browser,
    required this.platform,
    required this.transport,
    required this.connection,
    this.status,
    this.rooms = const [],
    this.currentRoomId,
    this.members = const [],
    this.files = const [],
    this.fetches = const {},
    this.pipes = const [],
    this.peers = const [],
    this.lastError,
  });

  /// ISO-8601 timestamp string.
  final String generatedAt;
  final String uiVersion;
  final String browser;
  final String platform;

  /// The client's transport description (`Client.describe()`).
  final String transport;
  final ConnectionState connection;
  final DaemonStatus? status;
  final List<RoomSummary> rooms;
  final String? currentRoomId;
  final List<Member> members;
  final List<FileEntry> files;

  /// Per-`file_id` client-local fetch states.
  final Map<String, FetchState> fetches;
  final List<PipeEntry> pipes;
  final List<PeerStatus> peers;
  final DiagnosticEvent? lastError;
}

Map<String, int> _countBy<T>(Iterable<T> items, String? Function(T item) keyFor) {
  final counts = <String, int>{};
  for (final item in items) {
    final key = keyFor(item) ?? 'unknown';
    counts[key] = (counts[key] ?? 0) + 1;
  }
  return counts;
}

String _countRecordValues(Map<String, int> record) {
  final entries = record.entries.toList()..sort((a, b) => a.key.compareTo(b.key));
  if (entries.isEmpty) return 'none';
  return entries.map((e) => '${e.key}=${e.value}').join(', ');
}

final RegExp _ticketRe = RegExp('roomtkt1[0-9a-z]+', caseSensitive: false);
final RegExp _endpointAddrRe = RegExp(r'''[a-z0-9]{20,}@[^\s'")]+''', caseSensitive: false);
final RegExp _hashRe = RegExp('[A-Fa-f0-9]{64}');
final RegExp _unixPathRe = RegExp(r'''/(?:Users|home|tmp|var|private|Volumes)/[^\s'")]+''');
final RegExp _windowsPathRe = RegExp(r'''[A-Za-z]:\\[^\s'")]+''');

String _redact(String value) => value
    .replaceAll(_ticketRe, '<ticket>')
    .replaceAll(_endpointAddrRe, '<endpoint-address>')
    .replaceAll(_hashRe, '<hash>')
    .replaceAll(_unixPathRe, '<local-path>')
    .replaceAll(_windowsPathRe, '<local-path>');

String _shortOrNone(String? id) => id == null || id.isEmpty ? 'none' : shortId(id);

RoomSummary? _currentRoom(DiagnosticsInput input) {
  final id = input.currentRoomId;
  if (id == null || id.isEmpty) return null;
  for (final room in input.rooms) {
    if (room.roomId == id) return room;
  }
  return null;
}

/// Build the redacted support report (diagnostics.ts `buildDiagnostics`). The
/// output excludes message bodies, invite tickets, full local paths, file
/// names, pipe targets, and full identity IDs; free-text fields pass through
/// the same redaction patterns as the reference.
String buildDiagnostics(DiagnosticsInput input) {
  final current = _currentRoom(input);
  final fetchPhases = _countBy(input.fetches.values, (fetch) => fetch.phase);
  final fetchErrors = input.fetches.values
      .where((fetch) => fetch.phase == FetchPhases.error)
      .map((fetch) => fetch.error?.code ?? 'unknown')
      .toList();
  final filesFetched = input.files.where((file) {
    final phase = input.fetches[file.fileId]?.phase ?? '';
    return file.fetched || phase == FetchPhases.fetched || phase == FetchPhases.verified;
  }).length;
  final filesAvailable = input.files.where((file) => file.available).length;
  final providerTotal = input.files.fold<int>(0, (sum, file) => sum + file.providers);
  final pipesOpen = input.pipes.where((pipe) => pipe.state == PipeStates.open).length;
  final pipesConnected = input.pipes.where((pipe) => pipe.connected).length;
  final endpoint = input.status?.endpoint;

  final lines = <String>[
    '# Jeliya Diagnostics',
    '',
    'Generated by the Jeliya UI for support. This excludes message bodies, invite tickets, full local paths, file names, pipe targets, and full identity IDs.',
    '',
    '## Runtime',
    '- generated_at: ${input.generatedAt}',
    '- ui_version: ${input.uiVersion}',
    '- daemon_version: ${input.status?.version ?? 'unknown'}',
    '- daemon_mode: ${input.status?.mode ?? 'unknown'}',
    '- connection: ${input.connection.name}',
    '- transport: ${_redact(input.transport)}',
    '- browser: ${_redact(input.browser)}',
    '- platform: ${_redact(input.platform)}',
    '',
    '## Local Node',
    '- identity_present: ${input.status?.identity != null ? 'yes' : 'no'}',
    '- identity_id: ${_shortOrNone(input.status?.identity?.identityId)}',
    '- device_id: ${_shortOrNone(input.status?.identity?.deviceId)}',
    '- endpoint_present: ${endpoint != null ? 'yes' : 'no'}',
    '- endpoint_id: ${_shortOrNone(endpoint?.endpointId)}',
    '- endpoint_address_present: ${(endpoint?.addr ?? '').isNotEmpty ? 'yes' : 'no'}',
    '- relay_url_present: ${(endpoint?.relayUrl ?? '').isNotEmpty ? 'yes' : 'no'}',
    '- rooms_open_count: ${input.status?.roomsOpen.length ?? 0}',
    '',
    '## Rooms',
    '- total: ${input.rooms.length}',
    '- open: ${input.rooms.where((room) => room.open).length}',
    '- status_counts: '
        '${_countRecordValues(_countBy(input.rooms, (room) => room.status ?? 'active'))}',
    '- current_room_id: ${_shortOrNone(input.currentRoomId)}',
    '- current_room_status: ${current?.status ?? 'none'}',
    '- current_room_open: ${(current?.open ?? false) ? 'yes' : 'no'}',
    '',
    '## Current Room',
    '- members: ${input.members.length}',
    '- member_status_counts: '
        '${_countRecordValues(_countBy(input.members, (member) => member.status))}',
    '- member_role_counts: '
        '${_countRecordValues(_countBy(input.members, (member) => member.role))}',
    '- peers: ${input.peers.length}',
    '- peer_state_counts: ${_countRecordValues(_countBy(input.peers, (peer) => peer.state))}',
    '- peer_path_counts: '
        '${_countRecordValues(_countBy(input.peers, (peer) => peer.path ?? 'unknown'))}',
    '- peer_identity_bound_count: '
        '${input.peers.where((peer) => (peer.identityId ?? '').isNotEmpty).length}',
    '- files_total: ${input.files.length}',
    '- files_available: $filesAvailable',
    '- files_fetched: $filesFetched',
    '- files_fetch_errors: ${fetchErrors.length}',
    '- files_provider_total: $providerTotal',
    '- fetch_phase_counts: ${_countRecordValues(fetchPhases)}',
    '- pipes_total: ${input.pipes.length}',
    '- pipes_open: $pipesOpen',
    '- pipes_connected: $pipesConnected',
  ];

  if (fetchErrors.isNotEmpty) {
    lines.add(
        '- fetch_error_codes: ${_countRecordValues(_countBy(fetchErrors, (code) => code))}');
  }

  lines.addAll(['', '## Last UI Error']);
  final lastError = input.lastError;
  if (lastError != null) {
    final hint = lastError.hint;
    lines.addAll([
      '- at: ${lastError.at}',
      '- context: ${_redact(lastError.context)}',
      '- code: ${_redact(lastError.code)}',
      '- message: ${_redact(lastError.message)}',
      '- hint: ${hint == null || hint.isEmpty ? 'none' : _redact(hint)}',
    ]);
  } else {
    lines.add('- none_captured: yes');
  }

  return '${lines.join('\n')}\n';
}
