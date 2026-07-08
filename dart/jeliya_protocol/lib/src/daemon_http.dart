/// The daemon's loopback HTTP surface, from the native-client side
/// (docs/PROTOCOL.md, "Native file sharing" and the browser helpers):
///
/// - [stageAndShareFile] — share an arbitrary user file via the documented
///   staging convention (`file.share` itself refuses paths outside the daemon
///   data dir, the anti-exfiltration invariant for a loopback daemon);
/// - [buildLocalFileUrl] — the token-carrying `GET /api/files/local` URL for
///   a previously fetched local copy.
///
/// [SupervisedDaemonHttp] binds both to a [SidecarSupervisor]'s portfile so
/// the data dir, HTTP origin and per-start token are re-read each call and a
/// daemon restart heals. Pure `dart:io`; no HTTP upload endpoint is used —
/// natives stage on disk, only browsers need `POST /api/files/share`.
library;

import 'dart:io';

import 'methods.dart';
import 'models.dart';
import 'protocol.dart';
import 'supervisor.dart';

/// `file.share`'s hard size limit (docs/PROTOCOL.md, Files): 100 MiB.
const int maxSharedFileBytes = 104857600;

int _stageSeq = 0;

/// Share the arbitrary user file at [sourcePath] into [roomId] via the native
/// staging convention: copy it to `<data_dir>/uploads/<unique-name>`, call
/// `file.share` on the staged path, then ALWAYS delete the staged copy — the
/// daemon has imported the blob by the time `file.share` returns, and a failed
/// share must not leak bytes into the data dir either. This mirrors what the
/// daemon itself does for browser uploads on `POST /api/files/share`.
///
/// [dataDir] must be the daemon's data dir (prefer the portfile's own
/// `data_dir`; see [SupervisedDaemonHttp.shareUserFile]). [name] defaults to
/// the source basename; either way only a basename survives, so a display
/// name can never traverse out of the staging dir. Files larger than
/// [maxSharedFileBytes] fail BEFORE the copy with the same typed error the
/// daemon would mint ([RequestError], code [ErrorCodes.invalidParams]).
Future<FileShareResult> stageAndShareFile(
  Client client, {
  required String dataDir,
  required String roomId,
  required String sourcePath,
  String? name,
  String? mime,
}) async {
  final source = File(sourcePath);
  final int size;
  try {
    size = await source.length();
  } on FileSystemException catch (e) {
    throw RequestError(
      ErrorCodes.invalidParams,
      'could not read $sourcePath: ${e.osError?.message ?? e.message}',
      hint: 'pick a readable file',
    );
  }
  if (size > maxSharedFileBytes) {
    throw RequestError(
      ErrorCodes.invalidParams,
      'file is $size bytes; the share limit is $maxSharedFileBytes bytes',
      hint: 'files over 100 MiB cannot be shared',
    );
  }
  final displayName = _displayName(name, sourcePath);
  final stageDir = Directory('$dataDir/uploads');
  await stageDir.create(recursive: true);
  final staged = File('${stageDir.path}/${_uniqueStageName(displayName)}');
  try {
    // Inside the try: a failed/partial copy (disk full, source vanished) must
    // not leak bytes into the data dir any more than a failed share may.
    await source.copy(staged.path);
    return await client.fileShare(
      roomId: roomId,
      path: staged.path,
      name: displayName,
      mime: mime,
    );
  } finally {
    try {
      staged.deleteSync();
    } catch (_) {/* already gone — nothing staged may linger, but never mask the share error */}
  }
}

/// The `GET /api/files/local` URL serving a previously fetched local copy from
/// the daemon's loopback origin, token included (`/api/files/*` is gated on
/// the per-start token). The daemon resolves `(room_id, file_id)` against
/// verified local fetch state and answers as a download
/// (`Content-Disposition: attachment`, nosniff, inert content-type); missing
/// or stale local copies return the standard JSON error envelope.
Uri buildLocalFileUrl({
  required Uri httpBase,
  required String token,
  required String roomId,
  required String fileId,
}) =>
    httpBase.replace(
      path: '/api/files/local',
      queryParameters: {'room_id': roomId, 'file_id': fileId, 'token': token},
    );

/// The HTTP-surface helpers bound to a [SidecarSupervisor], resolving the data
/// dir, HTTP origin and auth token from the portfile on every call so a
/// daemon restart (new port, new token) heals.
extension SupervisedDaemonHttp on SidecarSupervisor {
  /// [stageAndShareFile] against this supervisor's daemon. Stages into the
  /// portfile's `data_dir` when readable — the daemon's own canonicalized
  /// spelling of the directory — falling back to the configured [dataDir].
  Future<FileShareResult> shareUserFile(
    Client client, {
    required String roomId,
    required String sourcePath,
    String? name,
    String? mime,
  }) =>
      stageAndShareFile(
        client,
        dataDir: readPortfile()?.dataDir ?? dataDir,
        roomId: roomId,
        sourcePath: sourcePath,
        name: name,
        mime: mime,
      );

  /// [buildLocalFileUrl] on this supervisor's current HTTP origin and token.
  /// Throws [SidecarError] with no readable portfile.
  Uri localFileUrl({required String roomId, required String fileId}) {
    final token = authToken;
    if (token == null) throw SidecarError('no portfile to read an auth token from');
    return buildLocalFileUrl(
      httpBase: httpBase(),
      token: token,
      roomId: roomId,
      fileId: fileId,
    );
  }
}

/// Only a basename survives, unsafe characters are replaced and the result is
/// capped at 180 chars — a 1:1 port of the daemon's `upload_display_name`
/// (serve.rs), so a long-but-legal filename cannot ENAMETOOLONG the staged
/// copy where the browser upload path would have succeeded.
String _displayName(String? name, String sourcePath) {
  final trimmed = name?.trim();
  final candidate = (trimmed == null || trimmed.isEmpty) ? sourcePath : trimmed;
  final normalized = candidate.replaceAll(r'\', '/');
  final base = normalized.substring(normalized.lastIndexOf('/') + 1).trim();
  final cleaned = String.fromCharCodes(base.runes.map((r) {
    const unsafe = ['/', r'\', ':', '*', '?', '"', '<', '>', '|'];
    final ch = String.fromCharCode(r);
    if (unsafe.contains(ch) || r < 0x20 || r == 0x7f) return 0x5f; // '_'
    return r;
  }));
  final capped = cleaned.runes.length <= 180
      ? cleaned
      : String.fromCharCodes(cleaned.runes.take(180));
  return capped.trim().isEmpty ? 'upload.bin' : capped;
}

/// Mirrors the daemon's `unique_stage_name` (pid + timestamp + display name),
/// plus a per-process sequence so two stages in the same microsecond cannot
/// collide.
String _uniqueStageName(String displayName) =>
    '$pid-${DateTime.now().microsecondsSinceEpoch}-${_stageSeq++}-$displayName';
