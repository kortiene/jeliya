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
/// Data comes from `SessionScope.of(context).room`; the shell keys this
/// widget by roomId and wraps it in a ListenableBuilder on the RoomStore, so
/// scroll/live-region state resets on room switch and rebuilds ride the
/// store's notifications.
library;

import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart'
    show
        FileEntry,
        FileRef,
        LabelTone,
        PendingMessage,
        PendingPhases,
        PipeRef,
        Roles,
        TimelineEvent,
        TimelineKinds,
        labelTone,
        shortId;

import '../format.dart';
import '../l10n/strings_context.dart';
import '../l10n/tokens.dart';
import '../l10n/wire_display.dart';
import '../session/daemon_session.dart';
import '../session/room_store.dart';
import '../theme.dart';
import '../widgets/avatar.dart';
import '../widgets/buttons.dart';
import '../widgets/fetch_control.dart';
import '../widgets/progress_bar.dart';
import '../widgets/sender_name.dart';
import '../widgets/template_text.dart';

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

/// Grouping identity: pending items are attributed to self; only `message`
/// events group (event cards/syslines never do).
String? _itemSender(_Item item, String? selfId) {
  if (item.pendingMsg != null) return selfId;
  final e = item.event!;
  return e.kind == TimelineKinds.message ? e.sender.identityId : null;
}

bool _shouldGroup(_Item? prev, _Item item, String? selfId) {
  if (prev == null) return false;
  final sender = _itemSender(item, selfId);
  final prevSender = _itemSender(prev, selfId);
  if (sender == null || prevSender == null || sender != prevSender) return false;
  return item.ts - prev.ts <= _groupWindowMs;
}

const Set<String> _sidedKinds = {
  TimelineKinds.message,
  TimelineKinds.agentStatus,
  TimelineKinds.fileShared,
  TimelineKinds.pipeOpened,
};

_Side _itemSide(_Item item, String? selfId) {
  if (item.pendingMsg != null) return _Side.own;
  final e = item.event!;
  if (!_sidedKinds.contains(e.kind)) return _Side.system;
  return selfId != null && e.sender.identityId == selfId
      ? _Side.own
      : _Side.remote;
}

/// One render row: a day divider or an item, with its resolved side/compact
/// flags and the vertical rhythm (the web's collapsed margins + flex gap).
class _Row {
  const _Row.divider(String this.dividerLabel, {required this.topSpacing})
      : item = null,
        side = _Side.system,
        compact = false;

  const _Row.item(
    _Item this.item, {
    required this.side,
    required this.compact,
    required this.topSpacing,
  }) : dividerLabel = null;

  final String? dividerLabel;
  final _Item? item;
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
  const TimelineView({super.key, required this.onShowPipes});

  /// 'Open in Pipes' on pipe tiles — switches the right panel tab.
  final VoidCallback onShowPipes;

  @override
  State<TimelineView> createState() => _TimelineViewState();
}

class _TimelineViewState extends State<TimelineView> {
  final ScrollController _controller = ScrollController();
  bool _stick = true;
  int _lastCount = 0;
  String? _lastTailId;
  int _newItemCount = 0;
  double _viewportDim = 0;

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
    final pos = _controller.position;
    _stick = pos.maxScrollExtent - pos.pixels < _stickThresholdPx;
    // Scrolling back within the threshold clears the pill.
    if (_stick && _newItemCount != 0) setState(() => _newItemCount = 0);
  }

  /// On item-count change: stick → jump to bottom and reset the counter;
  /// scrolled up → accumulate the delta into the pill.
  void _afterItemsChanged(int delta) {
    if (!mounted) return;
    if (_stick) {
      _jumpToBottom(3);
      if (_newItemCount != 0) setState(() => _newItemCount = 0);
    } else if (delta > 0) {
      setState(() => _newItemCount += delta);
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

    if (items.length != _lastCount) {
      final delta = items.length - _lastCount;
      // A late-backlog SPLICE (reconnected peer's older events insert above)
      // leaves the tail item unchanged — it must not inflate the 'new
      // messages' pill, which promises new content at the BOTTOM. Known
      // deviation from the web: the browser's native scroll anchoring also
      // preserves the reading position across such splices; Flutter has no
      // equivalent without a custom RenderSliver, so a splice above the
      // viewport still shifts the scroll position here.
      final tailId = items.isEmpty
          ? null
          : (items.last.event?.eventId ?? items.last.pendingMsg!.clientId);
      final appended = tailId != _lastTailId;
      _lastCount = items.length;
      _lastTailId = tailId;
      WidgetsBinding.instance
          .addPostFrameCallback((_) => _afterItemsChanged(appended ? delta : 0));
    }

    final rows = _buildRows(fmt, items, selfId);

    Widget scroller;
    if (!loading && items.isEmpty) {
      scroller = Center(
        child: Text(
          s.timelineEmptyState,
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
                return Padding(
                  padding: EdgeInsets.only(top: row.topSpacing),
                  child: _buildRow(context, row, store, selfId, rowCap),
                );
              },
            ),
          );
        },
      );
    }

    return Semantics(
      container: true,
      liveRegion: true, // role="log"
      label: s.timelineRoomTimeline,
      child: Stack(
        children: [
          Positioned.fill(child: SelectionArea(child: scroller)),
          if (_newItemCount > 0)
            Positioned(
              left: 0,
              right: 0,
              bottom: JeliyaSpacing.x14,
              child: Center(child: _newMessagesPill(s, tokens)),
            ),
        ],
      ),
    );
  }

  List<_Row> _buildRows(JeliyaFormats fmt, List<_Item> items, String? selfId) {
    final rows = <_Row>[];
    var lastDay = '';
    _Item? prevItem;
    _Side? prevSide;
    var afterDivider = false;
    for (final item in items) {
      final day = fmt.dayLabel(item.ts);
      if (day != lastDay) {
        lastDay = day;
        rows.add(_Row.divider(day,
            topSpacing: rows.isEmpty ? 0 : _gapDivider));
        afterDivider = true;
      }
      final compact = _shouldGroup(prevItem, item, selfId);
      final side = _itemSide(item, selfId);
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
      rows.add(_Row.item(item, side: side, compact: compact, topSpacing: top));
      prevItem = item;
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
    final item = row.item!;
    final pending = item.pendingMsg;
    if (pending != null) {
      return _alignRow(
        _Side.own,
        rowCap,
        _pendingCard(context, store, pending, row.compact),
      );
    }
    final event = item.event!;
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

  /// Sender name + optional AGENT chip + time (msg-meta, 12px).
  Widget _metaRow(BuildContext context, TimelineEvent event, {required bool own}) {
    return Row(
      mainAxisSize: MainAxisSize.min,
      mainAxisAlignment:
          own ? MainAxisAlignment.end : MainAxisAlignment.start,
      children: [
        SenderName(
          id: event.sender.identityId,
          style: JeliyaText.name.copyWith(fontSize: 12),
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
    final text = Text(
      body,
      style: JeliyaText.body.copyWith(color: dim ? tokens.textDim : tokens.text),
    );
    final Widget bubble;
    if (own) {
      bubble = Container(
        padding: const EdgeInsets.symmetric(
            horizontal: JeliyaSpacing.x14, vertical: JeliyaSpacing.x10),
        decoration: BoxDecoration(
          gradient: LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [tokens.bubbleOwnGradientStart, tokens.bubbleOwnGradientEnd],
          ),
          border: Border.all(color: tokens.bubbleOwnBorder),
          borderRadius: radius,
          boxShadow: const [
            BoxShadow(
              color: Color(0x29000000), // rgba(0,0,0,0.16)
              offset: Offset(0, 10),
              blurRadius: 26,
            ),
          ],
        ),
        child: text,
      );
    } else {
      // 1px hairline + the 2px blue LEFT edge: a clipped stripe (Flutter
      // can't mix non-uniform border widths with a radius).
      bubble = Container(
        decoration: BoxDecoration(
          color: tokens.bubbleRemoteBg,
          border: Border.all(color: tokens.border),
          borderRadius: radius,
        ),
        child: ClipRRect(
          borderRadius: radius,
          // IntrinsicHeight bounds the stretch: timeline rows get unbounded
          // height from the list, and a stretch Row under an infinite max
          // height is a layout error (the stripe must match the text height).
          child: IntrinsicHeight(
            child: Row(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Container(width: 2, color: tokens.bubbleRemoteEdge),
                Flexible(
                  child: Padding(
                    padding: const EdgeInsets.symmetric(
                        horizontal: JeliyaSpacing.x12, vertical: JeliyaSpacing.x10),
                    child: text,
                  ),
                ),
              ],
            ),
          ),
        ),
      );
    }
    return ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: _bubbleMaxWidth),
      child: bubble,
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
            Text(label, style: JeliyaText.meta),
            if (failed) ...[
              const SizedBox(width: JeliyaSpacing.x6),
              _TextButton(
                label: s.commonRetry,
                onTap: () => store.retryPendingMessage(message.clientId),
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
          Row(
            children: [
              SenderName(
                id: event.sender.identityId,
                style: JeliyaText.name.copyWith(fontSize: 13),
              ),
              const SizedBox(width: JeliyaSpacing.x8),
              const _AgentChip(),
              const SizedBox(width: JeliyaSpacing.x8),
              _time(context, event.ts),
              const Spacer(),
              const SizedBox(width: JeliyaSpacing.x8),
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
          if (own)
            const _ServingNote()
          else
            FetchControl(
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
            ),
        ],
      ),
    );

    if (own) return tile;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [tile, FetchDetail(state: state)],
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
          JeliyaButton(
            label: s.timelineOpenInPipes,
            size: JeliyaButtonSize.sm,
            onPressed: widget.onShowPipes,
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

  Widget _newMessagesPill(AppStrings s, JeliyaTokens tokens) {
    return DecoratedBox(
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(JeliyaRadii.pill),
        boxShadow: const [
          BoxShadow(
            color: Color(0x47000000), // rgba(0,0,0,0.28)
            offset: Offset(0, 10),
            blurRadius: 30,
          ),
        ],
      ),
      child: TextButton(
        onPressed: _scrollToBottom,
        style: TextButton.styleFrom(
          backgroundColor: tokens.bgCard2,
          foregroundColor: tokens.accent,
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
          minimumSize: Size.zero,
          tapTargetSize: MaterialTapTargetSize.shrinkWrap,
          textStyle: const TextStyle(fontSize: 12.5, fontWeight: FontWeight.w700),
          shape: StadiumBorder(side: BorderSide(color: tokens.accentLine)),
        ),
        child: Text(s.timelineNewMessages(_newItemCount)),
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

/// Bare accent text button (.text-btn) — the pending Retry affordance.
class _TextButton extends StatelessWidget {
  const _TextButton({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    return Semantics(
      button: true,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        child: GestureDetector(
          onTap: onTap,
          child: Text(
            label,
            style: TextStyle(
              fontSize: 11.5,
              fontWeight: FontWeight.w700,
              color: tokens.accent,
            ),
          ),
        ),
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
