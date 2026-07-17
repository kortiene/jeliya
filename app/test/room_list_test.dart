/// The searchable, stateful room-list projection (room_list.dart) — the Dart
/// mirror of ui/src/lib/roomList.ts. Native cases pin the local behavior; the
/// shared-fixture block replays ui/src/lib/conformance/room-list.fixtures.json,
/// the SAME corpus the TypeScript test reads, so React and Flutter section,
/// order, and disambiguate one room list from one source (issue #64).
library;

import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/session/room_list.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart' show RoomSummary;

const _untitled = 'Untitled room';

RoomSummary _room(
  String roomId, {
  String? name,
  String? status = 'active',
  int? lastEventTs,
}) =>
    RoomSummary(
      roomId: roomId,
      name: name,
      status: status,
      memberCount: 0,
      open: false,
      lastEventTs: lastEventTs,
    );

/// Walk up to the repo root (the dir holding the shared conformance fixtures),
/// so this test reads the SAME manifest as the TypeScript one.
Directory _repoRoot() {
  var dir = Directory.current;
  for (var i = 0; i < 8; i++) {
    if (File('${dir.path}/ui/src/lib/conformance/room-list.fixtures.json')
        .existsSync()) {
      return dir;
    }
    final parent = dir.parent;
    if (parent.path == dir.path) break;
    dir = parent;
  }
  throw StateError('could not locate repo root from ${Directory.current.path}');
}

void main() {
  group('roomMatchesQuery', () {
    test('matches everything on a blank or whitespace-only query', () {
      final r = _room('blake3:aa', name: 'Design');
      expect(roomMatchesQuery(r, '', _untitled), isTrue);
      expect(roomMatchesQuery(r, '   ', _untitled), isTrue);
    });

    test('matches a case-folded, trimmed substring of the display name', () {
      final r = _room('blake3:aa', name: 'Design System');
      expect(roomMatchesQuery(r, '  SYSTEM ', _untitled), isTrue);
      expect(roomMatchesQuery(r, 'ops', _untitled), isFalse);
    });

    test('matches the untitled placeholder for a null-named room', () {
      final r = _room('blake3:aa', name: null);
      expect(roomMatchesQuery(r, 'untitled', _untitled), isTrue);
    });

    test('matches the raw room id (namespace stripped), not the namespace', () {
      final r = _room('blake3:beef0000cafe', name: 'Sandbox');
      expect(roomMatchesQuery(r, 'cafe', _untitled), isTrue);
      expect(roomMatchesQuery(r, 'beef', _untitled), isTrue);
      expect(roomMatchesQuery(r, 'blake3', _untitled), isFalse);
    });
  });

  group('roomLifecycle', () {
    test('left/removed are departed; everything else (incl. null) is active', () {
      expect(roomLifecycle(_room('r', status: 'left')), RoomLifecycle.departed);
      expect(
          roomLifecycle(_room('r', status: 'removed')), RoomLifecycle.departed);
      expect(roomLifecycle(_room('r', status: 'active')), RoomLifecycle.active);
      expect(roomLifecycle(_room('r', status: null)), RoomLifecycle.active);
    });
  });

  group('projectRoomList', () {
    RoomListView project(
      List<RoomSummary> rooms, {
      String query = '',
      LifecycleFilter filter = LifecycleFilter.all,
      Set<String> pinned = const {},
      Set<String> archived = const {},
    }) =>
        projectRoomList(
          rooms: rooms,
          query: query,
          filter: filter,
          pinned: pinned,
          archived: archived,
          untitledLabel: _untitled,
        );

    test('orders a section by recency desc, null recency last, name, then id', () {
      final view = project([
        _room('blake3:z', name: 'Zeta'),
        _room('blake3:a', name: 'Alpha'),
        _room('blake3:m', name: 'Mid', lastEventTs: 500),
      ]);
      expect(view.sections.single.rows.map((r) => r.room.name),
          ['Mid', 'Alpha', 'Zeta']);
    });

    test('breaks a full tie (equal recency and name) by room id', () {
      final view = project([
        _room('blake3:b', name: 'Same', lastEventTs: 10),
        _room('blake3:a', name: 'Same', lastEventTs: 10),
      ]);
      expect(view.sections.single.rows.map((r) => r.room.roomId),
          ['blake3:a', 'blake3:b']);
    });

    test('emits no empty sections', () {
      final view = project([_room('blake3:a', name: 'Only')]);
      expect(view.sections.map((s) => s.key), [RoomSectionKey.active]);
    });

    test('distinguishes an empty search result from a roomless account', () {
      final empty = project(const []);
      expect(empty.totalCount, 0);
      expect(empty.visibleCount, 0);

      final noMatch = project([_room('blake3:a', name: 'Design')], query: 'zzz');
      expect(noMatch.totalCount, 1);
      expect(noMatch.visibleCount, 0);
      expect(noMatch.hasQuery, isTrue);
    });
  });

  group('shared room-list fixtures (parity with React)', () {
    final file = File(
        '${_repoRoot().path}/ui/src/lib/conformance/room-list.fixtures.json');
    final cases = ((jsonDecode(file.readAsStringSync()) as Map<String, dynamic>)[
            'cases'] as List)
        .cast<Map<String, dynamic>>();

    test('covers every distinct case name once', () {
      final names = cases.map((c) => c['name'] as String).toSet();
      expect(names.length, cases.length);
    });

    for (final c in cases) {
      test('case "${c['name']}"', () {
        final rooms = (c['rooms'] as List).cast<Map<String, dynamic>>().map((r) {
          return _room(
            r['room_id'] as String,
            name: r['name'] as String?,
            status: r['status'] as String?,
            lastEventTs: r['last_event_ts'] as int?,
          );
        }).toList();
        final view = projectRoomList(
          rooms: rooms,
          query: c['query'] as String,
          filter: LifecycleFilter.values.byName(c['filter'] as String),
          pinned: (c['pinned'] as List).cast<String>().toSet(),
          archived: (c['archived'] as List).cast<String>().toSet(),
          untitledLabel: _untitled,
        );
        final expected = c['expect'] as Map<String, dynamic>;

        final sections = view.sections
            .map((s) => {
                  'key': s.key.name,
                  'rooms': s.rows.map((r) => r.room.roomId).toList(),
                })
            .toList();
        final expectedSections = (expected['sections'] as List)
            .cast<Map<String, dynamic>>()
            .map((s) => {
                  'key': s['key'],
                  'rooms': (s['rooms'] as List).cast<String>(),
                })
            .toList();
        expect(sections, expectedSections);

        final homonyms = view.sections
            .expand((s) => s.rows)
            .where((r) => r.isHomonym)
            .map((r) => r.room.roomId)
            .toList()
          ..sort();
        final expectedHomonyms = (expected['homonyms'] as List).cast<String>().toList()
          ..sort();
        expect(homonyms, expectedHomonyms);

        expect(view.visibleCount, expected['visibleCount']);
        expect(view.totalCount, expected['totalCount']);
        expect(view.hasQuery, expected['hasQuery']);
      });
    }
  });
}
