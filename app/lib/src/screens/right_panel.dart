/// RightPanel (RightPanel.tsx port) — the in-room side panel hosting the
/// Members / Agents / Files / Pipes tabs with counts, per phase3-features.json
/// "RightPanel shell" + the four tab specs.
///
/// - Tab strip: counts capped at '99+', roving focus (arrow keys wrap,
///   Home/End jump — selection AND focus move together).
/// - Members: roster from the signed room history; statuses reflect
///   membership events, never live reachability (P4).
/// - Agents: latest agent_status derived by scanning the timeline backwards —
///   no protocol call.
/// - Files: honest availability accounting (self-shared files render
///   'Serving', never 'No provider online'), share picker + advanced
///   daemon-readable path, FetchControl/FetchDetail per file.
/// - Pipes: connect/preview/close via RoomStore.connectPipe/closePipe with
///   the pipeConns/closingPipes discipline, expose form to exactly one peer.
///
/// Data comes from `SessionScope.of(context).room`; this widget listens to
/// the RoomStore itself (the shell's ListenableBuilder only wraps the center
/// column).
library;

import 'dart:async';
import 'dart:math' as math;

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart'
    show
        ErrorCodes,
        FetchPhases,
        FetchState,
        FileEntry,
        Member,
        PipeEntry,
        PipeStates,
        RequestError,
        Roles,
        TimelineEvent,
        TimelineKinds,
        errorShape,
        labelTone;
import 'package:url_launcher/url_launcher.dart';

import '../format.dart';
import '../l10n/error_display.dart';
import '../l10n/strings_context.dart';
import '../l10n/tokens.dart';
import '../l10n/wire_display.dart';
import '../session/daemon_session.dart';
import '../session/room_store.dart';
import '../theme.dart';
import '../widgets/avatar.dart';
import '../widgets/buttons.dart';
import '../widgets/error_note.dart';
import '../widgets/fetch_control.dart';
import '../widgets/progress_bar.dart';
import '../widgets/sender_name.dart';
import '../widgets/template_text.dart';

/// Right-panel tabs (RightPanel.tsx `PanelTab`).
enum PanelTab { members, agents, files, pipes }

String _tabLabel(AppStrings s, PanelTab tab) => switch (tab) {
      PanelTab.members => s.panelTabMembers,
      PanelTab.agents => s.panelTabAgents,
      PanelTab.files => s.panelTabFiles,
      PanelTab.pipes => s.panelTabPipes,
    };

// Clock formatting goes through ../format.dart context.formats clock — one
// consistent (and later locale-aware) clock for every surface.

// prettyLabel / extOf live in ../format.dart — the shared single home.

class RightPanel extends StatelessWidget {
  const RightPanel({
    super.key,
    required this.tab,
    required this.onTab,
    required this.onLeaveRoom,
  });

  /// The active tab — owned by the shell (RoomHeader's Share File / Open Pipe
  /// buttons and the nav rail also switch it).
  final PanelTab tab;

  final ValueChanged<PanelTab> onTab;

  /// Opens the Leave Room modal.
  final VoidCallback onLeaveRoom;

  @override
  Widget build(BuildContext context) {
    final session = SessionScope.of(context);
    final tokens = JeliyaTokens.of(context);
    final room = session.room;
    return Container(
      decoration: BoxDecoration(
        color: tokens.bgRaise,
        border: Border(left: BorderSide(color: tokens.border)),
      ),
      child: room == null
          ? _buildPanel(context, session, null)
          : ListenableBuilder(
              listenable: room,
              builder: (context, _) => _buildPanel(context, session, room),
            ),
    );
  }

  Widget _buildPanel(
      BuildContext context, DaemonSession session, RoomStore? room) {
    final s = context.strings;
    final members = room?.members ?? const <Member>[];
    final files = room?.files ?? const <FileEntry>[];
    final pipes = room?.pipes ?? const <PipeEntry>[];
    final agentCount = members.where((m) => m.role == Roles.agent).length;
    final openPipes = pipes.where((p) => p.state == PipeStates.open).length;

    final counts = <PanelTab, int>{
      PanelTab.members: members.length,
      PanelTab.agents: agentCount,
      PanelTab.files: files.length,
      PanelTab.pipes: openPipes,
    };

    final Widget body = switch (tab) {
      PanelTab.members => _MembersTab(
          members: members,
          session: session,
          onLeaveRoom: onLeaveRoom,
        ),
      PanelTab.agents => _AgentsTab(
          members: members,
          timeline: room?.timeline ?? const <TimelineEvent>[],
        ),
      PanelTab.files => _FilesTab(
          key: ValueKey('files-${room?.roomId}'),
          room: room,
          session: session,
        ),
      PanelTab.pipes => _PipesTab(
          key: ValueKey('pipes-${room?.roomId}'),
          room: room,
          session: session,
        ),
    };

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _PanelTabs(tab: tab, onTab: onTab, counts: counts),
        Expanded(
          // role='tabpanel' labelled by the active tab.
          child: Semantics(
            container: true,
            label: _tabLabel(s, tab),
            child: SingleChildScrollView(
              padding: const EdgeInsets.all(JeliyaSpacing.panel),
              child: body,
            ),
          ),
        ),
      ],
    );
  }
}

// -- tab strip ----------------------------------------------------------------------

class _PanelTabs extends StatefulWidget {
  const _PanelTabs({required this.tab, required this.onTab, required this.counts});

  final PanelTab tab;
  final ValueChanged<PanelTab> onTab;
  final Map<PanelTab, int> counts;

  @override
  State<_PanelTabs> createState() => _PanelTabsState();
}

class _PanelTabsState extends State<_PanelTabs> {
  late final Map<PanelTab, FocusNode> _nodes = {
    for (final tab in PanelTab.values)
      tab: FocusNode(debugLabel: 'panel-tab-${tab.name}', onKeyEvent: _onKey),
  };

  @override
  void dispose() {
    for (final node in _nodes.values) {
      node.dispose();
    }
    super.dispose();
  }

  /// Full ARIA tabs keyboard pattern: arrow keys move between tabs (one tab
  /// stop via roving focus), Home/End jump to the ends — selection AND focus.
  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is KeyUpEvent) return KeyEventResult.ignored;
    final tabs = PanelTab.values;
    final idx = tabs.indexOf(widget.tab);
    final key = event.logicalKey;
    int next;
    if (key == LogicalKeyboardKey.arrowRight ||
        key == LogicalKeyboardKey.arrowDown) {
      next = (idx + 1) % tabs.length;
    } else if (key == LogicalKeyboardKey.arrowLeft ||
        key == LogicalKeyboardKey.arrowUp) {
      next = (idx - 1 + tabs.length) % tabs.length;
    } else if (key == LogicalKeyboardKey.home) {
      next = 0;
    } else if (key == LogicalKeyboardKey.end) {
      next = tabs.length - 1;
    } else {
      return KeyEventResult.ignored;
    }
    widget.onTab(tabs[next]);
    _nodes[tabs[next]]!.requestFocus();
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    // Roving tabindex: only the active tab participates in Tab traversal.
    for (final tab in PanelTab.values) {
      _nodes[tab]!.skipTraversal = tab != widget.tab;
    }
    return Semantics(
      container: true,
      label: s.panelRoomPanel,
      child: Container(
        padding: const EdgeInsets.fromLTRB(
            JeliyaSpacing.x8, JeliyaSpacing.x12, JeliyaSpacing.x8, 0),
        decoration: BoxDecoration(
          border: Border(bottom: BorderSide(color: tokens.border)),
        ),
        // Content-sized tabs, not width quarters: any flex wrapper caps a
        // tab at 1/4 of the row, truncating the longest label under wider
        // locales ('Membres' + count badge ellipsized to 'Me…' in fr). The
        // ConstrainedBox keeps the justified spaceBetween layout whenever
        // the tabs fit (fr with every badge populated does, with slack);
        // only pathological states (e.g. four 99+ badges) engage the
        // horizontal scroll instead of clipping the last tab — a focused
        // tab auto-scrolls into view (InkResponse ensures visibility).
        child: LayoutBuilder(
          builder: (context, constraints) => SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: ConstrainedBox(
              constraints: BoxConstraints(minWidth: constraints.maxWidth),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.end,
                mainAxisAlignment: MainAxisAlignment.spaceBetween,
                children: [
                  for (final tab in PanelTab.values)
                    _PanelTabButton(
                      label: _tabLabel(s, tab),
                      count: widget.counts[tab] ?? 0,
                      active: tab == widget.tab,
                      focusNode: _nodes[tab]!,
                      onTap: () => widget.onTab(tab),
                    ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _PanelTabButton extends StatefulWidget {
  const _PanelTabButton({
    required this.label,
    required this.count,
    required this.active,
    required this.focusNode,
    required this.onTap,
  });

  final String label;
  final int count;
  final bool active;
  final FocusNode focusNode;
  final VoidCallback onTap;

  @override
  State<_PanelTabButton> createState() => _PanelTabButtonState();
}

class _PanelTabButtonState extends State<_PanelTabButton> {
  bool _hovered = false;
  bool _focused = false;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final ink = widget.active || _hovered ? tokens.text : tokens.textDim;
    return Semantics(
      button: true,
      selected: widget.active,
      child: InkWell(
        focusNode: widget.focusNode,
        onTap: widget.onTap,
        onHover: (hovered) => setState(() => _hovered = hovered),
        onFocusChange: (focused) => setState(() => _focused = focused),
        focusColor: Colors.transparent,
        child: Container(
          // x4, not x6: the four French labels + all four count badges must
          // fit the 303px strip without engaging the overflow scroll.
          padding: const EdgeInsets.fromLTRB(
              JeliyaSpacing.x4, JeliyaSpacing.x8, JeliyaSpacing.x4, JeliyaSpacing.x10),
          decoration: BoxDecoration(
            // 2px active-tab underline; a focus ring for keyboard users.
            border: Border(
              bottom: BorderSide(
                color: widget.active ? tokens.accent : Colors.transparent,
                width: 2,
              ),
            ),
            boxShadow: _focused
                ? [BoxShadow(color: tokens.accent, spreadRadius: 1)]
                : null,
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Flexible(
                child: Text(
                  widget.label,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 13, fontWeight: FontWeight.w600, color: ink),
                ),
              ),
              if (widget.count > 0) ...[
                const SizedBox(width: JeliyaSpacing.x4),
                Container(
                  constraints: const BoxConstraints(minWidth: 17),
                  height: 17,
                  padding: const EdgeInsets.symmetric(horizontal: 4),
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: tokens.bgCard2,
                    borderRadius: BorderRadius.circular(JeliyaRadii.pill),
                    border: Border.all(color: tokens.borderStrong),
                  ),
                  child: Text(
                    widget.count > 99
                        ? Tokens.countCap
                        : '${widget.count}',
                    style: TextStyle(fontSize: 10, color: tokens.textDim),
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

// -- shared bits ----------------------------------------------------------------------

/// Centered muted empty state (`.panel-empty`).
class _PanelEmpty extends StatelessWidget {
  const _PanelEmpty(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 40, horizontal: 20),
      child: Text(
        text,
        textAlign: TextAlign.center,
        style: TextStyle(fontSize: 13, color: tokens.textDim),
      ),
    );
  }
}

/// 7px status dot in [color]; glow only for non-neutral, earned tones.
class _Dot extends StatelessWidget {
  const _Dot({required this.color, this.glow = false});

  final Color color;
  final bool glow;

  @override
  Widget build(BuildContext context) {
    return ExcludeSemantics(
      child: Container(
        width: 7,
        height: 7,
        decoration: BoxDecoration(
          color: color,
          shape: BoxShape.circle,
          boxShadow: glow
              ? [BoxShadow(color: color.withValues(alpha: 0.7), blurRadius: 6)]
              : null,
        ),
      ),
    );
  }
}

/// Uppercase micro section head with a right-aligned summary.
class _SectionHead extends StatelessWidget {
  const _SectionHead({required this.title, required this.summary});

  final String title;
  final String summary;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(2, 2, 2, 0),
      child: Row(
        children: [
          Expanded(
            child: Text(
              title.toUpperCase(),
              style: TextStyle(
                  fontSize: 12, letterSpacing: 0.72, color: tokens.textMute),
            ),
          ),
          Text(summary,
              style: TextStyle(fontSize: 11.5, color: tokens.textDim)),
        ],
      ),
    );
  }
}

/// Stat cell used by the members-summary and files heroes (dl > div).
class _StatCell extends StatelessWidget {
  const _StatCell({required this.term, required this.value});

  final String term;
  final String value;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(
          horizontal: JeliyaSpacing.x10, vertical: JeliyaSpacing.x8),
      decoration: BoxDecoration(
        color: tokens.bg.withValues(alpha: 0.42),
        borderRadius: BorderRadius.circular(JeliyaRadii.nav),
        border: Border.all(color: tokens.border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            term.toUpperCase(),
            style: TextStyle(
                fontSize: 10.5, letterSpacing: 0.63, color: tokens.textMute),
          ),
          const SizedBox(height: JeliyaSpacing.x2),
          Text(
            value,
            style: JeliyaText.mono(
                fontSize: 15, color: tokens.text, fontWeight: FontWeight.w700),
          ),
        ],
      ),
    );
  }
}

/// Dashed 1px border (the reference's affordance-invitation style — Flutter
/// has no built-in dashed borders).
class _DashedBorder extends StatelessWidget {
  const _DashedBorder({required this.radius, required this.child, this.color});

  final double radius;
  final Widget child;
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    return CustomPaint(
      foregroundPainter: _DashedBorderPainter(
          color: color ?? tokens.borderStrong, radius: radius),
      child: child,
    );
  }
}

class _DashedBorderPainter extends CustomPainter {
  const _DashedBorderPainter({required this.color, required this.radius});

  final Color color;
  final double radius;

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1;
    final path = Path()
      ..addRRect(RRect.fromRectAndRadius(
          (Offset.zero & size).deflate(0.5), Radius.circular(radius)));
    const dash = 5.0;
    const gap = 4.0;
    for (final metric in path.computeMetrics()) {
      var distance = 0.0;
      while (distance < metric.length) {
        canvas.drawPath(
          metric.extractPath(
              distance, math.min(distance + dash, metric.length)),
          paint,
        );
        distance += dash + gap;
      }
    }
  }

  @override
  bool shouldRepaint(covariant _DashedBorderPainter oldDelegate) =>
      oldDelegate.color != color || oldDelegate.radius != radius;
}

void _launch(String url) => unawaited(launchUrl(Uri.parse(url)));

// -- Members tab -------------------------------------------------------------------------

int _roleRank(String role) => switch (role) {
      Roles.owner => 0,
      Roles.agent => 1,
      _ => 2,
    };

int _statusRank(String status) => switch (status) {
      'active' => 0,
      'invited' => 1,
      'left' => 2,
      'removed' => 3,
      _ => 4,
    };

String _displayStatus(AppStrings s, String status) => s.memberStatus(status);

String _displayRole(AppStrings s, String role) => s.rolePill(role);

String _shortMemberId(String id) => id.length > 18
    ? '${id.substring(0, 8)}…${id.substring(id.length - 6)}'
    : id;

class _MembersTab extends StatelessWidget {
  const _MembersTab({
    required this.members,
    required this.session,
    required this.onLeaveRoom,
  });

  final List<Member> members;
  final DaemonSession session;
  final VoidCallback onLeaveRoom;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    if (members.isEmpty) return _PanelEmpty(s.panelMembersEmpty);

    final sorted = [...members]..sort((a, b) {
        final aSelf = session.isSelf(a.identityId);
        final bSelf = session.isSelf(b.identityId);
        if (aSelf != bSelf) return aSelf ? -1 : 1;
        final byStatus = _statusRank(a.status) - _statusRank(b.status);
        if (byStatus != 0) return byStatus;
        final byRole = _roleRank(a.role) - _roleRank(b.role);
        if (byRole != 0) return byRole;
        return session
            .displayName(s, a.identityId)
            .compareTo(session.displayName(s, b.identityId));
      });
    final activeCount = members.where((m) => m.status == 'active').length;
    final invitedCount = members.where((m) => m.status == 'invited').length;
    final agentCount = members.where((m) => m.role == Roles.agent).length;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _buildSummary(context, activeCount, invitedCount, agentCount),
        const SizedBox(height: JeliyaSpacing.x10),
        _SectionHead(
          title: s.panelRoomRoster,
          summary: s.panelNActive(activeCount),
        ),
        for (final member in sorted) ...[
          const SizedBox(height: JeliyaSpacing.x10),
          _MemberRow(
              member: member, session: session, onLeaveRoom: onLeaveRoom),
        ],
      ],
    );
  }

  Widget _buildSummary(
      BuildContext context, int active, int invited, int agents) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    return Semantics(
      container: true,
      label: s.panelMembersSummaryLabel,
      child: Container(
        decoration: BoxDecoration(
          color: tokens.bgCard,
          borderRadius: BorderRadius.circular(JeliyaRadii.hero),
          border: Border.all(color: tokens.borderStrong),
        ),
        child: Container(
          padding: const EdgeInsets.all(JeliyaSpacing.panel),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(JeliyaRadii.hero),
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              stops: const [0, 0.62],
              colors: [
                tokens.blue.withValues(alpha: 0.1),
                tokens.accent.withValues(alpha: 0.035),
              ],
            ),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(s.panelRoomMemberCount(members.length),
                  style: JeliyaText.cardTitle),
              const SizedBox(height: JeliyaSpacing.x4),
              Text(s.panelRosterCopy,
                  style: TextStyle(fontSize: 12.5, color: tokens.textDim)),
              const SizedBox(height: JeliyaSpacing.x12),
              Semantics(
                container: true,
                label: s.panelMemberCountsLabel,
                child: Row(
                  children: [
                    Expanded(
                        child: _StatCell(
                            term: s.panelStatActive, value: '$active')),
                    const SizedBox(width: JeliyaSpacing.x8),
                    Expanded(
                        child: _StatCell(
                            term: s.panelStatAgents, value: '$agents')),
                    if (invited > 0) ...[
                      const SizedBox(width: JeliyaSpacing.x8),
                      Expanded(
                          child: _StatCell(
                              term: s.panelStatInvited,
                              value: '$invited')),
                    ],
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _MemberRow extends StatelessWidget {
  const _MemberRow({
    required this.member,
    required this.session,
    required this.onLeaveRoom,
  });

  final Member member;
  final DaemonSession session;
  final VoidCallback onLeaveRoom;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final mine = session.isSelf(member.identityId);
    final departed = member.status == 'left' || member.status == 'removed';
    final canLeave =
        mine && member.status == 'active' && member.role != Roles.owner;
    final ownerCannotLeave =
        mine && member.status == 'active' && member.role == Roles.owner;

    final statusColor = switch (member.status) {
      'active' => tokens.accent,
      'invited' => tokens.blue,
      _ => tokens.textMute,
    };
    final roleColor = switch (member.role) {
      Roles.owner => tokens.accent,
      Roles.agent => tokens.blue,
      _ => tokens.textDim,
    };

    return Tooltip(
      message: member.identityId,
      child: Container(
        padding: const EdgeInsets.symmetric(
            horizontal: JeliyaSpacing.x12, vertical: JeliyaSpacing.x12 - 1),
        decoration: BoxDecoration(
          // Departed rows recede to the muted ground — never blanket opacity.
          color: departed ? tokens.bgRaise : tokens.bgCard,
          borderRadius: BorderRadius.circular(JeliyaRadii.bubble),
          border: Border.all(color: tokens.border),
        ),
        child: Row(
          children: [
            Avatar(id: member.identityId, size: 38),
            const SizedBox(width: JeliyaSpacing.x10),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  // Wrap, not Row: at the 320px panel width the self row's
                  // 'this device' chip does not fit beside the name next to
                  // the badges column — let it drop to a second line.
                  Wrap(
                    spacing: 7,
                    runSpacing: JeliyaSpacing.x2,
                    crossAxisAlignment: WrapCrossAlignment.center,
                    children: [
                      SenderName(id: member.identityId),
                      if (mine)
                        Container(
                          padding: const EdgeInsets.symmetric(
                              horizontal: 7, vertical: JeliyaSpacing.x2),
                          decoration: BoxDecoration(
                            color: tokens.accentDim,
                            borderRadius:
                                BorderRadius.circular(JeliyaRadii.pill),
                            border: Border.all(color: tokens.accentLine),
                          ),
                          child: Text(
                            s.panelThisDevice,
                            style: TextStyle(
                                fontSize: 10.5, color: tokens.accent),
                          ),
                        ),
                    ],
                  ),
                  const SizedBox(height: JeliyaSpacing.x2),
                  Text(
                    _shortMemberId(member.identityId),
                    // One line, always: the id is decorative (the Tooltip
                    // carries the full value) and must never wrap into
                    // glyph soup when a wide locale squeezes this column.
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: JeliyaText.mono(fontSize: 11, color: tokens.textMute),
                  ),
                ],
              ),
            ),
            const SizedBox(width: JeliyaSpacing.x10),
            Semantics(
              container: true,
              label:
                  '${_displayRole(s, member.role)}, ${_displayStatus(s, member.status)}',
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.end,
                children: [
                  _pill(
                      tokens,
                      _displayRole(s, member.role),
                      roleColor,
                      member.role == Roles.owner
                          ? tokens.accentLine
                          : tokens.borderStrong),
                  const SizedBox(height: 5),
                  _pill(tokens, _displayStatus(s, member.status), statusColor,
                      tokens.borderStrong,
                      dot: true),
                  if (ownerCannotLeave) ...[
                    const SizedBox(height: 5),
                    // Under the pills, width-capped, wrapping to two lines:
                    // inline it starved the name column at the 320px panel
                    // width once French copy ('Le propriétaire reste') got
                    // ~2x wider than 'Owner stays'.
                    Tooltip(
                      message: s.panelOwnerStaysTitle,
                      child: ConstrainedBox(
                        constraints: BoxConstraints(
                            // Scale-aware: a fixed cap would force
                            // mid-word breaks under large accessibility
                            // text factors.
                            maxWidth:
                                MediaQuery.textScalerOf(context).scale(96)),
                        child: Text(
                          s.panelOwnerStays,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          textAlign: TextAlign.end,
                          style:
                              TextStyle(fontSize: 11, color: tokens.textMute),
                        ),
                      ),
                    ),
                  ],
                ],
              ),
            ),
            if (canLeave) ...[
              const SizedBox(width: JeliyaSpacing.x10),
              JeliyaButton(
                label: s.panelLeave,
                size: JeliyaButtonSize.sm,
                variant: JeliyaButtonVariant.danger,
                onPressed: onLeaveRoom,
              ),
            ],
          ],
        ),
      ),
    );
  }

  Widget _pill(JeliyaTokens tokens, String text, Color color, Color borderColor,
      {bool dot = false}) {
    return Container(
      padding:
          const EdgeInsets.symmetric(horizontal: 7, vertical: JeliyaSpacing.x2),
      decoration: BoxDecoration(
        color: tokens.bgCard2,
        borderRadius: BorderRadius.circular(JeliyaRadii.pill),
        border: Border.all(color: borderColor),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (dot) ...[
            _Dot(color: color),
            const SizedBox(width: 5),
          ],
          Text(text, style: TextStyle(fontSize: 10.5, color: color)),
        ],
      ),
    );
  }
}

// -- Agents tab -------------------------------------------------------------------------

class _AgentsTab extends StatelessWidget {
  const _AgentsTab({required this.members, required this.timeline});

  final List<Member> members;
  final List<TimelineEvent> timeline;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final agents = members.where((m) => m.role == Roles.agent).toList();
    if (agents.isEmpty) return _PanelEmpty(s.panelAgentsEmpty);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final (i, agent) in agents.indexed) ...[
          if (i > 0) const SizedBox(height: JeliyaSpacing.x10),
          _AgentCard(agent: agent, latest: _latestStatus(agent.identityId)),
        ],
      ],
    );
  }

  /// The newest agent_status by this identity — scanned from the timeline
  /// tail; no protocol call.
  TimelineEvent? _latestStatus(String identityId) {
    for (var i = timeline.length - 1; i >= 0; i--) {
      final event = timeline[i];
      if (event.kind == TimelineKinds.agentStatus &&
          event.sender.identityId == identityId) {
        return event;
      }
    }
    return null;
  }
}

class _AgentCard extends StatelessWidget {
  const _AgentCard({required this.agent, required this.latest});

  final Member agent;
  final TimelineEvent? latest;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final fmt = context.formats;
    final tokens = JeliyaTokens.of(context);
    final latest = this.latest;
    final label = latest?.label;
    final progress = latest?.progress;
    return Container(
      padding: const EdgeInsets.all(JeliyaSpacing.x12),
      decoration: BoxDecoration(
        color: tokens.bgCard,
        borderRadius: BorderRadius.circular(JeliyaRadii.composer),
        border: Border.all(color: tokens.border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Avatar(id: agent.identityId, size: 36),
              const SizedBox(width: JeliyaSpacing.x10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    SenderName(id: agent.identityId),
                    if (label != null)
                      Row(
                        children: [
                          _Dot(
                            color: tokens.toneColor(labelTone(label)),
                            glow: tokens.toneColor(labelTone(label)) !=
                                tokens.textDim,
                          ),
                          const SizedBox(width: 5),
                          Flexible(
                            child: Text(
                              prettyLabel(label),
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                  fontSize: 12, color: tokens.accent),
                            ),
                          ),
                        ],
                      )
                    else
                      Text(s.panelNoStatusPostedYet,
                          style:
                              TextStyle(fontSize: 12, color: tokens.textDim)),
                  ],
                ),
              ),
              if (latest != null)
                Text(fmt.clock(latest.ts), style: JeliyaText.meta),
            ],
          ),
          if (latest?.statusMessage != null) ...[
            const SizedBox(height: JeliyaSpacing.x8),
            Text(latest!.statusMessage!,
                style: TextStyle(fontSize: 13, color: tokens.textDim)),
          ],
          if (progress != null) ...[
            const SizedBox(height: JeliyaSpacing.x8),
            Row(
              children: [
                Expanded(child: ProgressBar(value: progress.toDouble())),
                const SizedBox(width: JeliyaSpacing.x8),
                Text(
                  fmt.percent(progress.clamp(0, 100).round()),
                  style: JeliyaText.mono(fontSize: 11.5, color: tokens.textDim),
                ),
              ],
            ),
          ],
          const SizedBox(height: JeliyaSpacing.x8),
          Text(
            s.panelAgentStatusFooter(_displayStatus(s, agent.status))
                .toUpperCase(),
            style: TextStyle(
                fontSize: 11, letterSpacing: 0.66, color: tokens.textMute),
          ),
        ],
      ),
    );
  }
}

// -- Files tab -------------------------------------------------------------------------

/// RightPanel.tsx `mimeTypeLabel`.
String _mimeTypeLabel(AppStrings s, String mime, String fallback) {
  if (mime.isEmpty) return fallback.isEmpty ? s.panelKindFile : fallback;
  final slash = mime.indexOf('/');
  if (slash < 0) return mime;
  final type = mime.substring(0, slash);
  final subtype = mime.substring(slash + 1);
  if (subtype.isEmpty) return mime;
  if (subtype == 'octet-stream') {
    return fallback.isEmpty ? s.panelKindBinary : fallback;
  }
  if (type == 'text' && subtype == 'plain') return s.panelKindText;
  return subtype.replaceAll(RegExp(r'[.+-]'), ' ');
}

class _PickedFile {
  const _PickedFile({
    required this.path,
    required this.name,
    required this.size,
    this.mime,
  });

  final String path;
  final String name;
  final int size;
  final String? mime;
}

class _FilesTab extends StatefulWidget {
  const _FilesTab({super.key, required this.room, required this.session});

  final RoomStore? room;
  final DaemonSession session;

  @override
  State<_FilesTab> createState() => _FilesTabState();
}

class _FilesTabState extends State<_FilesTab> {
  final TextEditingController _path = TextEditingController();
  _PickedFile? _selected;
  bool _sharing = false;
  RequestError? _shareError;

  @override
  void initState() {
    super.initState();
    _path.addListener(() {
      // Typing a path clears the picked file (mutually exclusive).
      if (_path.text.trim().isNotEmpty && _selected != null) {
        setState(() => _selected = null);
      } else {
        setState(() {});
      }
    });
  }

  @override
  void dispose() {
    _path.dispose();
    super.dispose();
  }

  bool _isFetched(FileEntry file, FetchState? state) =>
      state?.phase == FetchPhases.verified ||
      state?.phase == FetchPhases.fetched ||
      file.fetched;

  bool _isMine(FileEntry file) => widget.session.isSelf(file.senderId);

  Future<void> _pick() async {
    final picked = await openFile();
    if (picked == null) return;
    final size = await picked.length();
    if (!mounted) return;
    setState(() {
      _selected = _PickedFile(
        path: picked.path,
        name: picked.name,
        size: size,
        mime: picked.mimeType,
      );
      // Picking a file clears the path field (mutually exclusive).
      _path.clear();
    });
  }

  Future<void> _share() async {
    final room = widget.room;
    final path = _path.text.trim();
    final selected = _selected;
    if (room == null || _sharing || (selected == null && path.isEmpty)) return;
    setState(() {
      _sharing = true;
      _shareError = null;
    });
    try {
      if (selected != null) {
        await room.shareUserFile(selected.path,
            name: selected.name, mime: selected.mime);
        if (mounted) setState(() => _selected = null);
      } else {
        await room.shareFilePath(path);
        _path.clear();
      }
    } catch (e) {
      if (mounted) setState(() => _shareError = errorShape(e));
    } finally {
      if (mounted) setState(() => _sharing = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final files = widget.room?.files ?? const <FileEntry>[];
    final fetches = widget.room?.fetches ?? const <String, FetchState>{};

    final availableCount = files.where((f) => f.available).length;
    final fetchedCount =
        files.where((f) => _isFetched(f, fetches[f.fileId])).length;
    // A file this device shared reports available:false from its own view
    // (the daemon excludes self from the provider set) — never a fault.
    final servingCount =
        files.where((f) => !f.available && _isMine(f)).length;
    final waitingProviderCount = files.where((f) {
      final fetched = _isFetched(f, fetches[f.fileId]);
      final serving = !f.available && _isMine(f);
      return !f.available && !serving && !fetched;
    }).length;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _buildHero(context, files, availableCount, fetchedCount, servingCount),
        const SizedBox(height: JeliyaSpacing.x12),
        _buildShareForm(context),
        if (files.isNotEmpty) ...[
          const SizedBox(height: JeliyaSpacing.x12),
          _SectionHead(
            title: s.panelSharedInThisRoom,
            summary: availableCount == files.length
                ? s.panelAllFetchable
                : [
                    if (servingCount > 0) s.panelServedByYou(servingCount),
                    if (waitingProviderCount > 0)
                      s.panelAwaitingProvider(waitingProviderCount),
                  ].join(Tokens.metaSep),
          ),
          for (final file in files) ...[
            const SizedBox(height: JeliyaSpacing.x10),
            _buildFileRow(context, file, fetches[file.fileId]),
          ],
        ],
      ],
    );
  }

  Widget _buildHero(BuildContext context, List<FileEntry> files,
      int availableCount, int fetchedCount, int servingCount) {
    final s = context.strings;
    final fmt = context.formats;
    final tokens = JeliyaTokens.of(context);
    final empty = files.isEmpty;
    final totalBytes = files.fold<int>(0, (sum, f) => sum + f.size);
    final providerCount = files.fold<int>(0, (sum, f) => sum + f.providers);
    // Independent stat phrases joined by the decorative separator — a list,
    // never a sentence built from fragments.
    final detail = empty
        ? s.panelFilesHeroEmptyDetail
        : [
            s.panelFilesHeroDetail(fmt.bytes(totalBytes), availableCount),
            if (fetchedCount > 0) s.panelNFetched(fetchedCount),
            if (servingCount > 0) s.panelServedByYou(servingCount),
          ].join(Tokens.metaSep);

    final inner = Container(
      padding: const EdgeInsets.all(JeliyaSpacing.panel),
      decoration: empty
          ? null
          : BoxDecoration(
              borderRadius: BorderRadius.circular(JeliyaRadii.hero),
              gradient: LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                stops: const [0, 0.58],
                colors: [
                  tokens.accent.withValues(alpha: 0.1),
                  tokens.accent.withValues(alpha: 0.025),
                ],
              ),
            ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              ExcludeSemantics(
                child: Container(
                  width: 40,
                  height: 40,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: tokens.accentDim,
                    borderRadius: BorderRadius.circular(JeliyaRadii.row),
                    border: Border.all(color: tokens.accentLine),
                  ),
                  child: Text(Tokens.filesHeroMark,
                      style:
                          TextStyle(fontSize: 18, color: tokens.accent)),
                ),
              ),
              const SizedBox(width: JeliyaSpacing.x12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      empty
                          ? s.panelNoSharedFilesYet
                          : s.panelSharedFileCount(files.length),
                      style: JeliyaText.cardTitle,
                    ),
                    const SizedBox(height: 3),
                    Text(detail,
                        style:
                            TextStyle(fontSize: 12.5, color: tokens.textDim)),
                  ],
                ),
              ),
            ],
          ),
          if (!empty) ...[
            const SizedBox(height: JeliyaSpacing.x12),
            Semantics(
              container: true,
              label: s.panelFileAvailabilityLabel,
              child: Row(
                children: [
                  Expanded(
                    child: _StatCell(
                      term: s.panelFetchableNow,
                      value: s.panelFetchableNowValue(
                          availableCount, files.length),
                    ),
                  ),
                  const SizedBox(width: JeliyaSpacing.x8),
                  Expanded(
                    child: _StatCell(
                      term: s.panelProviderDevices,
                      value: '$providerCount',
                    ),
                  ),
                ],
              ),
            ),
          ],
        ],
      ),
    );

    final box = Container(
      decoration: BoxDecoration(
        color: tokens.bgCard,
        borderRadius: BorderRadius.circular(JeliyaRadii.hero),
        border: empty ? null : Border.all(color: tokens.accentLine),
      ),
      child: inner,
    );
    return Semantics(
      container: true,
      label: s.panelFilesSummaryLabel,
      // Empty hero: dashed accent-line affordance instead of the solid border.
      child: empty
          ? _DashedBorder(
              radius: JeliyaRadii.hero, color: tokens.accentLine, child: box)
          : box,
    );
  }

  Widget _buildShareForm(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final selected = _selected;
    final canSubmit = widget.room != null &&
        !_sharing &&
        (selected != null || _path.text.trim().isNotEmpty);
    return Container(
      padding: const EdgeInsets.all(JeliyaSpacing.x12),
      decoration: BoxDecoration(
        color: tokens.bgCard,
        borderRadius: BorderRadius.circular(JeliyaRadii.composer),
        border: Border.all(color: tokens.borderStrong),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(s.panelShareCardTitle,
                        style: TextStyle(
                            fontSize: 13,
                            fontWeight: FontWeight.w700,
                            color: tokens.text)),
                    const SizedBox(height: JeliyaSpacing.x2),
                    Text(s.panelShareCardHelp,
                        style:
                            TextStyle(fontSize: 12, color: tokens.textDim)),
                  ],
                ),
              ),
              const SizedBox(width: JeliyaSpacing.x10),
              Semantics(
                label: s.panelHashCheckedBadgeLabel,
                child: Container(
                  padding: const EdgeInsets.symmetric(
                      horizontal: JeliyaSpacing.x8, vertical: 3),
                  decoration: BoxDecoration(
                    color: tokens.accentDim,
                    borderRadius: BorderRadius.circular(JeliyaRadii.pill),
                    border: Border.all(color: tokens.accentLine),
                  ),
                  child: Text(s.panelHashCheckedBadge,
                      style:
                          TextStyle(fontSize: 10.5, color: tokens.accent)),
                ),
              ),
            ],
          ),
          const SizedBox(height: 9),
          _DashedBorder(
            radius: JeliyaRadii.card,
            child: Container(
              constraints: const BoxConstraints(minHeight: 44),
              padding: const EdgeInsets.all(7),
              decoration: BoxDecoration(
                color: tokens.bgRaise,
                borderRadius: BorderRadius.circular(JeliyaRadii.card),
              ),
              child: Row(
                children: [
                  JeliyaButton(
                    label: s.panelChooseFile,
                    size: JeliyaButtonSize.sm,
                    variant: JeliyaButtonVariant.primary,
                    semanticLabel: s.panelChooseFileToShare,
                    onPressed: _sharing ? null : () => unawaited(_pick()),
                  ),
                ],
              ),
            ),
          ),
          const SizedBox(height: 9),
          if (selected != null)
            Semantics(
              liveRegion: true,
              child: _SelectedFileCard(
                file: selected,
                onClear: () => setState(() => _selected = null),
              ),
            )
          else
            Text(s.panelNoFileSelectedYet,
                style: TextStyle(fontSize: 12, color: tokens.textDim)),
          const SizedBox(height: JeliyaSpacing.x10),
          JeliyaButton(
            label: _sharing ? s.panelSharing : s.panelShare,
            variant: JeliyaButtonVariant.primary,
            busy: _sharing,
            onPressed: canSubmit ? () => unawaited(_share()) : null,
          ),
          const SizedBox(height: JeliyaSpacing.x10),
          _AdvancedPath(controller: _path),
          ErrorNote(error: _shareError),
        ],
      ),
    );
  }

  Widget _buildFileRow(BuildContext context, FileEntry file, FetchState? state) {
    final s = context.strings;
    final fmt = context.formats;
    final tokens = JeliyaTokens.of(context);
    final room = widget.room;
    final tint = tokens.fileTint(file.name);
    final rawExt = extOf(file.name).toUpperCase();
    final ext = rawExt.isEmpty ? s.commonFileExtFallback : rawExt;
    final kind = _mimeTypeLabel(s, file.mime, rawExt.toLowerCase());
    final mine = _isMine(file);
    final fetched = _isFetched(file, state);
    final failed = state?.phase == FetchPhases.error ? state : null;
    final unavailable = !file.available && !mine && !fetched;

    // Health line — label by ownership so a self-shared file is never shown
    // as a fault (SELF-OWNED FILE SEMANTICS).
    final (Color healthColor, String healthText) = mine
        ? (tokens.textDim, s.panelHealthServingToPeers)
        : fetched
            ? (tokens.accent, s.panelHealthFetchedLocally)
            : failed != null
                ? (
                    tokens.amber,
                    failed.isHardStop
                        ? s.panelHealthSecurityCheckFailed
                        : s.panelHealthFetchFailed
                  )
                : file.available
                    ? (tokens.accent, s.panelHealthReadyToFetch)
                    : (tokens.amber, s.commonNoProviderOnline);

    return Tooltip(
      message: file.fileId,
      child: Container(
        padding: const EdgeInsets.all(JeliyaSpacing.x12),
        decoration: BoxDecoration(
          color: unavailable ? tokens.bgRaise : tokens.bgCard,
          borderRadius: BorderRadius.circular(JeliyaRadii.bubble),
          border: Border.all(color: tokens.border),
        ),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            ExcludeSemantics(
              child: Container(
                width: 40,
                height: 40,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: tint.withAlpha(0x22),
                  borderRadius: BorderRadius.circular(JeliyaRadii.btn),
                ),
                child: Text(
                  ext.length > 4 ? ext.substring(0, 4) : ext,
                  style: TextStyle(
                      fontSize: 10,
                      fontWeight: FontWeight.w800,
                      letterSpacing: 0.4,
                      color: tint),
                ),
              ),
            ),
            const SizedBox(width: JeliyaSpacing.x12 - 1),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          file.name,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 13,
                              fontWeight: FontWeight.w600,
                              color: tokens.text),
                        ),
                      ),
                      const SizedBox(width: 7),
                      Container(
                        constraints: const BoxConstraints(maxWidth: 92),
                        padding: const EdgeInsets.symmetric(
                            horizontal: JeliyaSpacing.x6, vertical: 1),
                        decoration: BoxDecoration(
                          borderRadius:
                              BorderRadius.circular(JeliyaRadii.pill),
                          border: Border.all(color: tokens.borderStrong),
                        ),
                        child: Text(
                          kind.toUpperCase(),
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 10.5, color: tokens.textMute),
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 3),
                  Wrap(
                    crossAxisAlignment: WrapCrossAlignment.center,
                    children: [
                      Text('${fmt.bytes(file.size)}${Tokens.metaSep}',
                          style: JeliyaText.meta),
                      SenderName(
                        id: file.senderId,
                        style: TextStyle(
                            fontSize: 11.5,
                            fontWeight: FontWeight.w500,
                            color: tokens.textDim),
                      ),
                      Text(Tokens.metaSep + fmt.clock(file.ts),
                          style: JeliyaText.meta),
                    ],
                  ),
                  const SizedBox(height: 3),
                  Row(
                    children: [
                      _Dot(color: healthColor),
                      const SizedBox(width: 5),
                      Flexible(
                        child: Text(
                          '$healthText${Tokens.metaSep}'
                          '${s.panelNProviders(file.providers)}',
                          overflow: TextOverflow.ellipsis,
                          style:
                              TextStyle(fontSize: 11.5, color: healthColor),
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: JeliyaSpacing.x8),
                  if (mine)
                    Align(
                      alignment: Alignment.centerLeft,
                      child: Tooltip(
                        message: s.commonServingTooltip,
                        child: Container(
                          constraints: const BoxConstraints(minHeight: 28),
                          padding: const EdgeInsets.symmetric(
                              horizontal: 9, vertical: JeliyaSpacing.x4),
                          decoration: BoxDecoration(
                            color: tokens.bgCard2,
                            borderRadius:
                                BorderRadius.circular(JeliyaRadii.pill),
                            border: Border.all(color: tokens.borderStrong),
                          ),
                          child: Text(s.commonServing,
                              style: TextStyle(
                                  fontSize: 12, color: tokens.textDim)),
                        ),
                      ),
                    )
                  else ...[
                    Align(
                      alignment: Alignment.centerLeft,
                      child: FetchControl(
                        state: state,
                        availability: FetchAvailability(
                            available: file.available,
                            providers: file.providers),
                        onFetch: () =>
                            unawaited(room?.fetchFile(file.fileId)),
                        onRecheck: () => unawaited(room?.refreshFiles()),
                      ),
                    ),
                    FetchDetail(state: state),
                  ],
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _SelectedFileCard extends StatelessWidget {
  const _SelectedFileCard({required this.file, required this.onClear});

  final _PickedFile file;
  final VoidCallback onClear;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final fmt = context.formats;
    final tokens = JeliyaTokens.of(context);
    final rawExt = extOf(file.name);
    final iconExt = rawExt.toUpperCase();
    final type = _mimeTypeLabel(s, file.mime ?? '', rawExt);
    return Container(
      padding: const EdgeInsets.all(9),
      decoration: BoxDecoration(
        color: tokens.bgCard2,
        borderRadius: BorderRadius.circular(JeliyaRadii.composer),
        border: Border.all(color: tokens.border),
      ),
      child: Row(
        children: [
          ExcludeSemantics(
            child: Container(
              width: 35,
              height: 35,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: tokens.accentDim,
                borderRadius: BorderRadius.circular(JeliyaRadii.nav),
                border: Border.all(color: tokens.accentLine),
              ),
              child: Text(
                iconExt.isEmpty
                    ? s.commonFileExtFallback
                    : (iconExt.length > 4 ? iconExt.substring(0, 4) : iconExt),
                style: JeliyaText.mono(
                    fontSize: 10,
                    color: tokens.accent,
                    fontWeight: FontWeight.w800),
              ),
            ),
          ),
          const SizedBox(width: JeliyaSpacing.x10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  file.name,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 12.5,
                      fontWeight: FontWeight.w600,
                      color: tokens.text),
                ),
                const SizedBox(height: JeliyaSpacing.x2),
                Text('${fmt.bytes(file.size)}${Tokens.metaSep}$type',
                    style: TextStyle(fontSize: 12.5, color: tokens.textDim)),
              ],
            ),
          ),
          const SizedBox(width: JeliyaSpacing.x10),
          JeliyaButton(
            label: s.panelClearSelectedFile,
            size: JeliyaButtonSize.sm,
            onPressed: onClear,
          ),
        ],
      ),
    );
  }
}

/// `<details>` 'Advanced: paste a daemon-readable path'.
class _AdvancedPath extends StatefulWidget {
  const _AdvancedPath({required this.controller});

  final TextEditingController controller;

  @override
  State<_AdvancedPath> createState() => _AdvancedPathState();
}

class _AdvancedPathState extends State<_AdvancedPath> {
  bool _open = false;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    return Container(
      padding: const EdgeInsets.only(top: 9),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: tokens.border)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          InkWell(
            onTap: () => setState(() => _open = !_open),
            child: Text(
              '${_open ? '▾' : '▸'} ${s.panelAdvancedPathSummary}',
              style: TextStyle(fontSize: 12, color: tokens.textDim),
            ),
          ),
          if (_open) ...[
            const SizedBox(height: JeliyaSpacing.x8),
            Semantics(
              label: s.panelPathFieldLabel,
              child: TextField(
                controller: widget.controller,
                style: JeliyaText.mono(fontSize: 12.5, color: tokens.text),
                decoration:
                    InputDecoration(hintText: s.panelPathPlaceholder),
              ),
            ),
            const SizedBox(height: JeliyaSpacing.x8),
            Text(s.panelPathHint,
                style: TextStyle(fontSize: 11.5, color: tokens.textMute)),
          ],
        ],
      ),
    );
  }
}

// -- Pipes tab -------------------------------------------------------------------------

class _PipesTab extends StatefulWidget {
  const _PipesTab({super.key, required this.room, required this.session});

  final RoomStore? room;
  final DaemonSession session;

  @override
  State<_PipesTab> createState() => _PipesTabState();
}

class _PipesTabState extends State<_PipesTab> {
  final TextEditingController _target = TextEditingController();
  String? _peer;
  bool _exposing = false;
  RequestError? _exposeError;

  @override
  void initState() {
    super.initState();
    _target.addListener(() => setState(() {}));
  }

  @override
  void dispose() {
    _target.dispose();
    super.dispose();
  }

  List<Member> get _peerChoices => (widget.room?.members ?? const <Member>[])
      .where((m) => !widget.session.isSelf(m.identityId))
      .toList();

  Future<void> _expose() async {
    final room = widget.room;
    final choices = _peerChoices;
    final target = _target.text.trim();
    final peer = _peer ?? (choices.isNotEmpty ? choices.first.identityId : '');
    if (room == null || target.isEmpty || peer.isEmpty || _exposing) return;
    setState(() {
      _exposing = true;
      _exposeError = null;
    });
    try {
      await room.exposePipe(target: target, peerIdentity: peer);
      _target.clear();
    } catch (e) {
      if (mounted) setState(() => _exposeError = errorShape(e));
    } finally {
      if (mounted) setState(() => _exposing = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final pipes = widget.room?.pipes ?? const <PipeEntry>[];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (pipes.isEmpty) _PanelEmpty(s.panelPipesEmpty),
        for (final pipe in pipes) ...[
          _PipeRow(pipe: pipe, room: widget.room, session: widget.session),
          const SizedBox(height: JeliyaSpacing.x10),
        ],
        const SizedBox(height: JeliyaSpacing.x6),
        _buildExposeForm(context),
      ],
    );
  }

  Widget _buildExposeForm(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final choices = _peerChoices;
    final effectivePeer = choices.any((m) => m.identityId == _peer)
        ? _peer
        : (choices.isNotEmpty ? choices.first.identityId : null);
    final canSubmit = widget.room != null &&
        !_exposing &&
        _target.text.trim().isNotEmpty &&
        choices.isNotEmpty;
    return _DashedBorder(
      radius: JeliyaRadii.composer,
      child: Container(
        padding: const EdgeInsets.all(JeliyaSpacing.x12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(s.panelExposeTitle,
                style: TextStyle(
                    fontSize: 13,
                    fontWeight: FontWeight.w700,
                    color: tokens.text)),
            const SizedBox(height: JeliyaSpacing.x2),
            Text(s.panelExposeCopy,
                style: TextStyle(fontSize: 12, color: tokens.textDim)),
            const SizedBox(height: 9),
            Semantics(
              label: s.panelTargetFieldLabel,
              child: TextField(
                controller: _target,
                style: JeliyaText.mono(fontSize: 12.5, color: tokens.text),
                decoration: const InputDecoration(
                    hintText: Tokens.targetPlaceholderExample),
                onSubmitted: (_) => _expose(),
              ),
            ),
            const SizedBox(height: 9),
            Row(
              children: [
                Expanded(
                  child: Semantics(
                    label: s.panelAuthorizedPeerLabel,
                    child: Container(
                      padding: const EdgeInsets.symmetric(
                          horizontal: JeliyaSpacing.x10),
                      decoration: BoxDecoration(
                        color: tokens.bgInput,
                        borderRadius: BorderRadius.circular(JeliyaRadii.btn),
                        border: Border.all(color: tokens.borderStrong),
                      ),
                      child: DropdownButtonHideUnderline(
                        child: DropdownButton<String>(
                          value: effectivePeer,
                          isExpanded: true,
                          isDense: true,
                          padding: const EdgeInsets.symmetric(
                              vertical: JeliyaSpacing.x8),
                          hint: Text(s.panelNoOtherMembers,
                              style: TextStyle(
                                  fontSize: 13, color: tokens.textMute)),
                          disabledHint: Text(s.panelNoOtherMembers,
                              style: TextStyle(
                                  fontSize: 13, color: tokens.textMute)),
                          dropdownColor: tokens.bgCard,
                          style:
                              TextStyle(fontSize: 13, color: tokens.text),
                          items: [
                            for (final member in choices)
                              DropdownMenuItem(
                                value: member.identityId,
                                child: Text(
                                  s.panelPeerChoice(
                                    widget.session
                                        .displayName(s, member.identityId),
                                    s.roleInline(member.role),
                                  ),
                                  overflow: TextOverflow.ellipsis,
                                ),
                              ),
                          ],
                          onChanged: choices.isEmpty
                              ? null
                              : (value) => setState(() => _peer = value),
                        ),
                      ),
                    ),
                  ),
                ),
                const SizedBox(width: JeliyaSpacing.x8),
                JeliyaButton(
                  label:
                      _exposing ? s.panelExposing : s.panelExpose,
                  busy: _exposing,
                  onPressed: canSubmit ? () => unawaited(_expose()) : null,
                ),
              ],
            ),
            ErrorNote(
              error: _exposeError,
              // pipe.expose has flow-specific failure meanings (PROTOCOL.md):
              // pipe_denied = non-loopback target, invalid_params = malformed
              // target/peer — the generic code copy reads wrong here.
              friendly: switch (_exposeError?.code) {
                ErrorCodes.pipeDenied => FriendlyError(
                    title: s.panelExposeDeniedTitle,
                    message: s.panelExposeDeniedMessage,
                    action: s.panelExposeDeniedAction,
                  ),
                ErrorCodes.invalidParams => FriendlyError(
                    title: s.panelExposeInvalidTitle,
                    message: s.panelExposeInvalidMessage,
                    action: s.panelExposeInvalidAction,
                  ),
                _ => null,
              },
            ),
          ],
        ),
      ),
    );
  }
}

class _PipeRow extends StatelessWidget {
  const _PipeRow({required this.pipe, required this.room, required this.session});

  final PipeEntry pipe;
  final RoomStore? room;
  final DaemonSession session;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final closed = pipe.state == PipeStates.closed;
    final conn = room?.pipeConns[pipe.pipeId];
    final closing = room?.closingPipes.contains(pipe.pipeId) ?? false;
    final authorizedPeer = pipe.authorizedPeer;

    final (String chipText, Color chipColor, Color chipBorder) = closed
        ? (s.panelPipeStateClosed, tokens.textMute, tokens.borderStrong)
        : pipe.connected
            ? (s.panelPipeStateActive, tokens.accent, tokens.accentLine)
            : (s.panelPipeStateOpen, tokens.accent, tokens.accentLine);

    return Container(
      padding: const EdgeInsets.symmetric(
          horizontal: JeliyaSpacing.x12, vertical: JeliyaSpacing.x12 - 1),
      decoration: BoxDecoration(
        // Closed pipes recede via the muted ground + dimmed icon only — text
        // never dims below the AA floor.
        color: closed ? tokens.bgRaise : tokens.bgCard,
        borderRadius: BorderRadius.circular(JeliyaRadii.composer),
        border: Border.all(color: tokens.border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              ExcludeSemantics(
                child: Opacity(
                  opacity: closed ? 0.5 : 1,
                  child: Container(
                    width: 34,
                    height: 34,
                    alignment: Alignment.center,
                    decoration: BoxDecoration(
                      color: tokens.accentDim,
                      borderRadius: BorderRadius.circular(JeliyaRadii.btn),
                    ),
                    child: Text(Tokens.pipeIcon,
                        style:
                            TextStyle(fontSize: 17, color: tokens.accent)),
                  ),
                ),
              ),
              const SizedBox(width: 9),
              Expanded(
                child: Text(
                  pipe.target,
                  overflow: TextOverflow.ellipsis,
                  style: JeliyaText.mono(
                      fontSize: 12.9,
                      color: tokens.text,
                      fontWeight: FontWeight.w600),
                ),
              ),
              const SizedBox(width: 9),
              Container(
                padding: const EdgeInsets.symmetric(
                    horizontal: JeliyaSpacing.x8, vertical: JeliyaSpacing.x2),
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(JeliyaRadii.pill),
                  border: Border.all(color: chipBorder),
                ),
                child: Text(chipText,
                    style: TextStyle(fontSize: 11, color: chipColor)),
              ),
            ],
          ),
          const SizedBox(height: 5),
          templateText(
            s.panelPipeMeta('{openedBy}', '{authorized}'),
            style: TextStyle(fontSize: 12, color: tokens.textMute),
            slots: {
              'openedBy': widgetSlot(SenderName(
                id: pipe.openedBy,
                style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w500,
                    color: tokens.textDim),
              )),
              'authorized': authorizedPeer == null
                  ? TextSpan(
                      text: Tokens.emDash,
                      style: TextStyle(fontSize: 12, color: tokens.textMute))
                  : session.isSelf(authorizedPeer)
                      ? TextSpan(
                          text: s.commonYou,
                          style:
                              TextStyle(fontSize: 12, color: tokens.textDim))
                      : widgetSlot(SenderName(
                          id: authorizedPeer,
                          style: TextStyle(
                              fontSize: 12,
                              fontWeight: FontWeight.w500,
                              color: tokens.textDim),
                        )),
            },
          ),
          if (!closed) ...[
            const SizedBox(height: 9),
            Wrap(
              spacing: JeliyaSpacing.x8,
              runSpacing: JeliyaSpacing.x6,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                if (conn == null || conn.phase == PipeConnPhases.error)
                  JeliyaButton(
                    label: s.panelConnect,
                    size: JeliyaButtonSize.sm,
                    onPressed: () =>
                        unawaited(room?.connectPipe(pipe.pipeId)),
                  )
                else if (conn.phase == PipeConnPhases.connecting)
                  JeliyaButton(
                    label: s.panelConnecting,
                    size: JeliyaButtonSize.sm,
                    busy: true,
                    onPressed: null,
                  )
                else ...[
                  Container(
                    padding: const EdgeInsets.symmetric(
                        horizontal: JeliyaSpacing.x8,
                        vertical: JeliyaSpacing.x4),
                    decoration: BoxDecoration(
                      color: tokens.bgCard2,
                      borderRadius: BorderRadius.circular(JeliyaRadii.iconBtn),
                      border: Border.all(color: tokens.borderStrong),
                    ),
                    child: Text(
                      conn.localAddr ?? '',
                      style:
                          JeliyaText.mono(fontSize: 12, color: tokens.accent),
                    ),
                  ),
                  JeliyaButton(
                    label: s.panelOpenPreview,
                    size: JeliyaButtonSize.sm,
                    variant: JeliyaButtonVariant.primary,
                    onPressed: () => _launch('http://${conn.localAddr}'),
                  ),
                ],
                if (closing)
                  JeliyaButton(
                    label: s.panelClosingPipe,
                    size: JeliyaButtonSize.sm,
                    variant: JeliyaButtonVariant.ghost,
                    busy: true,
                    onPressed: null,
                  )
                else
                  JeliyaButton(
                    label: s.panelClosePipe,
                    size: JeliyaButtonSize.sm,
                    variant: JeliyaButtonVariant.ghost,
                    onPressed: () => unawaited(room?.closePipe(pipe.pipeId)),
                  ),
              ],
            ),
          ],
          if (conn?.phase == PipeConnPhases.error)
            ErrorNote(
              error: conn!.error,
              // pipe.connect's invalid_params means unknown pipe / owner
              // self-connect (PROTOCOL.md) — not a form-input problem.
              friendly: conn.error?.code == ErrorCodes.invalidParams
                  ? FriendlyError(
                      title: s.panelConnectInvalidTitle,
                      message: s.panelConnectInvalidMessage,
                      action: s.panelConnectInvalidAction,
                    )
                  : null,
            ),
        ],
      ),
    );
  }
}
