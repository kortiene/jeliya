/// The searchable, stateful room-list projection (issue #64) — the Dart mirror
/// of the reference `ui/src/lib/roomList.ts`. A single pure function turns the
/// raw `room.list` result plus this device's search, lifecycle filter, and
/// pin/archive choices into an ordered, sectioned view, so React and Flutter
/// section, order, and disambiguate one room list identically. The shared
/// `ui/src/lib/conformance/room-list.fixtures.json` (replayed by both clients)
/// is the parity guard.
///
/// It renders no recency, unread, or attention itself: those are per-row claims
/// the surface layers on from the #63 evidence primitives (roomUnread /
/// PrefsStore last-seen), and only where the evidence exists
/// (docs/room-attention.md, decision 6). This file decides only *which rooms
/// show, in what order, under which section*.
library;

import 'package:jeliya_protocol/jeliya_protocol.dart' show RoomSummary;

import 'room_homonyms.dart';

/// Which lifecycle the list shows. [departed] is the signed Left/Removed set
/// (docs/room-workbench.md, decision 4). A filter separates lifecycles; it never
/// drops a room from existence — every room is reachable under some filter.
enum LifecycleFilter { all, active, departed }

/// The sections a projected list can contain, in canonical render order. Only
/// non-empty sections are emitted. [pinned] floats above lifecycle; [archived]
/// is the device-local put-away bucket, orthogonal to the lifecycle filter.
enum RoomSectionKey { pinned, active, departed, archived }

/// A room's lifecycle. A null/unknown status is [active] (a joined room whose
/// status has not synced is not "departed"), matching what the sidebar renders.
enum RoomLifecycle { active, departed }

RoomLifecycle roomLifecycle(RoomSummary room) =>
    room.status == 'left' || room.status == 'removed'
        ? RoomLifecycle.departed
        : RoomLifecycle.active;

class RoomListRow {
  const RoomListRow({
    required this.room,
    required this.displayName,
    required this.isHomonym,
  });

  final RoomSummary room;

  /// The name the row shows — the label or the untitled placeholder.
  final String displayName;

  /// True iff this room shares a display name with another room in the SAME
  /// visible result (docs/room-workbench.md, decision 6). Recomputed over the
  /// filtered/searched subset, not the full list: the short-id disambiguator is
  /// noise when the twin that made it a homonym is not even on screen.
  final bool isHomonym;
}

class RoomListSection {
  const RoomListSection({required this.key, required this.rows});

  final RoomSectionKey key;
  final List<RoomListRow> rows;
}

class RoomListView {
  const RoomListView({
    required this.sections,
    required this.visibleCount,
    required this.totalCount,
    required this.hasQuery,
  });

  /// Non-empty sections in canonical order (pinned, active, departed, archived).
  final List<RoomListSection> sections;

  /// Rooms shown across all sections, after search + filter. Zero with a
  /// non-empty [totalCount] is "no matches", not "no rooms".
  final int visibleCount;

  /// Rooms supplied before any search/filter — so the surface tells an empty
  /// search result apart from a genuinely roomless account.
  final int totalCount;

  /// Whether a non-blank search query was applied.
  final bool hasQuery;
}

/// The room-id with any `namespace:` prefix stripped — the exact characters the
/// short-id disambiguator shows (`shortId`). Search folds against this so
/// "searching by short room ID" matches what the eye reads, not the wire
/// namespace.
String _rawId(String id) =>
    id.contains(':') ? id.substring(id.indexOf(':') + 1) : id;

/// A room matches a query when the query (trimmed, case-folded) is a substring
/// of its display name OR of its raw room-id. Blank query matches everything.
/// [untitledLabel] is the localized placeholder a null name renders as, folded
/// in so an unsynced room is still searchable by its shown name.
bool roomMatchesQuery(RoomSummary room, String query, String untitledLabel) {
  final q = query.trim().toLowerCase();
  if (q.isEmpty) return true;
  if ((room.name ?? untitledLabel).toLowerCase().contains(q)) return true;
  return _rawId(room.roomId).toLowerCase().contains(q);
}

/// Order within a section: newest signed activity first (the recency projection
/// of docs/room-attention.md, decision 2), rooms with no recency last, then by
/// display name, then by room-id for a total, stable order.
int _compareRooms(RoomSummary a, RoomSummary b, String untitledLabel) {
  final ta = a.lastEventTs;
  final tb = b.lastEventTs;
  if (ta != tb) {
    if (ta == null) return 1;
    if (tb == null) return -1;
    return tb.compareTo(ta);
  }
  final na = (a.name ?? untitledLabel).toLowerCase();
  final nb = (b.name ?? untitledLabel).toLowerCase();
  if (na != nb) return na.compareTo(nb);
  return a.roomId.compareTo(b.roomId);
}

/// Project the raw room list into the searched, filtered, ordered, sectioned
/// view both clients render. Pure — no I/O, no clock — so it unit-tests against
/// the shared fixtures without a tester. [untitledLabel] is the localized
/// null-name placeholder (`AppStrings.shellUntitledRoom`), injected because the
/// display name it folds on is app-level l10n.
RoomListView projectRoomList({
  required List<RoomSummary> rooms,
  required String query,
  required LifecycleFilter filter,
  required Set<String> pinned,
  required Set<String> archived,
  required String untitledLabel,
}) {
  final totalCount = rooms.length;
  final hasQuery = query.trim().isNotEmpty;

  final pinnedRooms = <RoomSummary>[];
  final activeRooms = <RoomSummary>[];
  final departedRooms = <RoomSummary>[];
  final archivedRooms = <RoomSummary>[];

  for (final room in rooms) {
    if (!roomMatchesQuery(room, query, untitledLabel)) continue;
    // Archive is a search-only bucket, orthogonal to the lifecycle filter: a
    // room you deliberately put away stays found by name, and switching the
    // filter never surfaces it back into the main sections.
    if (archived.contains(room.roomId)) {
      archivedRooms.add(room);
      continue;
    }
    final lifecycle = roomLifecycle(room);
    // The filter applies uniformly to everything non-archived — including
    // pinned, so "Active" never shows a departed room and "Left & removed"
    // never shows an active one, whatever its pin state.
    if (filter == LifecycleFilter.active && lifecycle != RoomLifecycle.active) {
      continue;
    }
    if (filter == LifecycleFilter.departed &&
        lifecycle != RoomLifecycle.departed) {
      continue;
    }
    if (pinned.contains(room.roomId)) {
      pinnedRooms.add(room);
    } else if (lifecycle == RoomLifecycle.active) {
      activeRooms.add(room);
    } else {
      departedRooms.add(room);
    }
  }

  int cmp(RoomSummary a, RoomSummary b) => _compareRooms(a, b, untitledLabel);
  pinnedRooms.sort(cmp);
  activeRooms.sort(cmp);
  departedRooms.sort(cmp);
  archivedRooms.sort(cmp);

  // Homonyms over the full visible subset (every section, including a pinned
  // room and its unpinned twin), so the disambiguator appears wherever two
  // on-screen rooms collide and nowhere it would be noise.
  final visible = [
    ...pinnedRooms,
    ...activeRooms,
    ...departedRooms,
    ...archivedRooms,
  ];
  final homonyms = homonymousRoomIds(
    visible.map((r) => (roomId: r.roomId, name: r.name)),
    untitledLabel: untitledLabel,
  );
  RoomListRow toRow(RoomSummary room) => RoomListRow(
        room: room,
        displayName: room.name ?? untitledLabel,
        isHomonym: homonyms.contains(room.roomId),
      );

  final sections = <RoomListSection>[];
  if (pinnedRooms.isNotEmpty) {
    sections.add(RoomListSection(
        key: RoomSectionKey.pinned, rows: pinnedRooms.map(toRow).toList()));
  }
  if (activeRooms.isNotEmpty) {
    sections.add(RoomListSection(
        key: RoomSectionKey.active, rows: activeRooms.map(toRow).toList()));
  }
  if (departedRooms.isNotEmpty) {
    sections.add(RoomListSection(
        key: RoomSectionKey.departed, rows: departedRooms.map(toRow).toList()));
  }
  if (archivedRooms.isNotEmpty) {
    sections.add(RoomListSection(
        key: RoomSectionKey.archived, rows: archivedRooms.map(toRow).toList()));
  }

  return RoomListView(
    sections: sections,
    visibleCount: visible.length,
    totalCount: totalCount,
    hasQuery: hasQuery,
  );
}
