/// Device-local unread projection (docs/room-attention.md, decision 3) — the
/// Dart mirror of the reference `ui/src/lib/lastSeen.ts` predicate. Unread is a
/// statement about THIS device's last look and nothing else: the protocol has
/// no delivery or read receipt, so unread here is the honest absence of one and
/// never implies another participant read or received anything.
///
/// The last-seen marks themselves are device-local storage, held by the app's
/// PrefsStore (the counterpart of the web client's `jeliya.lastSeen`
/// localStorage key); this file is only the pure verdict over them.
library;

import '../models.dart';

/// Unread iff the room has a signed event newer than this device's last-seen
/// mark (docs/room-attention.md, decision 3). Never a delivery/read receipt.
///
/// No recency (null [RoomSummary.lastEventTs]) and no baseline (null
/// [lastSeen]) both read as NOT unread: an unread dot is a claim, and neither
/// case holds the evidence for one — the app seeds a baseline for every listed
/// room, and only genuine activity after that baseline flags.
bool roomUnread(RoomSummary room, int? lastSeen) {
  final ts = room.lastEventTs;
  if (ts == null || lastSeen == null) return false;
  return ts > lastSeen;
}

/// Whether a live `room.event` for [room] should establish that room's unread
/// baseline (docs/room-attention.md, decision 3, amended for issue #154) — the
/// Dart mirror of `shouldSeedFromLiveEvent` in `ui/src/lib/lastSeen.ts`.
///
/// True only when the listed row carries NO recency. The daemon guarantees
/// every listed room has recency — a room with no stored events is not listed
/// at all — so a null here means the daemon predates the projection, and there
/// is nothing for the normal seeding path to seed from. Absorbing the first
/// observed event as the baseline is the honest floor: a push can carry
/// late-validated backlog, so "arrived live" is not proof of new activity, but
/// without a baseline such a daemon could never raise a dot at all.
///
/// False for an unlisted room (nothing to be unread yet) and false whenever the
/// row has recency, so this can never mask activity against a current daemon.
bool shouldSeedFromLiveEvent(RoomSummary? room) =>
    room != null && room.lastEventTs == null;
