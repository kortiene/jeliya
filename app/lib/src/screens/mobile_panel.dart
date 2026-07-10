/// Mobile hosts for the room-scoped [RightPanel] (issue #17): the pinned
/// Pipes/Files bottom-tab surfaces and the pushed room-detail route.
///
/// Both render the panel FULL WIDTH without its desktop pane chrome (the web
/// drops `border-left` below the breakpoint) and with every compact control
/// grown to the 44dp touch floor (styles.css mobile media query). The pinned
/// surfaces carry a slim room-name strip — the RoomHeader lives on the
/// hidden chat route, so this is the only room label a phone user gets there
/// (styles.css `.app .panel-room-context`) — and show an honest
/// select-a-room empty state when no room is open.
library;

import 'package:flutter/material.dart';

import '../l10n/strings_context.dart';
import '../session/daemon_session.dart';
import '../theme.dart';
import 'right_panel.dart';

/// The room-detail screen (RightPanel tabs) pushed onto the Rooms tab's
/// nested navigator — the mobile home of Members/Agents and the in-room
/// deep-link target for Share file / Open pipe / timeline pipe tiles.
Route<void> mobileRoomDetailRoute({
  required PanelTab initialTab,
  required VoidCallback onLeaveRoom,
}) =>
    MaterialPageRoute<void>(
      builder: (_) => _MobileRoomDetailScreen(
          initialTab: initialTab, onLeaveRoom: onLeaveRoom),
    );

/// The open room's display name — [RoomSummary] carries names; the
/// [RoomStore] itself does not.
String? _roomName(DaemonSession session, String roomId) {
  for (final r in session.rooms) {
    if (r.roomId == roomId) return r.name;
  }
  return null;
}

/// The desktop center column's 'Select a room' empty state at phone width:
/// room-scoped content with no room open says so instead of rendering an
/// empty panel.
class _SelectRoomEmpty extends StatelessWidget {
  const _SelectRoomEmpty();

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    return ColoredBox(
      color: tokens.bg,
      child: Center(
        child: Text(s.shellSelectRoom,
            style: TextStyle(fontSize: 13.5, color: tokens.textDim)),
      ),
    );
  }
}

// -- pinned Pipes / Files tab surfaces ---------------------------------------------------

/// The room-scoped [RightPanel] pinned full width to one panel tab (web
/// mv-pipes / mv-files parity). Tab-strip taps go through [onTab] so the
/// shell can translate them (pipes/files → their bottom tabs, members/agents
/// → the room-detail route on the Rooms tab).
class MobilePanelSurface extends StatelessWidget {
  const MobilePanelSurface({
    super.key,
    required this.tab,
    required this.onTab,
    required this.onLeaveRoom,
  });

  /// The panel tab this surface is pinned to (pipes or files).
  final PanelTab tab;

  final ValueChanged<PanelTab> onTab;
  final VoidCallback onLeaveRoom;

  @override
  Widget build(BuildContext context) {
    final session = SessionScope.of(context);
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final room = session.room;
    if (room == null) return const _SelectRoomEmpty();
    return ColoredBox(
      color: tokens.bgRaise,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // `.panel-room-context`: same weight/truncation as the room title,
          // sized for a slim strip (10px 16px 0).
          Padding(
            padding: const EdgeInsets.fromLTRB(
                JeliyaSpacing.x16, JeliyaSpacing.x10, JeliyaSpacing.x16, 0),
            child: Text(
              _roomName(session, room.roomId) ?? s.shellUntitledRoom,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 14,
                  fontWeight: FontWeight.w700,
                  color: tokens.text),
            ),
          ),
          Expanded(
            child: RightPanel(
              tab: tab,
              onTab: onTab,
              onLeaveRoom: onLeaveRoom,
              chrome: false,
              touchTargets: true,
            ),
          ),
        ],
      ),
    );
  }
}

// -- room-detail route -------------------------------------------------------------------

/// Room detail: back affordance + room name over the full-width RightPanel
/// (Members / Agents / Files / Pipes) with locally-owned tab state.
class _MobileRoomDetailScreen extends StatefulWidget {
  const _MobileRoomDetailScreen({
    required this.initialTab,
    required this.onLeaveRoom,
  });

  final PanelTab initialTab;
  final VoidCallback onLeaveRoom;

  @override
  State<_MobileRoomDetailScreen> createState() =>
      _MobileRoomDetailScreenState();
}

class _MobileRoomDetailScreenState extends State<_MobileRoomDetailScreen> {
  late PanelTab _tab = widget.initialTab;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final session = SessionScope.of(context);
    final tokens = JeliyaTokens.of(context);
    final room = session.room;
    return ColoredBox(
      color: tokens.bg,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(
            padding: const EdgeInsets.fromLTRB(JeliyaSpacing.x4,
                JeliyaSpacing.x4, JeliyaSpacing.x14, JeliyaSpacing.x4),
            decoration: BoxDecoration(
              color: tokens.bgRaise,
              border: Border(bottom: BorderSide(color: tokens.border)),
            ),
            child: Row(
              children: [
                BackButton(color: tokens.text),
                Expanded(
                  child: Text(
                    room == null
                        ? s.shellSelectRoom
                        : (_roomName(session, room.roomId) ??
                            s.shellUntitledRoom),
                    style: JeliyaText.cardTitle,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
              ],
            ),
          ),
          Expanded(
            // The room closed under this route (left/removed elsewhere): the
            // honest empty state, with the back affordance still in reach.
            child: room == null
                ? const _SelectRoomEmpty()
                : RightPanel(
                    tab: _tab,
                    onTab: (tab) => setState(() => _tab = tab),
                    onLeaveRoom: widget.onLeaveRoom,
                    chrome: false,
                    touchTargets: true,
                  ),
          ),
        ],
      ),
    );
  }
}
