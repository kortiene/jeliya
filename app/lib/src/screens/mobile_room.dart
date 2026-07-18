/// The compact shell's ROOM pane — the room's Activity destination: its app
/// bar, its nav strip, the room-keyed timeline, and the composer
/// (docs/room-workbench.md, decision 3).
///
/// It is a pane, not a pushed route. It used to be one, and the Navigator
/// stack under it was a second answer to "where is the user" that could
/// disagree with the shell's own: a tab switch could leave a chat route
/// mounted under a different tab, and the fix was a set of pops and
/// popUntils at every entry point. The route decides which pane shows; this
/// widget renders the room and nothing else decides anything.
library;

import 'package:flutter/material.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart' show RoomSummary;

import '../l10n/strings_context.dart';
import '../routes.dart';
import '../session/daemon_session.dart';
import '../theme.dart';
import '../widgets/buttons.dart';
import '../widgets/error_note.dart';
import 'composer.dart';
import 'room_header.dart';
import 'room_nav.dart';
import 'timeline.dart';

/// Room-scoped content with no room open says so, instead of rendering an
/// empty room (decision 5: an empty state and "we have not asked yet" are
/// different sentences, and neither is "you are not in a room").
class RoomPaneEmpty extends StatelessWidget {
  const RoomPaneEmpty({super.key});

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

/// The open room's `room.list` row — it carries the name, the short id, and
/// the session's Open/Closed fact; the [RoomStore] carries none of them.
RoomSummary? roomSummaryOf(DaemonSession session, String? roomId) {
  for (final r in session.rooms) {
    if (r.roomId == roomId) return r;
  }
  return null;
}

/// The compact room pane when the route names a room this device has no live
/// session for — a joined-then-left archive a reconnect closed, or a row no
/// longer in `room.list`. Inside a room route the bottom bar is gone
/// (MobileShell hides it whenever `route.roomId != null`), so a bare empty
/// state here would strand the user with no visible way out. State the fact —
/// the signed departure when the roster proves one, otherwise the plain "no
/// room here" — and always offer Back to Rooms, the one destination that is
/// always a way back (docs/room-workbench.md, decision 2: an unreachable room
/// resolves to a recoverable state, and Rooms is the recovery).
class RoomPaneUnavailable extends StatelessWidget {
  const RoomPaneUnavailable({
    super.key,
    required this.roomId,
    required this.onBack,
  });

  final String? roomId;
  final VoidCallback onBack;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final session = SessionScope.of(context);
    final summary = roomSummaryOf(session, roomId);
    final title = switch (summary?.status) {
      'left' => s.sidebarLeftRoomTitle,
      'removed' => s.sidebarRemovedRoomTitle,
      _ => s.shellSelectRoom,
    };
    return ColoredBox(
      color: tokens.bg,
      child: Center(
        child: Padding(
          padding: const EdgeInsets.all(JeliyaSpacing.page),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                title,
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: 13.5, color: tokens.textDim),
              ),
              const SizedBox(height: JeliyaSpacing.x12),
              ConstrainedBox(
                constraints: const BoxConstraints(minHeight: 44),
                child: JeliyaButton(
                  label: s.roomBackToRooms,
                  semanticLabel: s.roomBackToRooms,
                  onPressed: onBack,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class MobileRoomScreen extends StatelessWidget {
  const MobileRoomScreen({
    super.key,
    required this.roomId,
    required this.onBack,
    required this.onInvite,
    required this.onDest,
    required this.onShowFiles,
    required this.onShowPipes,
  });

  /// The room the route names. The pane renders THIS room or none: a store
  /// whose id has moved on belongs to a different room, and drawing it under
  /// this route's name is the disagreement the route model exists to prevent.
  final String? roomId;

  final VoidCallback onBack;
  final VoidCallback onInvite;
  final ValueChanged<RoomDest> onDest;

  /// Timeline 'Open in Files' / 'Open in Pipes' — deep-link into the tool AND
  /// the file/pipe the tile refers to (#67).
  final ValueChanged<String> onShowFiles;
  final ValueChanged<String?> onShowPipes;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final session = SessionScope.of(context);
    final tokens = JeliyaTokens.of(context);
    final room = session.room;
    if (roomId == null || room == null || room.roomId != roomId) {
      // The bottom bar is hidden on this route, so the empty pane must carry
      // its own way back (finding: an empty room pane on compact had none).
      return RoomPaneUnavailable(roomId: roomId, onBack: onBack);
    }
    final summary = roomSummaryOf(session, room.roomId);
    return ColoredBox(
      color: tokens.bg,
      child: ListenableBuilder(
        listenable: room,
        builder: (context, _) => Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // The app bar is one non-wrapping row and the strip is one more,
            // so the chrome above the timeline is bounded by construction —
            // no viewport-fraction cap, no internal scroll. The old header
            // needed both: its wrapping action row and peer-chip strip could
            // outgrow a landscape phone or a keyboard-shrunk viewport, and at
            // 360x640 in French it hard-overflowed the column by 14px. The
            // peer chips now live behind the app bar's ⋮ disclosure, which is
            // what made the bound possible.
            RoomHeader(
              name: summary?.name ?? s.shellUntitledRoom,
              summary: summary,
              compact: true,
              onBack: onBack,
              onInvite: onInvite,
              onShareFile: () => onDest(RoomDest.files),
              onOpenPipe: () => onDest(RoomDest.pipes),
            ),
            // This pane IS Activity — the shell shows a different one for
            // every other room destination — so the strip marks Activity, and
            // the strip is how the other four are reached from here.
            RoomNav(
              dest: RoomDest.activity,
              counts: roomNavCounts(room),
              onDest: onDest,
            ),
            if (room.openError != null)
              Padding(
                padding:
                    const EdgeInsets.symmetric(horizontal: JeliyaSpacing.page),
                child: ErrorNote(error: room.openError),
              ),
            // Keyed by room so the live-region/scroll state resets on switch.
            Expanded(
              child: TimelineView(
                key: ValueKey(room.roomId),
                onShowPipes: onShowPipes,
                onShowFiles: onShowFiles,
              ),
            ),
            const Composer(),
          ],
        ),
      ),
    );
  }
}
