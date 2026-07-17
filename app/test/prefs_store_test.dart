/// Device-local unread marks in PrefsStore (docs/room-attention.md, decision 3)
/// — the Flutter counterpart of the web client's `jeliya.lastSeen` localStorage
/// key. Marks are local only, never wire data, never a delivery/read receipt.
/// Mirrors the seed/mark/persist semantics of ui/src/lib/lastSeen.ts.
library;

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/session/prefs_store.dart';

const _roomA = 'blake3:1111111111111111111111111111111111111111111111111111111111111111';
const _roomB = 'blake3:2222222222222222222222222222222222222222222222222222222222222222';

void main() {
  test('seedRoomSeen establishes a baseline only when the room has no mark', () async {
    final dir = await Directory.systemTemp.createTemp('jeliya_prefs');
    addTearDown(() => dir.delete(recursive: true));
    final path = '${dir.path}/app_prefs.json';

    final store = PrefsStore(path);
    store.seedRoomSeen(_roomA, 100);
    expect(store.lastSeenFor(_roomA), 100);

    // A second seed never overwrites the acknowledged mark.
    store.seedRoomSeen(_roomA, 500);
    expect(store.lastSeenFor(_roomA), 100);

    final reloaded = PrefsStore(path);
    await reloaded.load();
    expect(reloaded.lastSeenFor(_roomA), 100);
  });

  test('markRoomSeen advances forward, never backwards, and isolates rooms', () async {
    final dir = await Directory.systemTemp.createTemp('jeliya_prefs');
    addTearDown(() => dir.delete(recursive: true));
    final path = '${dir.path}/app_prefs.json';

    final store = PrefsStore(path);
    store.markRoomSeen(_roomA, 100);
    store.markRoomSeen(_roomB, 100);
    store.markRoomSeen(_roomA, 300); // advances
    store.markRoomSeen(_roomA, 200); // ignored — never moves backwards
    expect(store.lastSeenFor(_roomA), 300);
    expect(store.lastSeenFor(_roomB), 100);

    final reloaded = PrefsStore(path);
    await reloaded.load();
    expect(reloaded.lastSeenFor(_roomA), 300);
    expect(reloaded.lastSeenFor(_roomB), 100);
  });

  test('unknown room has no mark', () {
    final store = PrefsStore.inMemory();
    expect(store.lastSeenFor(_roomA), isNull);
  });

  test('non-int marks on disk are dropped, never crashed on', () async {
    final dir = await Directory.systemTemp.createTemp('jeliya_prefs');
    addTearDown(() => dir.delete(recursive: true));
    final path = '${dir.path}/app_prefs.json';

    await File(path).writeAsString(
      '{"lastSeen": {"$_roomA": 300, "$_roomB": "nope", "bad": null}}',
    );
    final store = PrefsStore(path);
    await store.load();
    expect(store.lastSeenFor(_roomA), 300);
    expect(store.lastSeenFor(_roomB), isNull);
  });

  // Device-local pin/archive marks (issue #64) — the counterpart of the web
  // client's 'jeliya.rooms.v1' key, mirroring roomFlags.ts.
  test('togglePinned adds and removes a pin, persisting across reload', () async {
    final dir = await Directory.systemTemp.createTemp('jeliya_prefs');
    addTearDown(() => dir.delete(recursive: true));
    final path = '${dir.path}/app_prefs.json';

    final store = PrefsStore(path);
    store.togglePinned(_roomA);
    expect(store.isPinned(_roomA), isTrue);
    expect(store.pinnedRooms, {_roomA});

    final reloaded = PrefsStore(path);
    await reloaded.load();
    expect(reloaded.isPinned(_roomA), isTrue);

    reloaded.togglePinned(_roomA);
    expect(reloaded.isPinned(_roomA), isFalse);
  });

  test('pin and archive are mutually exclusive', () {
    final store = PrefsStore.inMemory();
    store.toggleArchived(_roomA);
    expect(store.isArchived(_roomA), isTrue);

    // Pinning an archived room clears the archive mark.
    store.togglePinned(_roomA);
    expect(store.isPinned(_roomA), isTrue);
    expect(store.isArchived(_roomA), isFalse);

    // Archiving a pinned room clears the pin mark.
    store.toggleArchived(_roomA);
    expect(store.isArchived(_roomA), isTrue);
    expect(store.isPinned(_roomA), isFalse);
  });

  test('a room recorded as both pinned and archived on disk loads as pinned', () async {
    final dir = await Directory.systemTemp.createTemp('jeliya_prefs');
    addTearDown(() => dir.delete(recursive: true));
    final path = '${dir.path}/app_prefs.json';

    await File(path).writeAsString(
      '{"pinnedRooms": ["$_roomA"], "archivedRooms": ["$_roomA", "$_roomB"]}',
    );
    final store = PrefsStore(path);
    await store.load();
    expect(store.pinnedRooms, {_roomA});
    expect(store.archivedRooms, {_roomB});
  });

  test('non-string entries in the pin/archive lists are dropped', () async {
    final dir = await Directory.systemTemp.createTemp('jeliya_prefs');
    addTearDown(() => dir.delete(recursive: true));
    final path = '${dir.path}/app_prefs.json';

    await File(path).writeAsString(
      '{"pinnedRooms": ["$_roomA", 42, "", null], "archivedRooms": "nope"}',
    );
    final store = PrefsStore(path);
    await store.load();
    expect(store.pinnedRooms, {_roomA});
    expect(store.archivedRooms, isEmpty);
  });
}
