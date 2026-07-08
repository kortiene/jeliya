/// Optimistic pending-message lifecycle, ported 1:1 from the reference
/// `sendMessage` flow (ui/src/App.tsx) per docs/PROTOCOL.md "Optimistic
/// pending-message lifecycle". Your own message renders immediately under a
/// client-local id (never sent on the wire) in phases `sending → syncing →
/// failed`, and the client MUST converge on exactly one rendered item by
/// reconciling echo↔response by `event_id` at the three points it can observe
/// them (PROTOCOL.md Pushes, hazard 2):
///
/// 1. the `message.send` response — [PendingMessages.resolveSend];
/// 2. the live `room.event` push — [PendingMessages.reconcilePush];
/// 3. the `room.open` backlog on the next open —
///    [PendingMessages.reconcileBacklog].
///
/// One [PendingMessages] per room (the reference keys its map by `room_id`).
/// The recommended drive sequence, mirroring App.tsx `sendMessage`:
///
/// ```dart
/// final clientId = pending.beginSend(body);           // phase: sending
/// try {
///   final eventId = await client.messageSend(roomId, body);
///   pending.resolveSend(clientId, eventId,
///       echoAlreadyVisible: fold.contains(eventId));  // gone, or syncing
/// } catch (e) {
///   pending.failSend(clientId, e);                    // phase: failed
/// }
/// ```
///
/// On the ambiguous `connection_lost`-after-send case the reference UI offers
/// a Retry that re-sends (`beginSend(body, retryClientId: ...)`) — accepting
/// the duplicate risk from the `message.send` idempotency gap.
library;

import 'dart:collection';

import '../models.dart';
import '../protocol.dart';

/// The three client-local phases. Never wire data.
abstract final class PendingPhases {
  static const String sending = 'sending';
  static const String syncing = 'syncing';
  static const String failed = 'failed';
  static const List<String> all = [sending, syncing, failed];
}

/// One optimistic message (ui/src/components/Timeline.tsx `PendingMessage`).
class PendingMessage {
  const PendingMessage({
    required this.clientId,
    required this.body,
    required this.ts,
    required this.phase,
    this.eventId,
    this.error,
  });

  /// Client-local correlation id (`pending-<ms>-<seq>`); never sent on the wire.
  final String clientId;
  final String body;

  /// Local send-attempt time in ms — display only, not a signed event `ts`.
  final int ts;

  /// One of [PendingPhases.all].
  final String phase;

  /// The wire `event_id` from the `message.send` response; null until the
  /// response arrives (phase [PendingPhases.sending] / [PendingPhases.failed]).
  final String? eventId;

  /// Set only in phase [PendingPhases.failed].
  final RequestError? error;
}

/// Per-room pending-message state machine.
class PendingMessages {
  /// [clock] returns milliseconds since epoch (injectable for tests); it feeds
  /// both the client-local id and [PendingMessage.ts], like `Date.now()` in
  /// the reference.
  PendingMessages({int Function()? clock}) : _clock = clock ?? _defaultClock;

  static int _defaultClock() => DateTime.now().millisecondsSinceEpoch;

  final int Function() _clock;
  int _seq = 0;
  final List<PendingMessage> _messages = [];

  /// The pending messages for this room, oldest first — a live unmodifiable
  /// view.
  late final List<PendingMessage> messages = UnmodifiableListView(_messages);

  PendingMessage? byClientId(String clientId) {
    final i = _indexOf(clientId);
    return i < 0 ? null : _messages[i];
  }

  /// Begin a send. Without [retryClientId], appends a new
  /// [PendingPhases.sending] message and returns its fresh client-local id.
  /// With [retryClientId] (the reference Retry path), the existing message
  /// re-enters `sending` with a fresh [PendingMessage.ts] and a cleared error
  /// — its body is kept, and nothing happens if the id is unknown (mirroring
  /// the reference's map-update semantics).
  String beginSend(String body, {String? retryClientId}) {
    final ts = _clock();
    if (retryClientId != null) {
      final i = _indexOf(retryClientId);
      if (i >= 0) {
        final m = _messages[i];
        _messages[i] = PendingMessage(
          clientId: m.clientId,
          body: m.body,
          ts: ts,
          phase: PendingPhases.sending,
          eventId: m.eventId,
        );
      }
      return retryClientId;
    }
    final clientId = 'pending-$ts-${_seq++}';
    _messages.add(PendingMessage(
      clientId: clientId,
      body: body,
      ts: ts,
      phase: PendingPhases.sending,
    ));
    return clientId;
  }

  /// Reconcile the `message.send` response (reconciliation point 1). When the
  /// echo push already rendered this `event_id` ([echoAlreadyVisible] — the
  /// fan-out beat the response), the pending message is dropped outright;
  /// otherwise it moves to [PendingPhases.syncing] carrying [eventId] so a
  /// later push/backlog can retire it.
  void resolveSend(String clientId, String eventId, {required bool echoAlreadyVisible}) {
    if (echoAlreadyVisible) {
      _messages.removeWhere((m) => m.clientId == clientId);
      return;
    }
    final i = _indexOf(clientId);
    if (i < 0) return;
    final m = _messages[i];
    _messages[i] = PendingMessage(
      clientId: m.clientId,
      body: m.body,
      ts: m.ts,
      phase: PendingPhases.syncing,
      eventId: eventId,
      error: m.error,
    );
  }

  /// The send threw — move to [PendingPhases.failed] with the shaped error
  /// (anything non-[RequestError] becomes [ErrorCodes.internal] via
  /// [errorShape]).
  void failSend(String clientId, Object error) {
    final i = _indexOf(clientId);
    if (i < 0) return;
    final m = _messages[i];
    _messages[i] = PendingMessage(
      clientId: m.clientId,
      body: m.body,
      ts: m.ts,
      phase: PendingPhases.failed,
      eventId: m.eventId,
      error: errorShape(error),
    );
  }

  /// Reconcile a live `room.event` push for this room (reconciliation
  /// point 2): a `message` event retires the pending message carrying the same
  /// `event_id`. Other kinds are ignored, like the reference. Returns whether
  /// anything was retired.
  bool reconcilePush(TimelineEvent event) {
    if (event.kind != TimelineKinds.message) return false;
    final before = _messages.length;
    _messages.removeWhere((m) => m.eventId != null && m.eventId == event.eventId);
    return _messages.length != before;
  }

  /// Reconcile a `room.open` / `room.timeline` backlog (reconciliation
  /// point 3): pending messages whose `event_id` appears in [timeline] are
  /// retired; messages still awaiting their response (no `event_id`) are kept.
  /// Returns how many were retired.
  int reconcileBacklog(Iterable<TimelineEvent> timeline) {
    if (!_messages.any((m) => m.eventId != null)) return 0;
    final eventIds = timeline.map((e) => e.eventId).toSet();
    final before = _messages.length;
    _messages.removeWhere((m) => m.eventId != null && eventIds.contains(m.eventId));
    return before - _messages.length;
  }

  void clear() => _messages.clear();

  int _indexOf(String clientId) => _messages.indexWhere((m) => m.clientId == clientId);
}
