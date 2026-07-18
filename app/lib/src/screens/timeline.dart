/// Timeline (chat log) — ported from ui/src/components/Timeline.tsx per
/// phase3-features.json "Timeline": merged events + pending messages sorted
/// by ts, local-tz day dividers, 5-minute same-sender grouping (compact
/// rows), side rules (own/remote/system), message bubbles, agent work cards,
/// file tiles (FetchControl/FetchDetail), pipe tiles, syslines, static
/// skeleton loading rows (no shimmer — honest "still fetching"), empty
/// state, stick-to-bottom (140px threshold) + '{n} new message(s)' pill, and
/// pending status lines (Sending... / Sent locally, syncing... / Couldn't
/// send + Retry).
///
/// Reconnect anchoring (#68): a reconnect re-opens the room into a fresh store,
/// so the timeline collapses to empty and refills with the resynced backlog. A
/// reader at the bottom stays pinned to the newest event; a reader in history
/// keeps their exact position (numeric offset for the wholesale reload; a
/// captured getOffsetToReveal anchor for a live splice above the viewport, as
/// Flutter has no native scroll anchoring), and the pill counts only genuinely
/// new events past a reload baseline — never the whole reloaded backlog.
///
/// Data comes from `SessionScope.of(context).room`; the shell keys this
/// widget by roomId and wraps it in a ListenableBuilder on the RoomStore, so
/// scroll/live-region state resets on room switch and rebuilds ride the
/// store's notifications.
library;

import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart' show RenderAbstractViewport;
import 'package:jeliya_protocol/jeliya_protocol.dart'
    show
        ActivityCategories,
        FetchState,
        FileEntry,
        FileRef,
        LabelTone,
        PendingMessage,
        PendingPhases,
        PipeRef,
        Roles,
        TimelineEvent,
        TimelineKinds,
        TimelineRun,
        activityBreakdown,
        groupRuns,
        isAllMessages,
        labelTone,
        matchesActivityFilter,
        runSummary,
        shortId;

import '../format.dart';
import '../l10n/strings_context.dart';
import '../l10n/tokens.dart';
import '../l10n/wire_display.dart';
import '../layout.dart';
import '../session/daemon_session.dart';
import '../session/room_store.dart';
import '../theme.dart';
import '../widgets/avatar.dart';
import '../widgets/buttons.dart';
import '../widgets/fetch_control.dart';
import '../widgets/focus_ring.dart';
import '../widgets/progress_bar.dart';
import '../widgets/sender_name.dart';
import '../widgets/template_text.dart';
import '../widgets/text_action.dart';

// Display formatting (context.formats: bytes / clock / dayLabel / percent,
// plus top-level prettyLabel / extOf) lives in ../format.dart — the shared
// single home.

// -- item / row models -------------------------------------------------------------

/// Grouping window: consecutive messages by the same sender within 5 minutes
/// render compact (no avatar/meta).
const int _groupWindowMs = 5 * 60 * 1000;

enum _Side { own, remote, system }

/// One merged timeline entry: a wire event OR an optimistic pending message.
class _Item {
  const _Item.ofEvent(TimelineEvent this.event) : pendingMsg = null;
  const _Item.ofPending(PendingMessage this.pendingMsg) : event = null;

  final TimelineEvent? event;
  final PendingMessage? pendingMsg;

  int get ts => event?.ts ?? pendingMsg!.ts;
}

/// What actually gets a timeline row once folding + filtering are layered on: a
/// standalone [event] (any kind, incl. a lone agent-status), a folded
/// agent-status [run], or an optimistic [pendingMsg]. Runs and the activity
/// filter are a VIEW over the raw event list — the counter and the scroll/anchor
/// accounting keep counting the unfolded, unfiltered `_Item`s, so folding and
/// filtering never rewrite history (issue #65). Mirrors ui RenderUnit.
class _Unit {
  const _Unit.event(TimelineEvent this.event)
      : run = null,
        pendingMsg = null;
  const _Unit.run(TimelineRun this.run)
      : event = null,
        pendingMsg = null;
  const _Unit.pending(PendingMessage this.pendingMsg)
      : event = null,
        run = null;

  final TimelineEvent? event;
  final TimelineRun? run;
  final PendingMessage? pendingMsg;

  /// A run sorts by its latest (last) status; an event/pending by its own ts.
  int get ts =>
      run != null ? run!.events.last.ts : (event?.ts ?? pendingMsg!.ts);
}

/// The message sender used for 5-minute compacting — null for runs and every
/// non-message event, so neither ever groups (or lets a neighbour group into
/// it), exactly as the pre-fold loop behaved.
String? _unitMessageSender(_Unit u, String? selfId) {
  if (u.pendingMsg != null) return selfId;
  if (u.run != null) return null;
  final e = u.event!;
  return e.kind == TimelineKinds.message ? e.sender.identityId : null;
}

bool _shouldGroupUnit(_Unit? prev, _Unit u, String? selfId) {
  if (prev == null) return false;
  final sender = _unitMessageSender(u, selfId);
  final prevSender = _unitMessageSender(prev, selfId);
  if (sender == null || prevSender == null || sender != prevSender) return false;
  return u.ts - prev.ts <= _groupWindowMs;
}

const Set<String> _sidedKinds = {
  TimelineKinds.message,
  TimelineKinds.agentStatus,
  TimelineKinds.fileShared,
  TimelineKinds.pipeOpened,
};

_Side _unitSide(_Unit u, String? selfId) {
  if (u.pendingMsg != null) return _Side.own;
  if (u.run != null) {
    return selfId != null && u.run!.senderId == selfId
        ? _Side.own
        : _Side.remote;
  }
  final e = u.event!;
  if (!_sidedKinds.contains(e.kind)) return _Side.system;
  return selfId != null && e.sender.identityId == selfId
      ? _Side.own
      : _Side.remote;
}

/// Fold the timeline into render units: filter events to the active categories
/// (an empty set means everything passes), collapse maximal same-sender
/// agent-status runs via the shared [groupRuns], then merge the always-shown
/// pending messages back in by timestamp. Pending are NEVER filtered — retry
/// must stay reachable (issue #65 AC). Mirrors ui buildRenderUnits.
List<_Unit> _buildRenderUnits(
  List<TimelineEvent> events,
  List<PendingMessage> pending,
  Set<String> active,
) {
  final visible = active.isEmpty
      ? events
      : [for (final e in events) if (matchesActivityFilter(e.kind, active)) e];
  final rows = groupRuns(visible);
  final sortedPending = [...pending]..sort((a, b) => a.ts.compareTo(b.ts));
  final units = <_Unit>[];
  var pi = 0;
  for (final row in rows) {
    final ts = row.isRun ? row.run!.events.last.ts : row.event!.ts;
    while (pi < sortedPending.length && sortedPending[pi].ts <= ts) {
      units.add(_Unit.pending(sortedPending[pi++]));
    }
    units.add(row.isRun ? _Unit.run(row.run!) : _Unit.event(row.event!));
  }
  while (pi < sortedPending.length) {
    units.add(_Unit.pending(sortedPending[pi++]));
  }
  return units;
}

/// The five activity-filter categories in the shared contract's order, paired
/// with their display labels. Labels are display-only; the category KEYS drive
/// [matchesActivityFilter].
List<(String, String)> _activityCategories(AppStrings s) => [
      (ActivityCategories.conversation, s.timelineFilterConversation),
      (ActivityCategories.agentRuns, s.timelineFilterAgentRuns),
      (ActivityCategories.membership, s.timelineFilterMembership),
      (ActivityCategories.files, s.timelineFilterFiles),
      (ActivityCategories.pipes, s.timelineFilterPipes),
    ];

/// One render row: a day divider or an item, with its resolved side/compact
/// flags and the vertical rhythm (the web's collapsed margins + flex gap).
class _Row {
  const _Row.divider(String this.dividerLabel, {required this.topSpacing})
      : unit = null,
        side = _Side.system,
        compact = false;

  const _Row.unit(
    _Unit this.unit, {
    required this.side,
    required this.compact,
    required this.topSpacing,
  }) : dividerLabel = null;

  final String? dividerLabel;
  final _Unit? unit;
  final _Side side;
  final bool compact;
  final double topSpacing;
}

// Vertical rhythm, matching the web's 4px flex gap + collapsed 5px margins:
// normal 14 (5+4+5), own↔remote switch 21 (5+4+12), compact 7 (5+4-2),
// around a day divider 17 (5+4+8).
const double _gapNormal = 14;
const double _gapSideSwitch = 21;
const double _gapCompact = 7;
const double _gapDivider = 17;

/// Stick-to-bottom threshold (px from the bottom).
const double _stickThresholdPx = 140;

/// Prose caps (the web's 72ch bubble / 78ch agent-card limits at 13–14px).
const double _bubbleMaxWidth = 520;
const double _agentCardMaxWidth = 620;

class TimelineView extends StatefulWidget {
  const TimelineView({
    super.key,
    required this.onShowPipes,
    required this.onShowFiles,
  });

  /// 'Open in Pipes' on pipe tiles — deep-links into the Pipes tool AND the
  /// pipe the tile refers to (its id, or null when the tile has none), the
  /// route carrying both which tool and which item (#67).
  final ValueChanged<String?> onShowPipes;

  /// 'Open in Files' on shared-file tiles — the symmetric deep link into the
  /// Files tool and the file the tile shared (#67).
  final ValueChanged<String> onShowFiles;

  @override
  State<TimelineView> createState() => _TimelineViewState();
}

class _TimelineViewState extends State<TimelineView> {
  final ScrollController _controller = ScrollController();

  /// True while the reader is within [_stickThresholdPx] of the bottom — new
  /// content then jumps into view; scrolled up, it feeds the pill instead.
  bool _stick = true;

  /// Item count and tail identity from the last processed build, so the next
  /// change is classified as an append (new tail) vs a splice above the
  /// viewport (out-of-order / backlog insert).
  int _prevCount = 0;
  String? _prevTailId;
  bool _pendingAppended = false;
  int _pendingGrew = 0;

  /// The reader's last deliberate scroll offset (from [_onScroll]); the anchor
  /// restored after a wholesale reload collapses and refills the list.
  double _lastOffset = 0;

  int _newItemCount = 0;
  double _viewportDim = 0;

  /// Which agent runs the reader has expanded, keyed by the run's FIRST event id
  /// (stable as a run grows with live status updates). View-local + per-room:
  /// this TimelineView is keyed by roomId, so a room switch resets it — folding
  /// is a view, never a mutation of the signed log (issue #65).
  final Set<String> _expandedRuns = <String>{};

  // -- reload / anchor bookkeeping ----------------------------------------------
  //
  // A reconnect re-opens the room into a fresh RoomStore, so the timeline
  // collapses to empty (skeleton) and then refills with the resynced backlog.
  // Across that churn the reader's position and an honest "new" count must
  // survive — the fix for the position shifting under a reader (#68).

  /// Set when a reader-in-history reload is in flight: restore [_restoreOffset]
  /// once the backlog lands (not before — the empty frames would restore to 0).
  bool _restorePending = false;
  double _restoreOffset = 0;

  /// Seen-item baseline captured when the timeline empties, so the refilled
  /// backlog announces only genuinely-new events, never the whole reload.
  int? _reloadBaseline;

  /// Stable per-row [GlobalKey]s so the reader's anchor row can be located and
  /// measured (getOffsetToReveal) before and after a splice inserts events
  /// above it — Flutter has no native scroll anchoring, so we recreate it.
  final Map<String, GlobalKey> _rowKeys = {};

  /// The reader's anchor across a splice: the stable id of the row nearest the
  /// viewport top and its signed offset from that top, captured pre-layout.
  String? _anchorId;
  double _anchorDelta = 0;

  /// The current store, captured each build so the post-frame reconcile can
  /// read [RoomStore.loading] without another context lookup.
  RoomStore? _store;

  bool _reconcileScheduled = false;

  @override
  void initState() {
    super.initState();
    _controller.addListener(_onScroll);
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _onScroll() {
    if (!_controller.hasClients) return;
    // Ignore the churn while a reload empties then refills the list: the
    // transient 0-extent frames must not flip stick or corrupt the anchor.
    if (_restorePending) return;
    final pos = _controller.position;
    _lastOffset = pos.pixels;
    _stick = pos.maxScrollExtent - pos.pixels < _stickThresholdPx;
    // Scrolling back within the threshold clears the pill.
    if (_stick && _newItemCount != 0) setState(() => _newItemCount = 0);
  }

  /// Post-frame scroll reconciliation after the item set changed. Ported from
  /// Timeline.tsx's `syncScroll` + items effect: restore the reader's anchor
  /// across a reconnect reload, stick to the bottom, grow the pill on appends,
  /// or absorb a splice above the viewport so the first visible event holds its
  /// pixel offset. Reads the intent captured in build ([_restorePending],
  /// [_reloadBaseline], [_pendingAppended]/[_pendingGrew]) and the pre-layout
  /// anchor ([_captureAnchor]).
  void _applyScroll() {
    if (!mounted || !_controller.hasClients) return;
    final loading = _store?.loading ?? false;

    if (_restorePending) {
      // Wait for the resynced backlog: restoring against the empty/skeleton
      // frames would land at the top and drop the reader's anchor.
      if (loading || _prevCount == 0) return;
      final max = math.max(0.0, _controller.position.maxScrollExtent);
      _controller.jumpTo(math.min(_restoreOffset, max));
      _lastOffset = _controller.position.pixels;
      // Everything past what the reader had actually seen is "new" — announcing
      // the whole reloaded backlog would be a lie.
      final seen = _reloadBaseline ?? 0;
      final next = math.max(0, _prevCount - seen);
      _restorePending = false;
      _reloadBaseline = null;
      if (next != _newItemCount) setState(() => _newItemCount = next);
      return;
    }

    if (_stick) {
      _reloadBaseline = null;
      _jumpToBottom(3);
      if (_newItemCount != 0) setState(() => _newItemCount = 0);
      return;
    }

    // Reading history, a live change with no reload in flight.
    _reloadBaseline = null;
    if (_pendingAppended) {
      // New tail content the reader has not seen — count it, do not move.
      if (_pendingGrew > 0) setState(() => _newItemCount += _pendingGrew);
    } else {
      // A splice above the viewport (out-of-order / backlog insert): re-pin the
      // anchor captured pre-layout so the first visible event holds its pixel
      // offset. Nothing counts as new-at-bottom.
      _restoreAnchor();
    }
  }

  /// Record the reader's anchor: the built row nearest the viewport top and its
  /// signed offset from that top. Reads the CURRENT (pre-layout) geometry, so a
  /// splice landing this frame can be undone in [_restoreAnchor] post-layout.
  void _captureAnchor() {
    _anchorId = null;
    if (!_controller.hasClients) return;
    final pixels = _controller.position.pixels;
    var best = double.infinity;
    for (final entry in _rowKeys.entries) {
      final renderObject = entry.value.currentContext?.findRenderObject();
      if (renderObject == null || !renderObject.attached) continue;
      final reveal =
          RenderAbstractViewport.of(renderObject).getOffsetToReveal(renderObject, 0).offset;
      final delta = reveal - pixels;
      if (delta.abs() < best) {
        best = delta.abs();
        _anchorId = entry.key;
        _anchorDelta = delta;
      }
    }
  }

  /// Restore the [_captureAnchor] anchor: jump so that row sits [_anchorDelta]
  /// below the viewport top again, absorbing whatever height a splice inserted
  /// above it.
  void _restoreAnchor() {
    final id = _anchorId;
    if (id == null || !_controller.hasClients) return;
    final renderObject = _rowKeys[id]?.currentContext?.findRenderObject();
    if (renderObject == null || !renderObject.attached) return;
    final reveal =
        RenderAbstractViewport.of(renderObject).getOffsetToReveal(renderObject, 0).offset;
    final target = (reveal - _anchorDelta)
        .clamp(0.0, _controller.position.maxScrollExtent)
        .toDouble();
    if ((target - _controller.position.pixels).abs() > 0.5) {
      _controller.jumpTo(target);
      _lastOffset = _controller.position.pixels;
    }
  }

  /// jumpTo(maxScrollExtent), re-checking a few frames because builder list
  /// extents settle lazily.
  void _jumpToBottom(int retries) {
    if (!mounted || !_controller.hasClients) return;
    final pos = _controller.position;
    if (pos.maxScrollExtent > pos.pixels) _controller.jumpTo(pos.maxScrollExtent);
    if (retries <= 0) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_controller.hasClients) return;
      final p = _controller.position;
      if (p.maxScrollExtent - p.pixels > 1) _jumpToBottom(retries - 1);
    });
  }

  /// A shrinking viewport while stuck to the bottom — the soft keyboard's
  /// inset resizing the chat surface (or a window resize) — must keep the
  /// tail visible: the scroll offset is measured from the TOP, so shrinking
  /// silently pulls the latest messages under the composer/keyboard.
  void _onViewportChanged(ScrollMetrics metrics) {
    if (!metrics.hasViewportDimension) return;
    final dim = metrics.viewportDimension;
    final shrank = dim < _viewportDim;
    _viewportDim = dim;
    if (shrank && _stick) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _jumpToBottom(2));
    }
  }

  void _scrollToBottom() {
    if (!_controller.hasClients) return;
    _stick = true;
    setState(() => _newItemCount = 0);
    // Reduced motion is a contract, not a hint (the WCAG floor in CONTRIBUTING):
    // the jump to the newest event lands instantly instead of animating. Mirrors
    // ui Timeline.scrollToBottom's prefers-reduced-motion branch; the
    // keyboard-inset tail-follow (_onViewportChanged) is unaffected.
    if (mounted && MediaQuery.disableAnimationsOf(context)) {
      _jumpToBottom(2);
      return;
    }
    _controller
        .animateTo(
          _controller.position.maxScrollExtent,
          duration: const Duration(milliseconds: 260),
          curve: Curves.easeOut,
        )
        .whenComplete(() => _jumpToBottom(2));
  }

  @override
  Widget build(BuildContext context) {
    final session = SessionScope.of(context);
    final tokens = JeliyaTokens.of(context);
    final store = session.room;
    if (store == null) return const SizedBox.expand();

    final s = context.strings;
    final fmt = context.formats;
    final selfId = session.selfId;
    final loading = store.loading;

    // Merge events + pending messages, sorted ascending by ts (stable: ties
    // keep list order — events first, then pendings, like the web sort).
    final merged = <_Item>[
      for (final e in store.timeline) _Item.ofEvent(e),
      for (final p in store.pendingMessages) _Item.ofPending(p),
    ];
    final indexed = List<int>.generate(merged.length, (i) => i)
      ..sort((a, b) {
        final c = merged[a].ts.compareTo(merged[b].ts);
        return c != 0 ? c : a.compareTo(b);
      });
    final items = [for (final i in indexed) merged[i]];

    _store = store;

    // While reading history, snapshot the reader's anchor from the CURRENT
    // (pre-layout) frame, so a splice this build can restore it post-layout.
    if (!_restorePending && !_stick) _captureAnchor();

    final count = items.length;
    final tailId = items.isEmpty
        ? null
        : (items.last.event?.eventId ?? items.last.pendingMsg!.clientId);

    if (count != _prevCount) {
      final previous = _prevCount;
      // A splice above the viewport (out-of-order / backlog insert) leaves the
      // tail unchanged; only a new tail is an append that feeds the pill.
      _pendingAppended = tailId != _prevTailId;
      _pendingGrew = count - previous;
      _prevCount = count;
      _prevTailId = tailId;

      // A wholesale reload (reconnect re-open into a fresh store) empties the
      // timeline before it refills. The instant it empties, remember how much
      // the reader had truly seen and — if they were reading history — the spot
      // to restore, so neither the pill nor the position lies on refill.
      if (count == 0 && previous > 0) {
        _reloadBaseline = previous - _newItemCount;
        if (!_stick) {
          _restorePending = true;
          _restoreOffset = _lastOffset;
        }
      }

      if (!_reconcileScheduled) {
        _reconcileScheduled = true;
        WidgetsBinding.instance.addPostFrameCallback((_) {
          _reconcileScheduled = false;
          _applyScroll();
        });
      }
    }

    // The counter tells the truth about a mixed batch: it counts the SAME
    // unfolded, unfiltered new items as before (the `_newItemCount` items at the
    // bottom — exactly those below the reader), but words itself by what those
    // items actually are. Pending count as messages. Mirrors ui counterLabel.
    final newKinds = [
      for (final it in items.skip(math.max(0, items.length - _newItemCount)))
        it.event?.kind ?? TimelineKinds.message,
    ];
    final counterLabel = isAllMessages(activityBreakdown(newKinds))
        ? s.timelineNewMessages(_newItemCount)
        : s.timelineNewActivity(_newItemCount);

    // Fold + filter are a VIEW over the raw events: runs collapse, filtered
    // categories drop out, pending always stay. Built from the store's events
    // (already ts-ascending) — the count/tail/anchor accounting above still
    // rides the unfolded, unfiltered `items`, so folding/filtering never touch
    // history or the honest "new" count (issue #65).
    final activeFilters = session.activityFilters;
    final units = _buildRenderUnits(
        store.timeline, store.pendingMessages, activeFilters);
    final rows = _buildRows(fmt, units, selfId);
    // Each row carries a stable GlobalKey (below) so [_captureAnchor] can locate
    // and measure the reader's anchor row; prune keys for rows that left so the
    // map tracks only what is currently on screen.
    final rowIds = {for (final row in rows) _rowKey(row)};
    _rowKeys.removeWhere((id, _) => !rowIds.contains(id));

    Widget scroller;
    if (!loading && rows.isEmpty) {
      // Nothing to show: either the room is genuinely empty, or active filters
      // hid every event — history isn't gone, clearing the chips restores it, so
      // say which case honestly (issue #65).
      scroller = Center(
        child: Text(
          items.isEmpty ? s.timelineEmptyState : s.timelineNoActivityMatches,
          style: TextStyle(fontSize: 13.5, color: tokens.textDim),
        ),
      );
    } else {
      scroller = LayoutBuilder(
        builder: (context, constraints) {
          // The web caps rows at min(78%, 760px) of the scroller content box.
          final content =
              math.max(0.0, constraints.maxWidth - 2 * JeliyaSpacing.x24 - 4);
          final rowCap = math.min(content * 0.78, 760.0);
          final extra = loading ? 1 : 0;
          return NotificationListener<ScrollMetricsNotification>(
            onNotification: (notification) {
              _onViewportChanged(notification.metrics);
              return false;
            },
            child: ListView.builder(
              controller: _controller,
              padding: const EdgeInsets.symmetric(
                  horizontal: JeliyaSpacing.x24 + 2,
                  vertical: JeliyaSpacing.x18),
              itemCount: rows.length + extra,
              itemBuilder: (context, index) {
                if (loading && index == 0) return const _SkeletonRows();
                final row = rows[index - extra];
                final id = _rowKey(row);
                return Padding(
                  key: _rowKeys.putIfAbsent(id, GlobalKey.new),
                  padding: EdgeInsets.only(top: row.topSpacing),
                  child: _buildRow(context, row, store, selfId, rowCap),
                );
              },
            ),
          );
        },
      );
    }

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // View-only activity filters (issue #65): a strip ABOVE the scroller,
        // never inside the role="log" live region. Multi-select; none selected =
        // everything shows. Filtering never deletes history — clearing the chips
        // restores it — and pending messages are exempt entirely. Shown on BOTH
        // shells because each mounts this TimelineView.
        ActivityFilterStrip(
          categories: _activityCategories(s),
          active: activeFilters,
          onToggle: session.toggleActivityFilter,
          semanticsLabel: s.timelineFilterActivity,
        ),
        Expanded(
          // NAMED, but deliberately NOT a live region (issue #73). This node
          // used to carry `liveRegion: true` as a stand-in for the web's
          // role="log" — but the two are not equivalent. role="log" announces
          // only ADDED children; Flutter's `liveRegion` re-announces the node
          // itself on semantics update, and this node is a container over a
          // ListView that updates on every push, every scroll-anchor reconcile
          // and every filter toggle. On iOS that posts an announcement per
          // update, interrupting whatever the user was reading. The live region
          // now lives on the new-activity pill alone (see [_newMessagesPill]):
          // a small dedicated node whose label IS the delta, which is the part
          // role="log" would have announced. The scroller keeps its name so the
          // list still identifies itself on focus.
          child: Semantics(
            container: true,
            label: s.timelineRoomTimeline,
            child: Stack(
              children: [
                // SelectionArea holds on BOTH form factors. Evaluated for the
                // mobile release (2026-07-10): SelectableRegion gives touch only
                // a horizontal-drag recognizer + long-press, so vertical drags
                // scroll the list untouched; long-press selects and its native
                // toolbar already offers Copy (timeline_touch_selection_test.dart
                // and mobile_chat_route_test.dart pin both). Do NOT add another
                // long-press action here — a second recognizer would create the
                // selection-vs-scroll conflict this arrangement avoids.
                Positioned.fill(child: SelectionArea(child: scroller)),
                if (_newItemCount > 0)
                  Positioned(
                    left: 0,
                    right: 0,
                    bottom: JeliyaSpacing.x14,
                    child: Center(
                        child: _newMessagesPill(context, tokens, counterLabel)),
                  ),
              ],
            ),
          ),
        ),
      ],
    );
  }

  /// A stable identity for a render row: a divider keys on its (unique-per-day)
  /// label, an item on its signed `event_id` or optimistic `clientId`. Feeds the
  /// ListView row keys + [ListView.builder]'s `findChildIndexCallback`.
  String _rowKey(_Row row) {
    final unit = row.unit;
    if (unit == null) return 'div:${row.dividerLabel}';
    if (unit.run != null) return 'run:${unit.run!.events.first.eventId}';
    return 'it:${unit.event?.eventId ?? unit.pendingMsg!.clientId}';
  }

  List<_Row> _buildRows(JeliyaFormats fmt, List<_Unit> units, String? selfId) {
    final rows = <_Row>[];
    var lastDay = '';
    _Unit? prevUnit;
    _Side? prevSide;
    var afterDivider = false;
    for (final unit in units) {
      final day = fmt.dayLabel(unit.ts);
      if (day != lastDay) {
        lastDay = day;
        rows.add(_Row.divider(day,
            topSpacing: rows.isEmpty ? 0 : _gapDivider));
        afterDivider = true;
      }
      final compact = _shouldGroupUnit(prevUnit, unit, selfId);
      final side = _unitSide(unit, selfId);
      final double top;
      if (rows.isEmpty) {
        top = 0;
      } else if (afterDivider) {
        top = _gapDivider;
      } else if (compact) {
        top = _gapCompact;
      } else if ((side == _Side.own && prevSide == _Side.remote) ||
          (side == _Side.remote && prevSide == _Side.own)) {
        top = _gapSideSwitch;
      } else {
        top = _gapNormal;
      }
      rows.add(_Row.unit(unit, side: side, compact: compact, topSpacing: top));
      prevUnit = unit;
      prevSide = side;
      afterDivider = false;
    }
    return rows;
  }

  // -- row builders -------------------------------------------------------------

  Widget _buildRow(BuildContext context, _Row row, RoomStore store,
      String? selfId, double rowCap) {
    final s = context.strings;
    final fmt = context.formats;
    final divider = row.dividerLabel;
    if (divider != null) return _DayDivider(label: divider);
    final unit = row.unit!;
    final pending = unit.pendingMsg;
    if (pending != null) {
      return _alignRow(
        _Side.own,
        rowCap,
        _pendingCard(context, store, pending, row.compact),
      );
    }
    final run = unit.run;
    if (run != null) {
      return _eventCardRow(row.side, rowCap,
          avatarId: run.senderId,
          main: _runCardMain(context, store, run),
          mainMaxWidth: _agentCardMaxWidth);
    }
    final event = unit.event!;
    switch (event.kind) {
      case TimelineKinds.message:
        return _alignRow(
            row.side, rowCap, _messageRow(context, event, row.compact));
      case TimelineKinds.agentStatus:
        return _eventCardRow(row.side, rowCap,
            avatarId: event.sender.identityId,
            main: _agentCardMain(context, store, event),
            mainMaxWidth: _agentCardMaxWidth);
      case TimelineKinds.fileShared:
        if (event.file == null) return const SizedBox.shrink();
        return _eventCardRow(row.side, rowCap,
            avatarId: event.sender.identityId,
            main: _fileCardMain(context, store, event, selfId));
      case TimelineKinds.pipeOpened:
        if (event.pipe == null) return const SizedBox.shrink();
        return _eventCardRow(row.side, rowCap,
            avatarId: event.sender.identityId,
            main: _pipeCardMain(context, event));
      case TimelineKinds.roomCreated:
        return _sysline(
          context,
          s.timelineSyslineRoomCreated('{sender}', fmt.clock(event.ts)),
          {'sender': _nameSlot(context, event.sender.identityId)},
        );
      case TimelineKinds.memberInvited:
        final invitee = event.member?.identityId;
        return _sysline(
          context,
          s.timelineSyslineInvited(
              '{sender}',
              '{invitee}',
              s.roleInline(event.member?.role ?? Roles.member),
              fmt.clock(event.ts)),
          {
            'sender': _nameSlot(context, event.sender.identityId),
            'invitee': invitee != null
                ? _nameSlot(context, invitee)
                : TextSpan(text: s.timelineSomeone),
          },
        );
      case TimelineKinds.memberJoined:
        return _sysline(
          context,
          s.timelineSyslineJoined(
              '{who}',
              s.roleInline(event.member?.role ?? event.sender.role),
              fmt.clock(event.ts)),
          {
            'who': _nameSlot(
                context, event.member?.identityId ?? event.sender.identityId),
          },
        );
      case TimelineKinds.memberLeft:
        return _sysline(
          context,
          s.timelineSyslineLeft('{who}', fmt.clock(event.ts)),
          {
            'who': _nameSlot(
                context, event.member?.identityId ?? event.sender.identityId),
          },
        );
      case TimelineKinds.pipeClosed:
        final tokens = JeliyaTokens.of(context);
        return _sysline(
          context,
          s.timelineSyslinePipeClosed('{sender}', '{target}', fmt.clock(event.ts)),
          {
            'sender': _nameSlot(context, event.sender.identityId),
            'target': TextSpan(
                text: event.pipe?.target ?? '',
                style: JeliyaText.mono(fontSize: 12, color: tokens.textDim)),
          },
        );
      default:
        // Unknown event kinds render nothing (forward compat).
        return const SizedBox.shrink();
    }
  }

  Widget _alignRow(_Side side, double rowCap, Widget child) {
    return Align(
      alignment:
          side == _Side.own ? Alignment.centerRight : Alignment.centerLeft,
      child: ConstrainedBox(
        constraints: BoxConstraints(maxWidth: rowCap),
        child: child,
      ),
    );
  }

  /// Event-card anatomy: remote = avatar + full-width main column; own = no
  /// avatar, right-aligned and capped like message rows.
  Widget _eventCardRow(
    _Side side,
    double rowCap, {
    required String avatarId,
    required Widget main,
    double? mainMaxWidth,
  }) {
    final capped = mainMaxWidth == null
        ? main
        : ConstrainedBox(
            constraints: BoxConstraints(maxWidth: mainMaxWidth), child: main);
    if (side == _Side.own) {
      return Align(
        alignment: Alignment.centerRight,
        child: ConstrainedBox(
          constraints: BoxConstraints(maxWidth: rowCap),
          child: capped,
        ),
      );
    }
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Avatar(id: avatarId),
        const SizedBox(width: JeliyaSpacing.x12),
        Expanded(
          child: Align(alignment: Alignment.centerLeft, child: capped),
        ),
      ],
    );
  }

  // -- messages -------------------------------------------------------------------

  Widget _messageRow(BuildContext context, TimelineEvent event, bool compact) {
    final session = SessionScope.of(context);
    final own = session.isSelf(event.sender.identityId);
    final bubble = _bubble(context,
        body: event.body ?? '', own: own, compact: compact);
    final col = Column(
      crossAxisAlignment:
          own ? CrossAxisAlignment.end : CrossAxisAlignment.start,
      children: [
        if (!compact) ...[
          _metaRow(context, event, own: own),
          const SizedBox(height: JeliyaSpacing.x4),
        ],
        bubble,
      ],
    );
    if (own) return col;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (compact)
          const SizedBox(width: 34)
        else
          Avatar(id: event.sender.identityId),
        const SizedBox(width: JeliyaSpacing.x12),
        Flexible(child: col),
      ],
    );
  }

  /// Sender name + optional AGENT chip + time (msg-meta, 12px). The name is
  /// the flexible segment: at phone widths (long aliases, wide French copy)
  /// it truncates before the chip/time ever overflow.
  Widget _metaRow(BuildContext context, TimelineEvent event, {required bool own}) {
    return Row(
      mainAxisSize: MainAxisSize.min,
      mainAxisAlignment:
          own ? MainAxisAlignment.end : MainAxisAlignment.start,
      children: [
        Flexible(
          child: SenderName(
            id: event.sender.identityId,
            style: JeliyaText.name.copyWith(fontSize: 12),
          ),
        ),
        if (event.sender.role == Roles.agent) ...[
          const SizedBox(width: JeliyaSpacing.x8),
          const _AgentChip(),
        ],
        const SizedBox(width: JeliyaSpacing.x8),
        _time(context, event.ts),
      ],
    );
  }

  Widget _bubble(BuildContext context,
      {required String body,
      required bool own,
      required bool compact,
      bool dim = false}) {
    final tokens = JeliyaTokens.of(context);
    final radius = BorderRadius.only(
      topLeft: Radius.circular(own
          ? JeliyaRadii.bubble
          : compact
              ? 8
              : JeliyaRadii.bubbleSharp),
      topRight: Radius.circular(own
          ? (compact ? 8 : JeliyaRadii.bubbleSharp)
          : JeliyaRadii.bubble),
      bottomLeft: const Radius.circular(JeliyaRadii.bubble),
      bottomRight: const Radius.circular(JeliyaRadii.bubble),
    );
    // One anatomy for both sides (issue #75). Ownership reads FOUR ways —
    // right alignment, the suppressed avatar, the flipped tail radius above,
    // and the emerald edge here — which is plenty; the accent gradient and
    // resting shadow this replaces each broke a Named Rule in DESIGN.md (the
    // progress fill is the only sanctioned accent gradient; depth is tonal
    // layering plus 1px borders, so there are no resting shadows).
    //
    // Authorship reads from the SURFACE: the remote bubble sits one tonal step
    // below the card. The 2px blue border-left it replaces was the exact
    // side-stripe construct DESIGN.md forbids, and carrying it in Flutter cost
    // a ClipRRect + IntrinsicHeight + stretch Row to fake a non-uniform border
    // under a radius.
    return ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: _bubbleMaxWidth),
      child: Container(
        padding: const EdgeInsets.symmetric(
            horizontal: JeliyaSpacing.x14, vertical: JeliyaSpacing.x10),
        decoration: BoxDecoration(
          color: own ? tokens.bgCard : tokens.bubbleRemoteBg,
          border: Border.all(color: own ? tokens.accentLine : tokens.border),
          borderRadius: radius,
        ),
        child: Text(
          body,
          style:
              JeliyaText.body.copyWith(color: dim ? tokens.textDim : tokens.text),
        ),
      ),
    );
  }

  // -- pending message card ----------------------------------------------------------

  Widget _pendingCard(BuildContext context, RoomStore store,
      PendingMessage message, bool compact) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final failed = message.phase == PendingPhases.failed;
    final label = failed
        ? s.timelinePendingFailed
        : message.phase == PendingPhases.syncing
            ? s.timelinePendingSyncing
            : s.timelinePendingSending;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.end,
      children: [
        if (!compact) ...[
          Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(s.commonYou,
                  style: JeliyaText.name.copyWith(fontSize: 12)),
              const SizedBox(width: JeliyaSpacing.x8),
              _time(context, message.ts),
            ],
          ),
          const SizedBox(height: JeliyaSpacing.x4),
        ],
        _bubble(context,
            body: message.body, own: true, compact: compact, dim: true),
        const SizedBox(height: JeliyaSpacing.x4),
        Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (!failed) ...[
              _Spinner(color: tokens.textMute),
              const SizedBox(width: JeliyaSpacing.x6),
            ],
            // Flexible: the status line and Retry share one row, and Retry now
            // carries a 44dp target on touch — at 360px in French
            // ("Échec de l'envoi" + "Réessayer") the status text has to be able
            // to give way rather than overflow.
            Flexible(
              child: Text(label,
                  style: JeliyaText.meta,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis),
            ),
            if (failed) ...[
              const SizedBox(width: JeliyaSpacing.x6),
              JeliyaTextAction(
                label: s.commonRetry,
                // Several sends can fail in one timeline, so the visible
                // single word is not enough for a screen reader listing
                // actions — it would hear "Retry" repeated with nothing to
                // tell the copies apart.
                semanticLabel: s.timelineRetryMessage,
                onPressed: () => store.retryPendingMessage(message.clientId),
                style: TextStyle(
                  fontSize: 11.5,
                  fontWeight: FontWeight.w700,
                  color: tokens.accent,
                ),
              ),
            ],
          ],
        ),
      ],
    );
  }

  // -- agent work card ------------------------------------------------------------------

  Widget _agentCardMain(
      BuildContext context, RoomStore store, TimelineEvent event) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final fmt = context.formats;
    final label = event.label ?? s.timelineStatusFallback;
    final tone = labelTone(label);
    final pretty = prettyLabel(label);
    final progress = event.progress;
    final statusMessage = event.statusMessage;

    return Container(
      width: double.infinity,
      padding: const EdgeInsets.symmetric(
          horizontal: JeliyaSpacing.x12, vertical: JeliyaSpacing.x10),
      decoration: BoxDecoration(
        color: tokens.agentCardBg,
        border: Border.all(color: tokens.agentCardBorder),
        borderRadius: BorderRadius.circular(JeliyaRadii.row),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Identity left, status chip right — spaceBetween renders the
          // desktop Row+Spacer layout when one run fits, and at phone widths
          // the chip wraps to its own run instead of overflowing (the name
          // truncates first).
          Wrap(
            alignment: WrapAlignment.spaceBetween,
            crossAxisAlignment: WrapCrossAlignment.center,
            spacing: JeliyaSpacing.x8,
            runSpacing: JeliyaSpacing.x4,
            children: [
              Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Flexible(
                    child: SenderName(
                      id: event.sender.identityId,
                      style: JeliyaText.name.copyWith(fontSize: 13),
                    ),
                  ),
                  const SizedBox(width: JeliyaSpacing.x8),
                  const _AgentChip(),
                  const SizedBox(width: JeliyaSpacing.x8),
                  _time(context, event.ts),
                ],
              ),
              _LabelChip(tone: tone, text: pretty),
            ],
          ),
          const SizedBox(height: JeliyaSpacing.x8),
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(Tokens.agentWorkGlyph,
                  style: TextStyle(fontSize: 13.5, color: tokens.blue)),
              const SizedBox(width: JeliyaSpacing.x8),
              Flexible(
                child: Text(
                  pretty,
                  style: TextStyle(
                      fontSize: 13.5,
                      fontWeight: FontWeight.w600,
                      color: tokens.text),
                ),
              ),
            ],
          ),
          if (statusMessage != null)
            Padding(
              padding: const EdgeInsets.only(top: JeliyaSpacing.x6),
              child: Text(statusMessage,
                  style: TextStyle(fontSize: 13, color: tokens.text)),
            ),
          if (progress != null)
            Padding(
              padding: const EdgeInsets.only(top: JeliyaSpacing.x10),
              child: Row(
                children: [
                  Expanded(child: ProgressBar(value: progress.toDouble())),
                  const SizedBox(width: JeliyaSpacing.x10),
                  Text(
                    fmt.percent(progress.clamp(0, 100)),
                    style: JeliyaText.mono(fontSize: 11.5, color: tokens.textDim),
                  ),
                ],
              ),
            ),
          if (event.artifacts.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: JeliyaSpacing.x10),
              child: Wrap(
                spacing: JeliyaSpacing.x6,
                runSpacing: JeliyaSpacing.x6,
                children: [
                  for (final fileId in event.artifacts)
                    _artifactChip(context, store, fileId),
                ],
              ),
            ),
        ],
      ),
    );
  }

  Widget _artifactChip(BuildContext context, RoomStore store, String fileId) {
    final tokens = JeliyaTokens.of(context);
    final file = _fileById(store.files, fileId);
    return Tooltip(
      message: fileId,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
        decoration: BoxDecoration(
          color: tokens.bgCard2,
          border: Border.all(color: tokens.borderStrong),
          borderRadius: BorderRadius.circular(JeliyaRadii.pill),
        ),
        child: Text(
          '${Tokens.artifactGlyph} ${file?.name ?? shortId(fileId)}',
          style: TextStyle(fontSize: 11, color: tokens.textDim),
        ),
      ),
    );
  }

  // -- folded agent run (issue #65) ---------------------------------------------

  /// A folded run of ≥2 consecutive same-sender agent-status updates, rendered
  /// as ONE card: the LATEST signed status (the same [_agentCardMain] a lone
  /// status uses) + an honest evidence line ("N updates · first–last", real
  /// timestamps via [JeliyaFormats.clock]) + a disclosure. Expanded, it reveals
  /// every original update in order — history is only folded, never lost. The
  /// reveal is a compositor-only fade that reduced motion turns off (mirrors ui
  /// RunCard / .agent-run-history).
  Widget _runCardMain(BuildContext context, RoomStore store, TimelineRun run) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final fmt = context.formats;
    final summary = runSummary(run);
    final span = summary.firstTs == summary.lastTs
        ? fmt.clock(summary.firstTs)
        : '${fmt.clock(summary.firstTs)}–${fmt.clock(summary.lastTs)}';
    final key = run.events.first.eventId;
    final expanded = _expandedRuns.contains(key);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _agentCardMain(context, store, summary.latest),
        const SizedBox(height: JeliyaSpacing.x4),
        Row(
          children: [
            Flexible(
              child: Text(
                s.timelineRunEvidence(summary.count, span),
                style: TextStyle(fontSize: 12, color: tokens.textMute),
              ),
            ),
            const SizedBox(width: JeliyaSpacing.x10),
            JeliyaTextAction(
              expanded: expanded,
              label: expanded
                  ? s.timelineRunHide
                  : s.timelineRunShow(summary.count),
              style: TextStyle(
                fontSize: 12,
                fontWeight: FontWeight.w700,
                color: tokens.accent,
              ),
              onPressed: () => setState(() {
                if (!_expandedRuns.remove(key)) _expandedRuns.add(key);
              }),
            ),
          ],
        ),
        if (expanded) ...[
          const SizedBox(height: JeliyaSpacing.x8),
          _RunHistory(
            reduceMotion: MediaQuery.disableAnimationsOf(context),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                for (var i = 0; i < run.events.length; i++) ...[
                  if (i > 0) const SizedBox(height: JeliyaSpacing.x4),
                  _agentCardMain(context, store, run.events[i]),
                ],
              ],
            ),
          ),
        ],
      ],
    );
  }

  // -- file_shared card ---------------------------------------------------------------------

  Widget _fileCardMain(BuildContext context, RoomStore store,
      TimelineEvent event, String? selfId) {
    final s = context.strings;
    final file = event.file!;
    final own = selfId != null && event.sender.identityId == selfId;
    return Column(
      crossAxisAlignment:
          own ? CrossAxisAlignment.end : CrossAxisAlignment.start,
      children: [
        _eventHead(context, event, s.timelineSharedAFile),
        const SizedBox(height: JeliyaSpacing.x8),
        _fileTile(context, store, file, own: own),
      ],
    );
  }

  /// SenderName + optional AGENT chip + muted verb + time (event-head, 13px).
  Widget _eventHead(BuildContext context, TimelineEvent event, String verb) {
    final tokens = JeliyaTokens.of(context);
    return Wrap(
      spacing: JeliyaSpacing.x8,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        SenderName(
          id: event.sender.identityId,
          style: JeliyaText.name.copyWith(fontSize: 13),
        ),
        if (event.sender.role == Roles.agent) const _AgentChip(),
        Text(verb, style: TextStyle(fontSize: 13, color: tokens.textDim)),
        _time(context, event.ts),
      ],
    );
  }

  Widget _fileTile(BuildContext context, RoomStore store, FileRef file,
      {required bool own}) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final fmt = context.formats;
    final tint = tokens.fileTint(file.name);
    final ext = extOf(file.name).toUpperCase();
    final extLabel = ext.isEmpty
        ? s.commonFileExtFallback
        : ext.substring(0, math.min(4, ext.length));
    final entry = _fileById(store.files, file.fileId);
    final state = store.fetches[file.fileId];

    final tile = Container(
      padding: const EdgeInsets.symmetric(
          horizontal: JeliyaSpacing.x12, vertical: JeliyaSpacing.x10),
      decoration: BoxDecoration(
        color: own ? tokens.accent.withValues(alpha: 0.1) : tokens.bgCard2,
        border:
            Border.all(color: own ? tokens.ownTileBorder : tokens.borderStrong),
        borderRadius: BorderRadius.circular(JeliyaRadii.row),
      ),
      child: Row(
        children: [
          Container(
            width: 40,
            height: 40,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: tint.withAlpha(0x22), // 13%
              borderRadius: BorderRadius.circular(JeliyaRadii.btn),
            ),
            child: Text(
              extLabel,
              style: TextStyle(
                fontSize: 10,
                fontWeight: FontWeight.w800,
                letterSpacing: 0.4,
                color: tint,
              ),
            ),
          ),
          const SizedBox(width: JeliyaSpacing.x12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  file.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                      color: tokens.text),
                ),
                Text(
                  s.timelineFileMeta(
                      fmt.bytes(file.size), ext.isEmpty ? extLabel : ext),
                  style: TextStyle(fontSize: 12, color: tokens.textDim),
                ),
              ],
            ),
          ),
          const SizedBox(width: JeliyaSpacing.x12),
          // A Row hands its trailing (non-flex) child unbounded width, so a
          // wide control state ('No provider online' + Recheck) overflows a
          // phone-width tile; below the breakpoint the control shares the
          // row flexibly (its Wrap re-runs). Desktop keeps the intrinsic,
          // right-flushed control exactly as before.
          if (isMobileWidth(context))
            Flexible(child: _fileTileControl(store, file, entry, state, own))
          else
            _fileTileControl(store, file, entry, state, own),
        ],
      ),
    );

    // Deep link into the Files workspace on this file (#67), the counterpart
    // to the pipe tile's 'Open in Pipes'. The Files tool handles a target that
    // has not synced into file.list yet.
    final openInFiles = Padding(
      padding: const EdgeInsets.only(top: JeliyaSpacing.x8),
      child: JeliyaButton(
        label: s.timelineOpenInFiles,
        size: JeliyaButtonSize.sm,
        onPressed: () => widget.onShowFiles(file.fileId),
      ),
    );
    if (own) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [tile, openInFiles],
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [tile, FetchDetail(state: state), openInFiles],
    );
  }

  /// The tile's trailing affordance: own files serve, remote files fetch.
  Widget _fileTileControl(RoomStore store, FileRef file, FileEntry? entry,
      FetchState? state, bool own) {
    if (own) return const _ServingNote();
    return FetchControl(
      state: state,
      availability: entry == null
          ? null
          : FetchAvailability(
              available: entry.available, providers: entry.providers),
      availabilityPending: entry == null,
      onFetch: () {
        store.fetchFile(file.fileId);
      },
      onRecheck: () {
        store.refreshFiles();
      },
    );
  }

  FileEntry? _fileById(List<FileEntry> files, String fileId) {
    for (final f in files) {
      if (f.fileId == fileId) return f;
    }
    return null;
  }

  // -- pipe_opened card ------------------------------------------------------------------------

  Widget _pipeCardMain(BuildContext context, TimelineEvent event) {
    final s = context.strings;
    final own = SessionScope.of(context).isSelf(event.sender.identityId);
    return Column(
      crossAxisAlignment:
          own ? CrossAxisAlignment.end : CrossAxisAlignment.start,
      children: [
        _eventHead(context, event, s.timelineOpenedAPipe),
        const SizedBox(height: JeliyaSpacing.x8),
        _pipeTile(context, event.pipe!, own: own),
      ],
    );
  }

  Widget _pipeTile(BuildContext context, PipeRef pipe, {required bool own}) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final authorized = pipe.authorizedPeer;
    return Container(
      padding: const EdgeInsets.symmetric(
          horizontal: JeliyaSpacing.x12, vertical: JeliyaSpacing.x10),
      decoration: BoxDecoration(
        color: own ? tokens.accent.withValues(alpha: 0.1) : tokens.bgCard2,
        border:
            Border.all(color: own ? tokens.ownTileBorder : tokens.borderStrong),
        borderRadius: BorderRadius.circular(JeliyaRadii.row),
      ),
      child: Row(
        children: [
          Container(
            width: 34,
            height: 34,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: tokens.accentDim,
              borderRadius: BorderRadius.circular(JeliyaRadii.btn),
            ),
            child: Text(Tokens.pipeGlyph,
                style: TextStyle(fontSize: 17, color: tokens.accent)),
          ),
          const SizedBox(width: JeliyaSpacing.x12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  pipe.target ?? Tokens.emDash,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: JeliyaText.mono(
                      fontSize: 13,
                      color: tokens.text,
                      fontWeight: FontWeight.w600),
                ),
                Row(
                  children: [
                    // Every segment flexible: at the 960px minimum window this
                    // row gets too little width for inflexible text next to
                    // the fixed icon and button. The sentence stays ONE
                    // translatable template; segments come from its parts.
                    for (final part
                        in templateParts(s.timelineAuthorizedPeer('{peer}')))
                      if (part.slot == 'peer')
                        Flexible(
                          child: authorized != null
                              ? SenderName(
                                  id: authorized,
                                  style: JeliyaText.name.copyWith(fontSize: 12),
                                )
                              : Text(Tokens.emDash,
                                  style: TextStyle(
                                      fontSize: 12, color: tokens.textDim)),
                        )
                      else
                        Flexible(
                          // An unmatched slot renders its literal marker
                          // (fail-visible, matching templateSpans' contract).
                          child: Text(part.text ?? '{${part.slot}}',
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                  fontSize: 12, color: tokens.textDim)),
                        ),
                  ],
                ),
              ],
            ),
          ),
          const SizedBox(width: JeliyaSpacing.x12),
          // A Row hands its trailing (non-flex) child unbounded width, so the
          // fixed "Open in Pipes" button overruns a phone-width tile — the
          // same defect _fileTile's control was already given a Flexible for.
          // Surfaced by the compact room hiding the bottom bar (buying back
          // ~72dp of timeline), which scrolls this pipe card into view where
          // the shorter old room never reached it. Desktop keeps the
          // intrinsic, right-flushed button.
          if (isMobileWidth(context))
            Flexible(
              child: JeliyaButton(
                label: s.timelineOpenInPipes,
                size: JeliyaButtonSize.sm,
                onPressed: () => widget.onShowPipes(pipe.pipeId),
              ),
            )
          else
            JeliyaButton(
              label: s.timelineOpenInPipes,
              size: JeliyaButtonSize.sm,
              onPressed: () => widget.onShowPipes(pipe.pipeId),
            ),
        ],
      ),
    );
  }

  // -- syslines ------------------------------------------------------------------------------------

  /// One full-sentence template per sysline; names/targets are injected as
  /// spans so translations can reorder them freely.
  Widget _sysline(
      BuildContext context, String template, Map<String, InlineSpan> slots) {
    return SizedBox(
      width: double.infinity,
      child: templateText(
        template,
        slots: slots,
        style: JeliyaText.sysline,
        textAlign: TextAlign.center,
      ),
    );
  }

  InlineSpan _nameSlot(BuildContext context, String id) {
    final tokens = JeliyaTokens.of(context);
    return widgetSlot(SenderName(
      id: id,
      style: TextStyle(
          fontSize: 12, fontWeight: FontWeight.w600, color: tokens.textDim),
    ));
  }

  // -- small pieces ------------------------------------------------------------------------------------

  Widget _time(BuildContext context, int ts) =>
      Text(context.formats.clock(ts), style: JeliyaText.meta);

  /// The new-activity pill — and the timeline's ONLY live region (issue #73).
  ///
  /// It floats in a `Positioned`/`Center` over the scroller, so unlike the
  /// filter strip it has no constant-height budget to protect: the 44dp touch
  /// floor costs the layout nothing. `minimumSize: Size.zero` +
  /// `shrinkWrap` had it at roughly 32dp.
  ///
  /// The live region sits here rather than on the scroller because this node is
  /// exactly the delta: it exists only while unseen items are below the reader,
  /// and its label already states how many and of what kind. It is a small leaf
  /// whose label changes only when the count does, so assistive tech hears the
  /// delta once per change instead of on every list rebuild.
  ///
  /// It separates from the timeline beneath it by its own tonal step
  /// ([JeliyaTokens.bgCard2]) and its accent border, NOT by a resting shadow
  /// (DESIGN.md section 4: flat by doctrine). Matches `.new-messages` in
  /// styles.css after the #75 conformance pass.
  Widget _newMessagesPill(
      BuildContext context, JeliyaTokens tokens, String label) {
    final touch = isMobileWidth(context);
    final pill = TextButton(
      onPressed: _scrollToBottom,
      style: TextButton.styleFrom(
        backgroundColor: tokens.bgCard2,
        foregroundColor: tokens.accent,
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
        minimumSize: touch ? const Size(44, 44) : Size.zero,
        tapTargetSize: MaterialTapTargetSize.shrinkWrap,
        textStyle: const TextStyle(fontSize: 12.5, fontWeight: FontWeight.w700),
        shape: StadiumBorder(side: BorderSide(color: tokens.accentLine)),
      ),
      child: Text(label),
    );
    // The labelled node carries the tap itself (the room_header lesson): the
    // TextButton's own semantics are excluded so the announcement is the count
    // alone, once, on a node a screen reader can also activate.
    return Semantics(
      container: true,
      liveRegion: true,
      button: true,
      label: label,
      onTap: _scrollToBottom,
      child: ExcludeSemantics(
        child: JeliyaFocusRing(
          borderRadius: BorderRadius.circular(JeliyaRadii.pill),
          child: pill,
        ),
      ),
    );
  }
}

/// The quiet 9.5px uppercase AGENT role chip (P3: agents are peers; their
/// role is a whisper, their work is the structured card).
class _AgentChip extends StatelessWidget {
  const _AgentChip();

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
      decoration: BoxDecoration(
        border: Border.all(color: tokens.borderStrong),
        borderRadius: BorderRadius.circular(JeliyaRadii.pill),
      ),
      child: Text(
        s.timelineAgentChip,
        style: TextStyle(
          fontSize: 9.5,
          fontWeight: FontWeight.w700,
          letterSpacing: 0.76, // 0.08em
          color: tokens.textMute,
        ),
      ),
    );
  }
}

/// Agent-status label chip: tone-tinted; neutral stays quiet (green is
/// earned — P4).
class _LabelChip extends StatelessWidget {
  const _LabelChip({required this.tone, required this.text});

  final LabelTone tone;
  final String text;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
      decoration: BoxDecoration(
        color: tokens.toneBg(tone),
        border: Border.all(color: tokens.toneBorder(tone)),
        borderRadius: BorderRadius.circular(JeliyaRadii.pill),
      ),
      child: Text(
        text,
        style: TextStyle(
          fontSize: 11,
          letterSpacing: 0.22, // 0.02em
          color: tone == LabelTone.neutral
              ? tokens.textDim
              : tokens.toneColor(tone),
        ),
      ),
    );
  }
}

// The run disclosure and the pending Retry were each a local `Semantics` over a
// bare `GestureDetector` — announceable but not activatable by assistive tech,
// unreachable by keyboard, and roughly 35x16. Both are now the one shared
// [JeliyaTextAction] primitive (issue #73), which keeps the aria-expanded state
// the disclosure needs.

/// The expanded run's history: every original agent-status card behind a 2px
/// blue rule, revealed with a compositor-only fade + slide that reduced motion
/// turns off (mirrors ui .agent-run-history / prefers-reduced-motion). The
/// HEIGHT change is always instant — the cards are real the moment they expand —
/// so this never fights the scroll-anchor reconcile.
class _RunHistory extends StatelessWidget {
  const _RunHistory({required this.reduceMotion, required this.child});

  final bool reduceMotion;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final body = Container(
      padding: const EdgeInsets.only(left: JeliyaSpacing.x14),
      decoration: BoxDecoration(
        border: Border(left: BorderSide(color: tokens.blueLine, width: 2)),
      ),
      child: child,
    );
    if (reduceMotion) return body;
    return TweenAnimationBuilder<double>(
      tween: Tween<double>(begin: 0, end: 1),
      duration: const Duration(milliseconds: 180),
      curve: Curves.easeOut,
      builder: (context, value, child) => Opacity(
        opacity: value,
        child: Transform.translate(
          offset: Offset(0, -4 * (1 - value)),
          child: child,
        ),
      ),
      child: body,
    );
  }
}

/// The view-only activity filter strip above the timeline (issue #65): five
/// multi-select chips, none selected = everything shows. Kept OUTSIDE the
/// role="log" live region (a sibling above it), mirroring ui .activity-filter.
///
/// A SINGLE, constant-height row (each chip an equal share of the width, its
/// label truncating rather than wrapping) — exactly the shape the room-list
/// lifecycle filter uses. This keeps the strip's height a constant so it can't
/// erode the compact timeline budget under a keyboard inset (the reason ui makes
/// its strip a constant-height row), and adds NO second Scrollable — the one
/// vertical Scrollable in this subtree stays the timeline list.
class ActivityFilterStrip extends StatelessWidget {
  const ActivityFilterStrip({
    super.key,
    required this.categories,
    required this.active,
    required this.onToggle,
    required this.semanticsLabel,
  });

  final List<(String, String)> categories;
  final Set<String> active;
  final ValueChanged<String> onToggle;
  final String semanticsLabel;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    return Semantics(
      container: true,
      label: semanticsLabel,
      child: Container(
        decoration: BoxDecoration(
          border: Border(bottom: BorderSide(color: tokens.border)),
        ),
        // A tight, CONSTANT-height strip: it must clear the compact keyboard-inset
        // budget and stay under the timeline's touch gutter so a drag started just
        // inside the list still scrolls (mobile_chat_route_test), never landing on
        // a chip.
        //
        // On touch the chips have to clear the 44dp floor (issue #73), and the
        // strip pays for as little of that as it can: its own vertical padding
        // goes to zero and the chip's 44dp box supplies the breathing room, so
        // the strip grows 35 -> 45dp rather than 35 -> 53dp. Desktop keeps the
        // dense 26dp row — the floor is a touch/compact contract (DESIGN.md).
        padding: EdgeInsets.symmetric(
            horizontal: JeliyaSpacing.x24 + 2,
            vertical: isMobileWidth(context) ? 0 : 4),
        child: Row(
          children: [
            for (var i = 0; i < categories.length; i++) ...[
              if (i > 0) const SizedBox(width: JeliyaSpacing.x6),
              Expanded(
                child: _ActivityChip(
                  label: categories[i].$2,
                  active: active.contains(categories[i].$1),
                  onTap: () => onToggle(categories[i].$1),
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// One activity-filter chip: tinted accent when active (aria-pressed/selected),
/// muted otherwise; its label truncates rather than overflowing the shared row.
/// Mirrors ui .activity-chip.
///
/// Semantics and keyboard were never the problem here — this is a real `InkWell`
/// under `Semantics(button: true, selected: active)`, so it focuses, takes Enter
/// and Space, and announces its pressed state. Two things were missing (issue
/// #73): a visible focus indicator, and SIZE — the pill measured 26dp against a
/// 44dp floor. The 44dp box is the TARGET, not the pill: the visual chip keeps
/// its 26dp height and centres inside it, so the strip reads exactly as before
/// while the whole box is tappable. Desktop is untouched.
class _ActivityChip extends StatelessWidget {
  const _ActivityChip(
      {required this.label, required this.active, required this.onTap});

  final String label;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final touch = isMobileWidth(context);
    final pill = Container(
      alignment: Alignment.center,
      padding: const EdgeInsets.symmetric(
          horizontal: JeliyaSpacing.x8, vertical: 3),
      decoration: BoxDecoration(
        color: active ? tokens.accentDim : Colors.transparent,
        borderRadius: BorderRadius.circular(JeliyaRadii.pill),
        // The chip's border is the only edge that says "this is a control", so
        // the inactive state takes the 3:1 interactive boundary. Active keeps
        // the accent line, which already clears it.
        border: Border.all(
            color: active ? tokens.accentLine : tokens.borderInteractive),
      ),
      child: Text(
        label,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: TextStyle(
          fontSize: 12,
          color: active ? tokens.accent : tokens.textMute,
          fontWeight: active ? FontWeight.w600 : FontWeight.w400,
        ),
      ),
    );
    return Semantics(
      button: true,
      selected: active,
      child: JeliyaFocusRing(
        borderRadius: BorderRadius.circular(JeliyaRadii.pill),
        child: Material(
          color: Colors.transparent,
          child: InkWell(
            onTap: onTap,
            borderRadius: BorderRadius.circular(JeliyaRadii.pill),
            overlayColor: jeliyaOverlay(tokens),
            child: touch
                ? SizedBox(height: 44, child: Center(child: pill))
                : pill,
          ),
        ),
      ),
    );
  }
}

/// Self-owned file note (in place of a fetch control) — the daemon reports
/// own files unavailable, so ownership renders 'Serving', never a misleading
/// 'No provider online'.
class _ServingNote extends StatelessWidget {
  const _ServingNote();

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    return Tooltip(
      message: s.commonServingTooltip,
      child: Container(
        constraints: const BoxConstraints(minHeight: 28),
        padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: tokens.bgCard2,
          border: Border.all(color: tokens.borderStrong),
          borderRadius: BorderRadius.circular(JeliyaRadii.pill),
        ),
        child: Text(s.commonServing,
            style: TextStyle(fontSize: 12, color: tokens.textDim)),
      ),
    );
  }
}

/// 11px in-flight spinner; a static 0.7-opacity dot under reduced motion.
class _Spinner extends StatelessWidget {
  const _Spinner({required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) {
    if (MediaQuery.of(context).disableAnimations) {
      return Container(
        width: 7,
        height: 7,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: color.withValues(alpha: 0.7),
        ),
      );
    }
    return SizedBox(
      width: 11,
      height: 11,
      child: CircularProgressIndicator(strokeWidth: 2, color: color),
    );
  }
}

/// Static skeleton rows while `room.open` is in flight — tonal bars, NO
/// shimmer (honest "still fetching").
class _SkeletonRows extends StatelessWidget {
  const _SkeletonRows();

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    Widget line(double widthFactor) => FractionallySizedBox(
          alignment: Alignment.centerLeft,
          widthFactor: widthFactor,
          child: Container(
            height: 12,
            decoration: BoxDecoration(
              color: tokens.bgCard2,
              borderRadius: BorderRadius.circular(JeliyaRadii.iconBtn),
            ),
          ),
        );
    Widget row(List<double> widths) => Padding(
          padding: const EdgeInsets.only(bottom: JeliyaSpacing.x14),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Container(
                width: 34,
                height: 34,
                decoration: BoxDecoration(
                  color: tokens.bgCard2,
                  borderRadius: BorderRadius.circular(JeliyaRadii.btn),
                ),
              ),
              const SizedBox(width: JeliyaSpacing.x12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    for (final w in widths) ...[
                      line(w),
                      const SizedBox(height: JeliyaSpacing.x6),
                    ],
                  ],
                ),
              ),
            ],
          ),
        );
    return ExcludeSemantics(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          row(const [0.32, 0.74]),
          row(const [0.24, 0.86, 0.58]),
          row(const [0.40, 0.66]),
        ],
      ),
    );
  }
}

/// Centered day-divider pill ('Today' / 'Yesterday' / 'MMM d, yyyy').
class _DayDivider extends StatelessWidget {
  const _DayDivider({required this.label});

  final String label;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    return Center(
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 3),
        decoration: BoxDecoration(
          color: tokens.bgRaise,
          border: Border.all(color: tokens.borderStrong),
          borderRadius: BorderRadius.circular(JeliyaRadii.pill),
        ),
        child: Text(label, style: TextStyle(fontSize: 12, color: tokens.textDim)),
      ),
    );
  }
}
