/// The chat screen pushed onto the Rooms tab's nested navigator (issue #17):
/// back affordance + RoomHeader (its own <560 stack), the room-keyed
/// timeline, and the composer. Pushed by the shell (room selection,
/// create/join/fleet deep links) so every entry point shares one route
/// construction; Android back pops it for free. The room-detail route
/// (RightPanel tabs) is pushed from here for Members / Share file /
/// Open pipe / timeline pipe tiles — every desktop deep-link callback keeps
/// its intent (shell.dart parity).
library;

import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart' show RoomSummary;

import '../l10n/strings_context.dart';
import '../session/daemon_session.dart';
import '../theme.dart';
import '../widgets/error_note.dart';
import 'composer.dart';
import 'mobile_panel.dart' show mobileRoomDetailRoute;
import 'right_panel.dart' show PanelTab;
import 'room_header.dart';
import 'timeline.dart';

/// The chat route factory — the shell's one construction site.
Route<void> mobileRoomRoute({
  required VoidCallback onInvite,
  required VoidCallback onLeaveRoom,
}) =>
    MaterialPageRoute<void>(
      builder: (_) =>
          MobileRoomScreen(onInvite: onInvite, onLeaveRoom: onLeaveRoom),
    );

class MobileRoomScreen extends StatelessWidget {
  const MobileRoomScreen(
      {super.key, required this.onInvite, required this.onLeaveRoom});

  final VoidCallback onInvite;
  final VoidCallback onLeaveRoom;

  void _pushDetail(BuildContext context, PanelTab tab) {
    Navigator.of(context).push(
        mobileRoomDetailRoute(initialTab: tab, onLeaveRoom: onLeaveRoom));
  }

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final session = SessionScope.of(context);
    final tokens = JeliyaTokens.of(context);
    final room = session.room;
    if (room == null) {
      // The room closed under this route (left/removed elsewhere): an honest
      // empty state with the back affordance still in reach, on the same
      // raised header chrome as the live chat.
      return ColoredBox(
        color: tokens.bg,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Container(
              padding: const EdgeInsets.symmetric(vertical: JeliyaSpacing.x4),
              decoration: BoxDecoration(
                color: tokens.bgRaise,
                border: Border(bottom: BorderSide(color: tokens.border)),
              ),
              child: Align(
                alignment: Alignment.centerLeft,
                child: BackButton(color: tokens.text),
              ),
            ),
            Expanded(
              child: Center(
                child: Text(s.shellSelectRoom,
                    style: TextStyle(fontSize: 13.5, color: tokens.textDim)),
              ),
            ),
          ],
        ),
      );
    }
    RoomSummary? summary;
    for (final r in session.rooms) {
      if (r.roomId == room.roomId) summary = r;
    }
    return ColoredBox(
      color: tokens.bg,
      child: LayoutBuilder(
        builder: (context, viewport) => ListenableBuilder(
          listenable: room,
          builder: (context, _) => Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              // The header's wrapping actions + peer chips can outgrow a
              // short viewport (landscape phones, wide French copy, the soft
              // keyboard's inset) — the chrome must never squeeze the
              // timeline out or hard-overflow the column, so past ~45% of
              // the route the header scrolls internally instead.
              ConstrainedBox(
                constraints: BoxConstraints(
                    maxHeight: math.max(viewport.maxHeight * 0.45, 160)),
                child: SingleChildScrollView(
                  child: RoomHeader(
                    leading: BackButton(color: tokens.text),
                    name: summary?.name ?? s.shellUntitledRoom,
                    memberCount: room.members.isNotEmpty
                        ? room.members.length
                        : summary?.memberCount ?? 0,
                    onInvite: onInvite,
                    onMembers: () => _pushDetail(context, PanelTab.members),
                    onShareFile: () => _pushDetail(context, PanelTab.files),
                    onOpenPipe: () => _pushDetail(context, PanelTab.pipes),
                  ),
                ),
              ),
              if (room.openError != null)
                Padding(
                  padding: const EdgeInsets.symmetric(
                      horizontal: JeliyaSpacing.page),
                  child: ErrorNote(error: room.openError),
                ),
              // Keyed by room so the live-region/scroll state resets on
              // switch.
              Expanded(
                child: TimelineView(
                  key: ValueKey(room.roomId),
                  onShowPipes: () => _pushDetail(context, PanelTab.pipes),
                ),
              ),
              const Composer(),
            ],
          ),
        ),
      ),
    );
  }
}
