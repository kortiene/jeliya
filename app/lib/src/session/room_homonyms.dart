/// Homonymous-room detection (docs/room-workbench.md, decision 6).
///
/// `room_id` is identity; `name` is a non-unique, daemon-local label that is
/// `null` until the genesis event syncs. Two rooms are HOMONYMS when the names
/// the UI actually displays are equal after trimming and case-folding —
/// INCLUDING the untitled placeholder, so two unsynced (null-named) rooms are
/// homonyms of each other, which is the most likely case for a new user.
///
/// Any surface where acting on the wrong room matters shows the short room id
/// for a room in this set (the disambiguator), via the shared `shortId` helper.
/// This is the SAME rule and the SAME facts the React client uses
/// (`ui/src/lib/rooms.ts`) — the milestone AC.
library;

/// The set of `room_id`s that share a displayed name with at least one OTHER
/// room in [rooms].
///
/// Display-driven on purpose: it folds over the SAME name the row shows
/// (`name ?? untitledLabel`), trimmed and lower-cased, so a homonym is exactly
/// what the eye sees as a duplicate. [untitledLabel] is the localized
/// placeholder a null name renders as (`AppStrings.shellUntitledRoom`), folded
/// in so untitled rooms count as homonyms of one another.
///
/// Pure — no widget, no localization lookup, no I/O — so it unit-tests without
/// a tester. Callers map their own row type (`RoomSummary`, `FleetAgentRoom`,
/// …) onto the `(roomId, name)` record. Repeated `room_id`s under one display
/// name collapse (a room the fleet references from several agents is not a
/// homonym of itself).
Set<String> homonymousRoomIds(
  Iterable<({String roomId, String? name})> rooms, {
  required String untitledLabel,
}) {
  final idsByDisplayName = <String, Set<String>>{};
  for (final room in rooms) {
    final display = (room.name ?? untitledLabel).trim().toLowerCase();
    (idsByDisplayName[display] ??= <String>{}).add(room.roomId);
  }
  final homonyms = <String>{};
  for (final ids in idsByDisplayName.values) {
    if (ids.length > 1) homonyms.addAll(ids);
  }
  return homonyms;
}
