/// Local app preferences — the desktop counterpart of the web client's
/// localStorage keys ('jeliya.lastRoom', 'jeliya.draft.{roomId}',
/// 'jeliya.aliases.v1'). One JSON file in the daemon data dir; every access
/// degrades silently, exactly like the reference's try/catch-wrapped storage.
///
/// Aliases are LOCAL ONLY — names never leave this machine (never wire data).
library;

import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';

/// A [ChangeNotifier] so alias renames re-render name labels everywhere.
class PrefsStore extends ChangeNotifier {
  /// Backed by the JSON file at [path] (conventionally
  /// `<data_dir>/app_prefs.json`). Call [load] before first use.
  PrefsStore(this.path);

  /// In-memory store for widget tests — no disk I/O at all.
  PrefsStore.inMemory() : path = null;

  /// Null in in-memory mode.
  final String? path;

  String? _lastRoomId;
  String? _textLocale;
  String? _formattingLocale;
  final Map<String, String> _drafts = {};
  final Map<String, String> _aliases = {};
  final Map<String, int> _lastSeen = {};
  final Set<String> _pinnedRooms = {};
  final Set<String> _archivedRooms = {};

  bool _lastWriteOk = true;

  /// Whether the MOST RECENT persistence attempt reached disk. True also for
  /// the in-memory no-op path (`path == null`): nothing to persist is not a
  /// failure. False only when a real disk write threw — the setter's in-memory
  /// change still took effect this session, so the caller can honestly say the
  /// change applies for this session but was not written. Read it right after
  /// a setter to decide whether to surface a "session only" note.
  bool get lastWriteOk => _lastWriteOk;

  /// UI language as a BCP-47 tag ('textLocale'); null follows the system
  /// language (MaterialApp resolves against supportedLocales).
  String? get textLocale => _textLocale;

  set textLocale(String? tag) {
    final v = _normalizedTag(tag);
    if (_textLocale == v) return;
    _textLocale = v;
    _save();
    notifyListeners();
  }

  /// Date/number-convention tag ('formattingLocale') — deliberately separate
  /// from [textLocale] (glossary decision 4: a Bambara UI on a French system
  /// formats dates the French way); null follows the system locale.
  String? get formattingLocale => _formattingLocale;

  set formattingLocale(String? tag) {
    final v = _normalizedTag(tag);
    if (_formattingLocale == v) return;
    _formattingLocale = v;
    _save();
    notifyListeners();
  }

  static String? _normalizedTag(String? tag) {
    final t = tag?.trim() ?? '';
    return t.isEmpty ? null : t;
  }

  /// The last opened room id ('jeliya.lastRoom').
  String? get lastRoomId => _lastRoomId;

  set lastRoomId(String? roomId) {
    if (_lastRoomId == roomId) return;
    _lastRoomId = roomId;
    _save();
    notifyListeners();
  }

  /// The composer draft for [roomId] ('jeliya.draft.{roomId}'), or null.
  String? draftFor(String roomId) => _drafts[roomId];

  /// Saved on each keystroke; an empty/blank draft removes the entry.
  void setDraft(String roomId, String draft) {
    if (draft.isEmpty) {
      if (_drafts.remove(roomId) == null) return;
    } else {
      if (_drafts[roomId] == draft) return;
      _drafts[roomId] = draft;
    }
    _save();
    // No notify: drafts are only read on room switch (like the reference),
    // and notifying on every keystroke would rebuild listeners for nothing.
  }

  /// Local peer aliases ('jeliya.aliases.v1'): identity id → name.
  Map<String, String> get aliases => Map.unmodifiable(_aliases);

  String? aliasFor(String identityId) => _aliases[identityId];

  /// A trimmed non-empty [name] stores the alias; null/blank deletes it
  /// (the Rename modal's Save/Clear-alias semantics).
  void setAlias(String identityId, String? name) {
    final trimmed = name?.trim() ?? '';
    if (trimmed.isEmpty) {
      if (_aliases.remove(identityId) == null) return;
    } else {
      if (_aliases[identityId] == trimmed) return;
      _aliases[identityId] = trimmed;
    }
    _save();
    notifyListeners();
  }

  /// Device-local unread marks ('jeliya.lastSeen'): room id → the newest
  /// signed-event ts this device has acknowledged (docs/room-attention.md,
  /// decision 3). Local only — never wire data, never a delivery/read receipt.
  int? lastSeenFor(String roomId) => _lastSeen[roomId];

  /// Establish the baseline the first time a room appears on this device, so a
  /// backlog that synced before it was ever opened does not read as unread
  /// (decision 3). Writes only when no mark exists.
  void seedRoomSeen(String roomId, int ts) {
    if (_lastSeen.containsKey(roomId)) return;
    _lastSeen[roomId] = ts;
    _save();
    notifyListeners();
  }

  /// Clear unread for one room by advancing its mark to [ts] (never backwards,
  /// so an out-of-order replay cannot re-raise a cleared dot). Affects only
  /// [roomId] (decision 3).
  void markRoomSeen(String roomId, int ts) {
    final current = _lastSeen[roomId];
    if (current != null && current >= ts) return;
    _lastSeen[roomId] = ts;
    _save();
    notifyListeners();
  }

  /// Device-local pin / archive marks for the room list ('pinnedRooms' /
  /// 'archivedRooms'), the desktop counterpart of the web client's
  /// 'jeliya.rooms.v1' localStorage key (issue #64; docs/room-attention.md,
  /// decision 1: device-local state). Where a room sits in *your* list — never
  /// wire data, never a claim about the room. Pin and archive are mutually
  /// exclusive, so the two sets stay disjoint.
  Set<String> get pinnedRooms => Set.unmodifiable(_pinnedRooms);
  Set<String> get archivedRooms => Set.unmodifiable(_archivedRooms);

  bool isPinned(String roomId) => _pinnedRooms.contains(roomId);
  bool isArchived(String roomId) => _archivedRooms.contains(roomId);

  /// Toggle a room's pin. Pinning clears any archive mark (the two are
  /// exclusive); unpinning leaves the room unarchived.
  void togglePinned(String roomId) {
    if (!_pinnedRooms.remove(roomId)) {
      _pinnedRooms.add(roomId);
      _archivedRooms.remove(roomId);
    }
    _save();
    notifyListeners();
  }

  /// Toggle a room's archive. Archiving clears any pin mark (the two are
  /// exclusive); unarchiving leaves the room unpinned.
  void toggleArchived(String roomId) {
    if (!_archivedRooms.remove(roomId)) {
      _archivedRooms.add(roomId);
      _pinnedRooms.remove(roomId);
    }
    _save();
    notifyListeners();
  }

  /// Read the JSON file; malformed content and non-string values are dropped
  /// (the reference drops non-string alias values on load). Never throws.
  Future<void> load() async {
    final p = path;
    if (p == null) return;
    try {
      final raw = await File(p).readAsString();
      final decoded = jsonDecode(raw);
      if (decoded is! Map) return;
      final map = decoded.cast<String, dynamic>();
      final last = map['lastRoom'];
      _lastRoomId = last is String && last.isNotEmpty ? last : null;
      _textLocale = _optionalString(map['textLocale']);
      _formattingLocale = _optionalString(map['formattingLocale']);
      _drafts
        ..clear()
        ..addAll(_stringMap(map['drafts']));
      _aliases
        ..clear()
        ..addAll(_stringMap(map['aliases']));
      _lastSeen
        ..clear()
        ..addAll(_intMap(map['lastSeen']));
      final pinned = _stringSet(map['pinnedRooms']);
      final archived = _stringSet(map['archivedRooms'])..removeAll(pinned);
      _pinnedRooms
        ..clear()
        ..addAll(pinned);
      _archivedRooms
        ..clear()
        ..addAll(archived);
      notifyListeners();
    } catch (_) {
      // Missing or corrupt prefs file — start fresh, like localStorage misses.
    }
  }

  static String? _optionalString(dynamic v) =>
      v is String && v.isNotEmpty ? v : null;

  static Map<String, String> _stringMap(dynamic v) {
    if (v is! Map) return const {};
    return {
      for (final entry in v.entries)
        if (entry.key is String && entry.value is String)
          entry.key as String: entry.value as String,
    };
  }

  static Map<String, int> _intMap(dynamic v) {
    if (v is! Map) return const {};
    return {
      for (final entry in v.entries)
        if (entry.key is String && entry.value is int)
          entry.key as String: entry.value as int,
    };
  }

  static Set<String> _stringSet(dynamic v) {
    if (v is! List) return <String>{};
    return {
      for (final item in v)
        if (item is String && item.isNotEmpty) item,
    };
  }

  /// Persist to disk. Returns true when the write reached disk OR when there is
  /// nothing to persist (in-memory mode, `path == null`); false when the write
  /// threw. Either way the in-memory change made by the calling setter has
  /// already taken effect — persistence failure never rolls it back. Sets
  /// [lastWriteOk] on every call so a setter can be followed by an honest
  /// "session only" check.
  bool _save() {
    final p = path;
    if (p == null) return _lastWriteOk = true;
    try {
      final file = File(p);
      file.parent.createSync(recursive: true);
      // Write-then-rename: a crash mid-write must never destroy the previous
      // prefs (rename is atomic on the same filesystem).
      final tmp = File('$p.tmp');
      tmp.writeAsStringSync(jsonEncode({
        if (_lastRoomId != null) 'lastRoom': _lastRoomId,
        if (_textLocale != null) 'textLocale': _textLocale,
        if (_formattingLocale != null) 'formattingLocale': _formattingLocale,
        'drafts': _drafts,
        'aliases': _aliases,
        'lastSeen': _lastSeen,
        'pinnedRooms': _pinnedRooms.toList(),
        'archivedRooms': _archivedRooms.toList(),
      }));
      tmp.renameSync(p);
      return _lastWriteOk = true;
    } catch (_) {
      // The write failed (unwritable dir, full disk, permissions). The setter's
      // in-memory change stands for this session; record the miss so the UI can
      // say so instead of implying a false persisted success.
      return _lastWriteOk = false;
    }
  }
}
