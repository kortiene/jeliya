/// File-fetch state fold, ported 1:1 from the reference `FetchState`
/// (ui/src/components/ui.tsx) and `persistedFetchState`/`mergeFetchedFiles`
/// (ui/src/App.tsx), per docs/PROTOCOL.md "File fetch: verified vs fetched
/// (never downgrade)":
///
/// - **verified** — set only from a live `file.fetch` result *this session*
///   (`verified: true`); asserts these bytes were hash-checked now.
/// - **fetched** — reconstructed from `file.list` persisted fields
///   (`fetched && local_path`); asserts only that the daemon reports a prior
///   local copy.
///
/// A `file.list` refresh MUST NOT downgrade an entry that is already
/// `verified` (or mid-fetch `pending`) back to plain `fetched`.
/// `hash_mismatch` is a hard stop ([FetchState.isHardStop]): discard the
/// copy, never render it, no retry (honesty rule 3).
library;

import '../models.dart';
import '../protocol.dart';

/// The four client-local fetch phases (honest taxonomy, no invented delivery
/// states). Never wire data.
abstract final class FetchPhases {
  static const String pending = 'pending';
  static const String verified = 'verified';
  static const String fetched = 'fetched';
  static const String error = 'error';
  static const List<String> all = [pending, verified, fetched, error];
}

/// One file's client-local fetch state (ui.tsx `FetchState`). [path]/[bytes]
/// are set for `verified`/`fetched`, [error] for `error`; [url] is the
/// optional `/api/files/local` link an app layer may attach.
class FetchState {
  /// A `file.fetch` call is in flight.
  const FetchState.pending()
      : phase = FetchPhases.pending,
        path = null,
        bytes = null,
        url = null,
        error = null;

  /// A live `file.fetch` succeeded this session — the strong claim.
  const FetchState.verified({required String this.path, required int this.bytes, this.url})
      : phase = FetchPhases.verified,
        error = null;

  /// Reconstructed from `file.list` persisted fields — the weaker claim.
  const FetchState.fetched({required String this.path, required int this.bytes, this.url})
      : phase = FetchPhases.fetched,
        error = null;

  /// The fetch failed with the shaped daemon error.
  const FetchState.error(RequestError this.error)
      : phase = FetchPhases.error,
        path = null,
        bytes = null,
        url = null;

  /// One of [FetchPhases.all].
  final String phase;
  final String? path;
  final int? bytes;
  final String? url;
  final RequestError? error;

  /// `hash_mismatch` is a hard stop per honesty rule 3: the copy was
  /// discarded, must never be rendered, and no retry is offered (the
  /// reference `FetchControl` shows a dead "Failed" for it).
  bool get isHardStop => error?.code == ErrorCodes.hashMismatch;
}

/// Reconstruct the weaker persisted `fetched` state from a `file.list` row
/// (App.tsx `persistedFetchState`), or null when the daemon reports no prior
/// local copy. [localFileUrl] mirrors the web UI's token-carrying
/// `/api/files/local` URL builder — supplied by the app layer (it needs the
/// daemon token); omitted leaves [FetchState.url] null.
FetchState? persistedFetchState(
  String roomId,
  FileEntry file, {
  String Function(String roomId, String fileId)? localFileUrl,
}) {
  final path = file.localPath;
  if (!file.fetched || path == null || path.isEmpty) return null;
  return FetchState.fetched(
    path: path,
    bytes: file.localBytes ?? file.size,
    url: localFileUrl?.call(roomId, file.fileId),
  );
}

/// Fold a `file.list` refresh into the current per-file fetch states (App.tsx
/// `mergeFetchedFiles`). Persisted `fetched` entries are merged in, but never
/// over an entry that is already [FetchPhases.pending] or
/// [FetchPhases.verified] — the never-downgrade rule. Copy-on-write: returns
/// [previous] itself (the same map instance) when nothing changed, and never
/// mutates it.
Map<String, FetchState> mergeFetchedFiles(
  Map<String, FetchState> previous,
  String roomId,
  List<FileEntry> files, {
  String Function(String roomId, String fileId)? localFileUrl,
}) {
  var next = previous;
  for (final file in files) {
    final persisted = persistedFetchState(roomId, file, localFileUrl: localFileUrl);
    if (persisted == null) continue;
    final current = next[file.fileId];
    if (current != null &&
        (current.phase == FetchPhases.pending || current.phase == FetchPhases.verified)) {
      continue;
    }
    if (identical(next, previous)) next = Map.of(previous);
    next[file.fileId] = persisted;
  }
  return next;
}
