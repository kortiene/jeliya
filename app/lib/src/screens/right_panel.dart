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
import '../layout.dart';
import '../routes.dart';
import 'room_nav.dart';
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


// Clock formatting goes through ../format.dart context.formats clock — one
// consistent (and later locale-aware) clock for every surface.

// prettyLabel / extOf live in ../format.dart — the shared single home.

/// The room's tool surface — an inspector (docs/room-workbench.md,
/// decision 3). It renders the room destination the route names; it never
/// decides which one that is.
class RightPanel extends StatelessWidget {
  const RightPanel({
    super.key,
    required this.tab,
    required this.onDest,
    required this.onClose,
    required this.shell,
    required this.onLeaveRoom,
    this.roomName,
    this.chrome = true,
    this.touchTargets = false,
  });

  /// The tool to render — derived from the route, never held here.
  final RoomDest tab;

  /// Room-nav taps. This panel carries the strip whenever it covers the
  /// workspace (compact and medium); on wide the workspace keeps it.
  final ValueChanged<RoomDest> onDest;

  /// Close the inspector — which is navigating to the room's Activity, not a
  /// local visibility toggle.
  final VoidCallback onClose;

  final Shell shell;

  /// The open room's display name. Room context stays visible on every
  /// room-scoped surface (decision 3), and on compact this panel IS the
  /// surface — the room header is on another pane. Null on medium and wide,
  /// where the header is alongside and repeating it would be duplication.
  final String? roomName;

  /// Opens the Leave Room modal.
  final VoidCallback onLeaveRoom;

  /// Desktop pane chrome: the hairline left border separating the panel from
  /// the center column. The full-width mobile surfaces pass false — the web
  /// drops `border-left` below the breakpoint (styles.css `.app .right-panel`
  /// in the mobile media query); the raised ground stays either way.
  final bool chrome;

  /// Grow the compact interactive controls to the ~44dp platform touch floor
  /// (web parity: the mobile media query sets `.btn` and `.panel-tabs button`
  /// to min-height 44px). Desktop keeps its compact controls.
  final bool touchTargets;

  @override
  Widget build(BuildContext context) {
    final session = SessionScope.of(context);
    final tokens = JeliyaTokens.of(context);
    final room = session.room;
    return Container(
      decoration: BoxDecoration(
        color: tokens.bgRaise,
        border:
            chrome ? Border(left: BorderSide(color: tokens.border)) : null,
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
    final counts = roomNavCounts(room);

    final Widget body = switch (tab) {
      RoomDest.people => _MembersTab(
          members: members,
          session: session,
          onLeaveRoom: onLeaveRoom,
          touchTargets: touchTargets,
        ),
      RoomDest.agents => _AgentsTab(
          members: members,
          timeline: room?.timeline ?? const <TimelineEvent>[],
        ),
      RoomDest.files => _FilesTab(
          key: ValueKey('files-${room?.roomId}'),
          room: room,
          session: session,
          touchTargets: touchTargets,
        ),
      RoomDest.pipes => _PipesTab(
          key: ValueKey('pipes-${room?.roomId}'),
          room: room,
          session: session,
          touchTargets: touchTargets,
        ),
      // Activity is the workspace, not a tool: the shell closes this panel
      // for it rather than building an empty one.
      RoomDest.activity => const SizedBox.shrink(),
    };

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _InspectorHead(shell: shell, onClose: onClose, roomName: roomName),
        // The strip belongs to whoever covers the workspace. On wide the tool
        // opens BESIDE the workspace, so the workspace keeps the one strip and
        // this panel does not build a second: two strips for one room would be
        // two tablists in the semantics tree.
        if (shell != Shell.wide)
          RoomNav(dest: tab, counts: counts, onDest: onDest),
        Expanded(
          // role='tabpanel' labelled by the active tab.
          child: Semantics(
            container: true,
            label: roomDestLabel(s, tab),
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

// -- inspector head -----------------------------------------------------------------

/// The room stays named on every room-scoped surface (decision 3), but how it
/// is delivered differs: on compact this panel IS the screen, so it carries
/// the room name itself; on medium and wide the room header is alongside, so
/// repeating it would be duplication and this row is only the way out.
class _InspectorHead extends StatelessWidget {
  const _InspectorHead({
    required this.shell,
    required this.onClose,
    required this.roomName,
  });

  final Shell shell;
  final VoidCallback onClose;
  final String? roomName;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    // One navigation, two affordances. Compact needs a Back of its own: this
    // panel is the whole screen there and the bottom bar is gone, so without
    // it there would be no in-app way back to the room's Activity.
    final compact = shell == Shell.compact;
    return Padding(
      padding: const EdgeInsets.fromLTRB(
          JeliyaSpacing.x4, JeliyaSpacing.x4, JeliyaSpacing.x4, 0),
      child: Row(
        children: [
          if (compact)
            IconButton(
              onPressed: onClose,
              icon: const Text('\u2039',
                  style: TextStyle(fontSize: 22, height: 1)),
              tooltip: s.roomBackToActivity,
              constraints:
                  const BoxConstraints(minWidth: 44, minHeight: 44),
            ),
          Expanded(
            child: roomName == null
                ? const SizedBox.shrink()
                : Text(
                    roomName!,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 14,
                        fontWeight: FontWeight.w700,
                        color: tokens.text),
                  ),
          ),
          if (!compact)
            IconButton(
              onPressed: onClose,
              icon: const Text('\u00d7',
                  style: TextStyle(fontSize: 18, height: 1)),
              tooltip: s.roomCloseInspector,
              constraints:
                  const BoxConstraints(minWidth: 32, minHeight: 32),
            ),
        ],
      ),
    );
  }
}


class _TouchFloor extends StatelessWidget {
  const _TouchFloor({required this.on, required this.child});

  final bool on;
  final Widget child;

  @override
  Widget build(BuildContext context) => on
      ? ConstrainedBox(
          constraints: const BoxConstraints(minHeight: 44), child: child)
      : child;
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
      // Both sides flex loosely (the #14 lesson: fr copy runs ~2x English
      // and overflowed this row at 360dp when the summary was unbounded):
      // whenever the pair fits, spaceBetween renders it exactly as before —
      // label flush left, summary flush right; under pressure each side
      // ellipsizes at its share instead of overflowing.
      child: Row(
        mainAxisAlignment: MainAxisAlignment.spaceBetween,
        children: [
          Flexible(
            child: Text(
              title.toUpperCase(),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 12, letterSpacing: 0.72, color: tokens.textMute),
            ),
          ),
          const SizedBox(width: JeliyaSpacing.x8),
          Flexible(
            child: Text(summary,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                textAlign: TextAlign.end,
                style: TextStyle(fontSize: 11.5, color: tokens.textDim)),
          ),
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

/// Synthetic code marking an "empty file" share failure, so the ErrorNote
/// composes its friendly copy from the catalog at render time (shared with the
/// composer's own zero-byte guard).
const String _kEmptyFileCode = 'file_empty';

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
    this.touchTargets = false,
  });

  final List<Member> members;
  final DaemonSession session;
  final VoidCallback onLeaveRoom;
  final bool touchTargets;

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
    // Signed membership, counted — the roster's own fact. The word for it is
    // "Member" on every surface (decision 4); only the wire says `active`.
    final memberCount = members.where((m) => m.status == 'active').length;
    final invitedCount = members.where((m) => m.status == 'invited').length;
    final agentCount = members.where((m) => m.role == Roles.agent).length;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _buildSummary(context, memberCount, invitedCount, agentCount),
        const SizedBox(height: JeliyaSpacing.x10),
        _SectionHead(
          title: s.panelRoomRoster,
          summary: s.commonMemberCount(memberCount),
        ),
        for (final member in sorted) ...[
          const SizedBox(height: JeliyaSpacing.x10),
          _MemberRow(
              member: member,
              session: session,
              onLeaveRoom: onLeaveRoom,
              touchTargets: touchTargets),
        ],
      ],
    );
  }

  Widget _buildSummary(
      BuildContext context, int memberCount, int invited, int agents) {
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
                            term: s.panelStatMembers,
                            value: '$memberCount')),
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
    this.touchTargets = false,
  });

  final Member member;
  final DaemonSession session;
  final VoidCallback onLeaveRoom;
  final bool touchTargets;

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
                  // Wrap, not Row: at the inspector's narrowest width the self
                  // row's 'this device' chip does not fit beside the name next
                  // to the badges column — let it drop to a second line.
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
                    // inline it starved the name column at the inspector's
                    // narrowest width once French copy ('Le propriétaire
                    // reste') got ~2x wider than 'Owner stays'.
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
              _TouchFloor(
                on: touchTargets,
                child: JeliyaButton(
                  label: s.panelLeave,
                  size: JeliyaButtonSize.sm,
                  variant: JeliyaButtonVariant.danger,
                  onPressed: onLeaveRoom,
                ),
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
  const _FilesTab({
    super.key,
    required this.room,
    required this.session,
    this.touchTargets = false,
  });

  final RoomStore? room;
  final DaemonSession session;
  final bool touchTargets;

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
        if (selected.size == 0) {
          // A zero-byte pick was silently dropped before; say so inline and
          // record the miss without leaking the file's name or path.
          final err =
              widget.session.recordLocalFailure('file.share', _kEmptyFileCode);
          if (mounted) setState(() => _shareError = err);
        } else {
          await room.shareUserFile(selected.path,
              name: selected.name, mime: selected.mime);
          if (mounted) setState(() => _selected = null);
        }
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
                  _TouchFloor(
                    on: widget.touchTargets,
                    child: JeliyaButton(
                      label: s.panelChooseFile,
                      size: JeliyaButtonSize.sm,
                      variant: JeliyaButtonVariant.primary,
                      semanticLabel: s.panelChooseFileToShare,
                      onPressed: _sharing ? null : () => unawaited(_pick()),
                    ),
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
                touchTargets: widget.touchTargets,
                onClear: () => setState(() => _selected = null),
              ),
            )
          else
            Text(s.panelNoFileSelectedYet,
                style: TextStyle(fontSize: 12, color: tokens.textDim)),
          const SizedBox(height: JeliyaSpacing.x10),
          _TouchFloor(
            on: widget.touchTargets,
            child: JeliyaButton(
              label: _sharing ? s.panelSharing : s.panelShare,
              variant: JeliyaButtonVariant.primary,
              busy: _sharing,
              onPressed: canSubmit ? () => unawaited(_share()) : null,
            ),
          ),
          const SizedBox(height: JeliyaSpacing.x10),
          _AdvancedPath(
              controller: _path, touchTargets: widget.touchTargets),
          ErrorNote(
            error: _shareError,
            friendly: _shareError?.code == _kEmptyFileCode
                ? FriendlyError(
                    title: s.composerShareEmptyFileTitle,
                    message: s.composerShareEmptyFile,
                  )
                : null,
          ),
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
                      // The floor reaches the single-button states (Fetch /
                      // Fetching / Retry); the Wrap states keep their own
                      // compact children (FetchControl internals).
                      child: _TouchFloor(
                        on: widget.touchTargets,
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
  const _SelectedFileCard({
    required this.file,
    required this.onClear,
    this.touchTargets = false,
  });

  final _PickedFile file;
  final VoidCallback onClear;
  final bool touchTargets;

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
          _TouchFloor(
            on: touchTargets,
            child: JeliyaButton(
              label: s.panelClearSelectedFile,
              size: JeliyaButtonSize.sm,
              onPressed: onClear,
            ),
          ),
        ],
      ),
    );
  }
}

/// `<details>` 'Advanced: paste a daemon-readable path'.
class _AdvancedPath extends StatefulWidget {
  const _AdvancedPath({required this.controller, this.touchTargets = false});

  final TextEditingController controller;
  final bool touchTargets;

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
            child: Container(
              // Touch surfaces grow the text-only disclosure to the 44dp
              // floor, full row width; desktop keeps the compact inline text.
              constraints: widget.touchTargets
                  ? const BoxConstraints(minHeight: 44)
                  : null,
              alignment:
                  widget.touchTargets ? Alignment.centerLeft : null,
              child: Text(
                '${_open ? '▾' : '▸'} ${s.panelAdvancedPathSummary}',
                style: TextStyle(fontSize: 12, color: tokens.textDim),
              ),
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
  const _PipesTab({
    super.key,
    required this.room,
    required this.session,
    this.touchTargets = false,
  });

  final RoomStore? room;
  final DaemonSession session;
  final bool touchTargets;

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
          _PipeRow(
              pipe: pipe,
              room: widget.room,
              touchTargets: widget.touchTargets),
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
                      // Touch surfaces grow the bordered box to the 44dp
                      // floor; the min-height flows through to the dropdown
                      // itself, so the whole box stays tappable.
                      constraints: widget.touchTargets
                          ? const BoxConstraints(minHeight: 44)
                          : null,
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
                _TouchFloor(
                  on: widget.touchTargets,
                  child: JeliyaButton(
                    label:
                        _exposing ? s.panelExposing : s.panelExpose,
                    busy: _exposing,
                    onPressed: canSubmit ? () => unawaited(_expose()) : null,
                  ),
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
  const _PipeRow({
    required this.pipe,
    required this.room,
    this.touchTargets = false,
  });

  final PipeEntry pipe;
  final RoomStore? room;
  final bool touchTargets;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final closed = pipe.state == PipeStates.closed;
    final conn = room?.pipeConns[pipe.pipeId];
    final closing = room?.closingPipes.contains(pipe.pipeId) ?? false;
    final authorizedPeer = pipe.authorizedPeer;

    // Three facts, three words (decision 4): a pipe is exposed and forwarding
    // (**Connected**), exposed with nothing on the other end (**Open**), or
    // **Closed**. The forwarding state used to read "Active" — the same word
    // the rail used for a live session and the wire uses for membership.
    final (String chipText, Color chipColor, Color chipBorder) = closed
        ? (s.pipeStateClosed, tokens.textMute, tokens.borderStrong)
        : pipe.connected
            ? (s.pipeStateConnected, tokens.accent, tokens.accentLine)
            : (s.pipeStateOpen, tokens.accent, tokens.accentLine);

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
              // Self routes through SenderName too, so it shows the device-local
              // label (or 'You') like everyone else (docs/self-label.md).
              'authorized': authorizedPeer == null
                  ? TextSpan(
                      text: Tokens.emDash,
                      style: TextStyle(fontSize: 12, color: tokens.textMute))
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
                  _TouchFloor(
                    on: touchTargets,
                    child: JeliyaButton(
                      label: s.panelConnect,
                      size: JeliyaButtonSize.sm,
                      onPressed: () =>
                          unawaited(room?.connectPipe(pipe.pipeId)),
                    ),
                  )
                else if (conn.phase == PipeConnPhases.connecting)
                  _TouchFloor(
                    on: touchTargets,
                    child: JeliyaButton(
                      label: s.panelConnecting,
                      size: JeliyaButtonSize.sm,
                      busy: true,
                      onPressed: null,
                    ),
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
                  _TouchFloor(
                    on: touchTargets,
                    child: JeliyaButton(
                      label: s.panelOpenPreview,
                      size: JeliyaButtonSize.sm,
                      variant: JeliyaButtonVariant.primary,
                      onPressed: () => _launch('http://${conn.localAddr}'),
                    ),
                  ),
                ],
                if (closing)
                  _TouchFloor(
                    on: touchTargets,
                    child: JeliyaButton(
                      label: s.panelClosingPipe,
                      size: JeliyaButtonSize.sm,
                      variant: JeliyaButtonVariant.ghost,
                      busy: true,
                      onPressed: null,
                    ),
                  )
                else
                  _TouchFloor(
                    on: touchTargets,
                    child: JeliyaButton(
                      label: s.panelClosePipe,
                      size: JeliyaButtonSize.sm,
                      variant: JeliyaButtonVariant.ghost,
                      onPressed: () =>
                          unawaited(room?.closePipe(pipe.pipeId)),
                    ),
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
