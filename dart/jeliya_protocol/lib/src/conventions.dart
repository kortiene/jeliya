/// Client conventions, ported 1:1 from the reference web client per
/// docs/PROTOCOL.md "Client conventions" and "Pushes". They are non-normative
/// on the wire (a second client is free to differ), but they carry the honesty
/// rules into the UI, so this Dart port mirrors the reference behaviors
/// exactly for cross-client parity:
///
/// - timeline fold (insert-by-`ts`, `event_id` dedup) — `timeline_fold.dart`;
/// - optimistic pending-message lifecycle — `pending_messages.dart`;
/// - combined-invite splitting — `invite.dart`;
/// - `room.join` retry ladder — `join.dart`;
/// - agent-status label tone — `format.dart`;
/// - fleet attention projection (Needs Attention closed set) — `fleet.dart`;
/// - file-fetch state fold (never downgrade, `hash_mismatch` hard stop) —
///   `fetch_state.dart`;
/// - device-local unread projection — `room_attention.dart`;
/// - redacted support diagnostics — `diagnostics.dart`.
library;

export 'conventions/diagnostics.dart';
export 'conventions/fetch_state.dart';
export 'conventions/fleet.dart';
export 'conventions/format.dart';
export 'conventions/invite.dart';
export 'conventions/join.dart';
export 'conventions/pending_messages.dart';
export 'conventions/room_attention.dart';
export 'conventions/timeline_fold.dart';
