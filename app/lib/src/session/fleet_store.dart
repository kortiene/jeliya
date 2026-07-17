/// Fleet data store for the top-level Agents view (FleetDashboard.tsx data
/// layer). The dashboard now stays mounted across navigation (so its search,
/// filter, and scroll survive), so this store outlives a single visit; the poll
/// loop is instead gated on lifecycle: it runs only while the Agents surface is
/// active AND the app is foregrounded ([setActive] / [setForeground]), and the
/// first return runs exactly one immediate reload. It never runs in the
/// background or behind another surface (#69).
///
/// Data flow:
/// - `agents.fleet` polled every 4000ms + one immediate load, PLUS a
///   400ms-debounced reload on any `room.event` push (numbers stay live
///   without hammering the daemon).
/// - Per-agent `agent.history` (limit 40) for the sparkline, keyed by the
///   latest status's room (else the agent's first room); refetched only when
///   `latest.ts` advances or the history room changes. History errors fall
///   back to empty points (dashed "no history" baseline) — never fabricated.
///
/// All liveness/label facts come straight off the wire models; nothing here
/// derives, stores, or extrapolates presence (honesty rule 4).
library;

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart';

/// Per-agent sparkline history: [points] is null while the FIRST fetch is in
/// flight (the solid "loading" baseline); on a refetch the previous points
/// stay visible until the new ones land (web parity — no flash).
class _AgentHistory {
  _AgentHistory({required this.roomId, required this.latestTs, this.points});

  /// The room the history was (or is being) read from; null when the agent
  /// has no room at all.
  final String? roomId;

  /// `latest.ts` at fetch time — the refetch key (sparkline tracks live
  /// progress without per-card polling).
  final int? latestTs;

  List<HistoryPoint>? points;
}

class FleetStore extends ChangeNotifier {
  FleetStore({
    required Client client,
    Duration pollInterval = const Duration(milliseconds: 4000),
    Duration pushDebounce = const Duration(milliseconds: 400),
  }) : _client = client,
       _pollInterval = pollInterval,
       _pushDebounce = pushDebounce {
    _pushSub = client.roomEvents.listen(_onRoomEvent);
    // The poll stays OFF until the surface is active AND the app is foregrounded
    // ([setActive] / [setForeground] drive it). The cheap push subscription
    // stays, but it only reloads while polling is enabled.
  }

  final Client _client;
  final Duration _pollInterval;
  final Duration _pushDebounce;

  Timer? _poll;
  Timer? _debounce;
  StreamSubscription<RoomEventPush>? _pushSub;

  FleetResult? _fleet;
  RequestError? _error;
  bool _loaded = false;
  bool _disposed = false;
  int _loadEpoch = 0;
  int _appliedEpoch = 0;

  bool _active = false;
  bool _foreground = true;
  int? _lastLoadedAtMs;

  final Map<String, _AgentHistory> _histories = {};

  // -- read surface ------------------------------------------------------------

  /// The last successful `agents.fleet` result (retained across errors — the
  /// list keeps its last data while the ErrorNote shows).
  FleetResult? get fleet => _fleet;

  /// The last load error, cleared on the next successful load.
  RequestError? get error => _error;

  /// True once the first `agents.fleet` call resolved (success OR error) —
  /// gates the skeleton state.
  bool get loaded => _loaded;

  /// True while the poll loop is running (surface active AND app foreground).
  bool get polling => _active && _foreground && !_disposed;

  /// Wall-clock ms of the last successful load — a device-local fact (when THIS
  /// client last refreshed), never a signed event. Null before the first
  /// success; the refresh indicator reads it so staleness is honest, not hidden.
  int? get lastLoadedAtMs => _lastLoadedAtMs;

  /// Sparkline points for [identityId]: null while the first history fetch is
  /// in flight, empty when there is no history (or the fetch failed).
  List<HistoryPoint>? historyFor(String identityId) =>
      _histories[identityId]?.points;

  // -- lifecycle gating --------------------------------------------------------

  /// Marks the Agents surface active (its route is the current one) or not. The
  /// retention counterpart of the old unmount-to-stop: pausing keeps the data,
  /// search, filter, and scroll; only the poll stops.
  void setActive(bool active) {
    if (_active == active) return;
    _active = active;
    _syncPoll();
  }

  /// App foreground/background, from the dashboard's [WidgetsBindingObserver].
  /// Backgrounding pauses the poll even while the surface is active; the first
  /// foreground resumes it with exactly one reload.
  void setForeground(bool foreground) {
    if (_foreground == foreground) return;
    _foreground = foreground;
    _syncPoll();
  }

  void _syncPoll() {
    final shouldPoll = polling;
    if (shouldPoll && _poll == null) {
      // Resume: exactly one immediate reload, then the periodic tick.
      _poll = Timer.periodic(_pollInterval, (_) => unawaited(load()));
      unawaited(load());
    } else if (!shouldPoll && _poll != null) {
      _poll!.cancel();
      _poll = null;
      _debounce?.cancel();
      _debounce = null;
    }
    if (!_disposed) notifyListeners(); // reflect the polling state in the UI
  }

  // -- loading ------------------------------------------------------------------

  /// One `agents.fleet` round trip. A response is discarded only when a
  /// NEWER response has already been applied — never merely because a newer
  /// request started (with a 4s poll and >4s latency, newest-request-wins
  /// would discard every response and show the skeleton forever).
  Future<void> load() async {
    final epoch = ++_loadEpoch;
    try {
      final f = await _client.agentsFleet();
      if (_disposed || epoch <= _appliedEpoch) return;
      _appliedEpoch = epoch;
      _fleet = f;
      _error = null;
      _loaded = true;
      _lastLoadedAtMs = DateTime.now().millisecondsSinceEpoch;
      _syncHistories(f.agents);
      notifyListeners();
    } catch (e) {
      if (_disposed || epoch <= _appliedEpoch) return;
      _appliedEpoch = epoch;
      _error = errorShape(e);
      _loaded = true;
      notifyListeners();
    }
  }

  void _onRoomEvent(RoomEventPush _) {
    // No background reloads: a push only nudges a refresh while the poll is
    // enabled (surface active + app foreground).
    if (!polling) return;
    _debounce?.cancel();
    _debounce = Timer(_pushDebounce, () => unawaited(load()));
  }

  // -- per-agent history ------------------------------------------------------------

  /// Reconcile the history cache with the fresh agent list: fetch for new
  /// agents, refetch when `latest.ts` advanced or the history room changed,
  /// and drop agents that left the fleet.
  void _syncHistories(List<FleetAgent> agents) {
    final seen = <String>{};
    for (final agent in agents) {
      seen.add(agent.identityId);
      final roomId =
          agent.latest?.roomId ??
          (agent.rooms.isEmpty ? null : agent.rooms.first.roomId);
      final latestTs = agent.latest?.ts;
      final cached = _histories[agent.identityId];
      if (cached != null &&
          cached.roomId == roomId &&
          cached.latestTs == latestTs) {
        continue; // fresh — nothing advanced
      }
      if (roomId == null) {
        // No room to read history from — honest empty, no call.
        _histories[agent.identityId] = _AgentHistory(
          roomId: null,
          latestTs: latestTs,
          points: const [],
        );
        continue;
      }
      // Keep the previous points visible while the refetch is in flight.
      final entry = _AgentHistory(
        roomId: roomId,
        latestTs: latestTs,
        points: cached?.points,
      );
      _histories[agent.identityId] = entry;
      unawaited(_fetchHistory(agent.identityId, entry));
    }
    _histories.removeWhere((id, _) => !seen.contains(id));
  }

  Future<void> _fetchHistory(String identityId, _AgentHistory entry) async {
    List<HistoryPoint> points;
    try {
      points = await _client.agentHistory(
        roomId: entry.roomId!,
        identityId: identityId,
        limit: 40,
      );
    } catch (_) {
      points = const []; // history errors fall back to empty points
    }
    // Stale guard: a newer sync replaced this entry (or the store is gone).
    if (_disposed || !identical(_histories[identityId], entry)) return;
    entry.points = points;
    notifyListeners();
  }

  // -- teardown ------------------------------------------------------------------------

  @override
  void dispose() {
    _disposed = true;
    _poll?.cancel();
    _debounce?.cancel();
    _pushSub?.cancel();
    super.dispose();
  }
}
