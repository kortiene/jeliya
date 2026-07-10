/// RoomHeader — ported from ui/src/components/RoomHeader.tsx per
/// phase3-features.json "RoomHeader": h1 room name, honest subtitle
/// ('{n} active | {n} agent(s) | {n} invite(s) pending | P2P badge'), the
/// three-state P2P badge (Alone / Peer-to-Peer / Relay only — NEVER invented
/// presence, P4), the action buttons, and the peer chip strip showing only
/// proven connection state + path.
library;

import 'package:flutter/material.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart'
    show Member, PeerPaths, PeerStates, PeerStatus, Roles, shortId;

import '../l10n/strings_context.dart';
import '../l10n/tokens.dart';
import '../l10n/wire_display.dart';
import '../session/daemon_session.dart';
import '../theme.dart';
import '../widgets/buttons.dart';

/// Wire value of an active membership (`Member.status`; no model constant).
const String _statusActive = 'active';

/// Wire value of a pending invite (`Member.status`).
const String _statusInvited = 'invited';

class RoomHeader extends StatelessWidget {
  const RoomHeader({
    super.key,
    required this.name,
    required this.memberCount,
    required this.onInvite,
    required this.onShareFile,
    required this.onOpenPipe,
    this.onMembers,
    this.leading,
  });

  /// Room display name ('Untitled room' fallback supplied by the shell).
  final String name;

  /// RoomSummary.memberCount — used until members have loaded.
  final int memberCount;

  final VoidCallback onInvite;

  /// Switches the right panel to the Files tab.
  final VoidCallback onShareFile;

  /// Switches the right panel to the Pipes tab.
  final VoidCallback onOpenPipe;

  /// Mobile-only members affordance: opens the room-detail Members tab
  /// (desktop keeps Members in the always-visible right panel and passes
  /// null, rendering exactly the reference header).
  final VoidCallback? onMembers;

  /// Mobile-only slot before the title (the chat route's back affordance);
  /// null on desktop.
  final Widget? leading;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final session = SessionScope.of(context);
    final room = session.room;
    final members = room?.members ?? const <Member>[];
    final peers = room?.peers ?? const <PeerStatus>[];

    final activeCount = members.isNotEmpty
        ? members.where((m) => m.status == _statusActive).length
        : memberCount;
    final invitedCount =
        members.where((m) => m.status == _statusInvited).length;
    final agentCount = members.where((m) => m.role == Roles.agent).length;

    return Container(
      padding: const EdgeInsets.fromLTRB(
          JeliyaSpacing.page, JeliyaSpacing.x14, JeliyaSpacing.page, JeliyaSpacing.x10),
      decoration: BoxDecoration(
        color: tokens.bgRaise,
        border: Border(bottom: BorderSide(color: tokens.border)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          LayoutBuilder(builder: (context, constraints) {
            Widget title = Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  name,
                  style: JeliyaText.roomTitle,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
                const SizedBox(height: JeliyaSpacing.x2),
                _Subtitle(
                  activeCount: activeCount,
                  agentCount: agentCount,
                  invitedCount: invitedCount,
                  peers: peers,
                ),
              ],
            );
            final leading = this.leading;
            if (leading != null) {
              title = Row(
                children: [
                  leading,
                  const SizedBox(width: JeliyaSpacing.x4),
                  Expanded(child: title),
                ],
              );
            }
            final onMembers = this.onMembers;
            final actions = Wrap(
              spacing: JeliyaSpacing.x8,
              runSpacing: JeliyaSpacing.x8,
              children: [
                if (onMembers != null)
                  JeliyaButton(
                    label: s.panelTabMembers,
                    semanticLabel: s.panelTabMembers,
                    onPressed: onMembers,
                  ),
                JeliyaButton(
                  label:
                      '${Tokens.roomHeaderShareFileGlyph} ${s.roomHeaderShareFile}',
                  semanticLabel: s.roomHeaderShareFile,
                  onPressed: onShareFile,
                ),
                JeliyaButton(
                  label:
                      '${Tokens.roomHeaderOpenPipeGlyph} ${s.roomHeaderOpenPipe}',
                  semanticLabel: s.roomHeaderOpenPipe,
                  onPressed: onOpenPipe,
                ),
                JeliyaButton(
                  label:
                      '${Tokens.roomHeaderInviteGlyph} ${s.roomHeaderInvite}',
                  semanticLabel: s.roomHeaderInvite,
                  variant: JeliyaButtonVariant.primary,
                  onPressed: onInvite,
                ),
              ],
            );
            // A Wrap inside a Row can never actually wrap; at the 960px
            // minimum window the center column is too narrow for title +
            // three buttons on one line, so stack and let the Wrap run.
            if (constraints.maxWidth < 560) {
              return Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  title,
                  const SizedBox(height: JeliyaSpacing.x8),
                  actions,
                ],
              );
            }
            return Row(
              crossAxisAlignment: CrossAxisAlignment.center,
              children: [
                Expanded(child: title),
                const SizedBox(width: JeliyaSpacing.x12),
                actions,
              ],
            );
          }),
          if (peers.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: JeliyaSpacing.x10),
              child: Semantics(
                container: true,
                label: s.roomHeaderPeerConnections,
                child: Wrap(
                  spacing: JeliyaSpacing.x6,
                  runSpacing: JeliyaSpacing.x6,
                  children: [for (final p in peers) _PeerChip(peer: p)],
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// '{n} active | {n} agent(s) | {n} invite(s) pending | P2P badge' — the
/// agent/invite segments render only when non-zero.
class _Subtitle extends StatelessWidget {
  const _Subtitle({
    required this.activeCount,
    required this.agentCount,
    required this.invitedCount,
    required this.peers,
  });

  final int activeCount;
  final int agentCount;
  final int invitedCount;
  final List<PeerStatus> peers;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final base = TextStyle(fontSize: 12.5, color: tokens.textDim);
    final sep = Text(Tokens.roomHeaderSeparator,
        style: TextStyle(fontSize: 12.5, color: tokens.textMute));

    return Wrap(
      spacing: JeliyaSpacing.x8,
      runSpacing: JeliyaSpacing.x4,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        Text(s.roomHeaderActiveCount(activeCount), style: base),
        if (agentCount > 0) ...[
          sep,
          Text(s.roomHeaderAgentCount(agentCount), style: base),
        ],
        if (invitedCount > 0) ...[
          sep,
          Text(
            s.roomHeaderInvitesPending(invitedCount),
            // pending-invites reads amber (a truthful "not yet" state).
            style: TextStyle(fontSize: 12.5, color: tokens.amber),
          ),
        ],
        sep,
        _P2pBadge(peers: peers),
      ],
    );
  }
}

/// Three honest states: nobody here, a live direct link, or relay-only.
/// Peers that are merely connecting/offline read as the same "alone" state —
/// there's no live link to call peer-to-peer yet either way (P4).
class _P2pBadge extends StatelessWidget {
  const _P2pBadge({required this.peers});

  final List<PeerStatus> peers;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final connected =
        peers.where((p) => p.state == PeerStates.connected).toList();
    final hasDirect = connected.any((p) => p.path == PeerPaths.direct);

    final (Color dotColor, bool glow, String label) = peers.isEmpty
        ? (tokens.textMute, false, s.roomHeaderAloneInRoom)
        : hasDirect
            ? (tokens.accent, true, s.roomHeaderPeerToPeer)
            : connected.isNotEmpty
                ? (tokens.amber, false, s.roomHeaderRelayOnly)
                : (tokens.textMute, false, s.roomHeaderAloneInRoom);

    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        _Dot(color: dotColor, glow: glow),
        const SizedBox(width: 5),
        // .p2p-badge colors the label accent in every state; only the dot
        // carries the neutral/green/amber truth.
        Text(label, style: TextStyle(fontSize: 12.5, color: tokens.accent)),
      ],
    );
  }
}

/// One peer: dot + display name + state label — path and state exactly as
/// reported by the daemon; relay fallback is never hidden (honesty rule).
class _PeerChip extends StatelessWidget {
  const _PeerChip({required this.peer});

  final PeerStatus peer;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final session = SessionScope.of(context);

    final connected = peer.state == PeerStates.connected;
    final connecting = peer.state == PeerStates.connecting;

    // Chip fg/border per peer-{state} peer-path-{path|none} (styles.css).
    final (Color fg, Color borderColor, Color stateColor) = connected
        ? switch (peer.path) {
            PeerPaths.direct => (tokens.accent, tokens.accentLine, tokens.accent),
            PeerPaths.relay => (tokens.amber, tokens.amberLine, tokens.amber),
            _ => (tokens.textDim, tokens.borderStrong, tokens.textMute),
          }
        : connecting
            ? (tokens.blue, tokens.blueLine, tokens.textMute)
            : (tokens.textMute, tokens.borderStrong, tokens.textMute);

    final path = peer.path;
    final stateLabel = connected
        ? (path != null
            ? s.peerPath(path)
            : s.roomHeaderPeerStateConnected)
        : connecting
            ? s.roomHeaderPeerStateConnecting
            : s.roomHeaderPeerStateOffline;

    // identity_id is only known once the SDK has bound the device (on admit);
    // fall back to the raw endpoint id until then. Full hex in the tooltip.
    final identityId = peer.identityId;
    final display = identityId != null
        ? session.displayName(s, identityId)
        : shortId(peer.endpointId);

    final offline = !connected && !connecting;
    Widget dot = _Dot(color: fg, glow: false);
    if (connecting) dot = _PulsingDot(color: fg);
    // Offline recedes via a dimmed dot only — text keeps full token contrast.
    if (offline) dot = Opacity(opacity: 0.5, child: dot);

    return Tooltip(
      message: peer.endpointId,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 2),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(JeliyaRadii.pill),
          border: Border.all(color: borderColor),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            dot,
            const SizedBox(width: 5),
            Text(display, style: JeliyaText.mono(fontSize: 11, color: fg)),
            const SizedBox(width: 5),
            Text(stateLabel,
                style: JeliyaText.mono(fontSize: 11, color: stateColor)),
          ],
        ),
      ),
    );
  }
}

/// The 7px status dot; glow only for the earned live (direct P2P) state.
class _Dot extends StatelessWidget {
  const _Dot({required this.color, required this.glow});

  final Color color;
  final bool glow;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 7,
      height: 7,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        color: color,
        boxShadow: glow
            ? [
                BoxShadow(
                  color: color.withValues(alpha: 0.7),
                  blurRadius: 6,
                ),
              ]
            : null,
      ),
    );
  }
}

/// Connecting-state dot: 1.1s opacity pulse; static 0.7-opacity when the
/// platform asks for reduced motion (state stays legible via the label).
class _PulsingDot extends StatefulWidget {
  const _PulsingDot({required this.color});

  final Color color;

  @override
  State<_PulsingDot> createState() => _PulsingDotState();
}

class _PulsingDotState extends State<_PulsingDot>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 1100),
  )..repeat(reverse: true);

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (MediaQuery.of(context).disableAnimations) {
      return Opacity(
        opacity: 0.7,
        child: _Dot(color: widget.color, glow: false),
      );
    }
    return FadeTransition(
      opacity: Tween<double>(begin: 1, end: 0.25).animate(
        CurvedAnimation(parent: _controller, curve: Curves.easeInOut),
      ),
      child: _Dot(color: widget.color, glow: false),
    );
  }
}
