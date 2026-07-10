/// The phone Rooms home (issue #17): brand row, 'Your Rooms' list from
/// session.rooms, create/join affordances, and the identity footer with the
/// connection badge — the phone counterpart of the sidebar's rooms region
/// (mockups/mobile-triptych.png, layout reference only). Built fresh against
/// the session (the sidebar's row widgets are private and its nav rail is
/// redundant with the tab bar — web parity: styles.css hides .nav-list on
/// phones). Presentation only: every intent arrives as a shell callback; the
/// chat surface is a pushed route (mobile_room.dart).
library;

import 'package:flutter/material.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart' show RoomSummary;

import '../l10n/strings_context.dart';
import '../l10n/tokens.dart';
import '../session/daemon_session.dart';
import '../theme.dart';
import '../widgets/template_text.dart';
import '../widgets/tree_mark.dart';
import 'sidebar.dart' show IdentityFooter;

class MobileRoomsScreen extends StatelessWidget {
  const MobileRoomsScreen({
    super.key,
    required this.onSelectRoom,
    required this.onCreateRoom,
    required this.onJoinRoom,
  });

  /// Room-row taps; the shell guards departed rooms and pushes the chat.
  final ValueChanged<String> onSelectRoom;

  final VoidCallback onCreateRoom;
  final VoidCallback onJoinRoom;

  @override
  Widget build(BuildContext context) {
    final session = SessionScope.of(context);
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    return ColoredBox(
      color: tokens.bg,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const Padding(
            padding: EdgeInsets.fromLTRB(JeliyaSpacing.x18, JeliyaSpacing.x18,
                JeliyaSpacing.x18, JeliyaSpacing.x14),
            child: Row(
              children: [
                TreeMark(size: 30),
                SizedBox(width: JeliyaSpacing.x10),
                Wordmark(fontSize: 19),
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(JeliyaSpacing.x18,
                JeliyaSpacing.x4, JeliyaSpacing.x18, JeliyaSpacing.x8),
            child: Text(
              s.sidebarYourRooms.toUpperCase(),
              style: TextStyle(
                  fontSize: 11,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 1.32,
                  color: tokens.textMute),
            ),
          ),
          Expanded(
            child: session.rooms.isEmpty
                ? Padding(
                    padding: const EdgeInsets.all(JeliyaSpacing.x14),
                    child: Align(
                      alignment: Alignment.topLeft,
                      child: Text(s.sidebarNoRoomsYet,
                          style:
                              TextStyle(fontSize: 13, color: tokens.textDim)),
                    ),
                  )
                : Semantics(
                    container: true,
                    label: s.sidebarRoomsListLabel,
                    child: ListView.separated(
                      padding: const EdgeInsets.symmetric(
                          horizontal: JeliyaSpacing.x10),
                      itemCount: session.rooms.length,
                      separatorBuilder: (_, _) =>
                          const SizedBox(height: JeliyaSpacing.x4),
                      itemBuilder: (context, index) => _MobileRoomRow(
                        room: session.rooms[index],
                        selected: session.rooms[index].roomId ==
                            session.currentRoomId,
                        onSelectRoom: onSelectRoom,
                      ),
                    ),
                  ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(
                JeliyaSpacing.x10, JeliyaSpacing.x8, JeliyaSpacing.x10, 0),
            child: _MobileAffordanceRow(
              glyph: Tokens.sidebarCreateRoomGlyph,
              label: s.modalCreateRoom,
              emphasized: true,
              onTap: onCreateRoom,
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(JeliyaSpacing.x10,
                JeliyaSpacing.x8, JeliyaSpacing.x10, JeliyaSpacing.x8),
            child: _MobileAffordanceRow(
              glyph: Tokens.sidebarJoinRoomGlyph,
              label: s.modalJoinRoomTitle,
              emphasized: false,
              onTap: onJoinRoom,
            ),
          ),
          IdentityFooter(session: session),
        ],
      ),
    );
  }
}

/// One room row — the sidebar room-row anatomy (hex tile tinted by
/// colorForId, name, member/state meta, departed rows disabled and receded)
/// at phone width. The meta's state segment renders as dot + label (status
/// is never color alone): accent + glow only for an OPEN session (the
/// sidebar's earned session-open signal), muted otherwise. 52dp tall: over
/// the 44dp floor.
class _MobileRoomRow extends StatelessWidget {
  const _MobileRoomRow({
    required this.room,
    required this.selected,
    required this.onSelectRoom,
  });

  final RoomSummary room;
  final bool selected;
  final ValueChanged<String> onSelectRoom;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final tint = tokens.colorForId(room.roomId);
    final departed = room.status == 'left' || room.status == 'removed';
    final stateLabel = departed
        ? (room.status == 'left' ? s.sidebarStateLeft : s.sidebarStateRemoved)
        : room.open
            ? s.sidebarStateActive
            : s.sidebarStateIdle;

    Widget row = TextButton(
      onPressed: departed ? null : () => onSelectRoom(room.roomId),
      style: ButtonStyle(
        padding: const WidgetStatePropertyAll(EdgeInsets.symmetric(
            horizontal: JeliyaSpacing.x10, vertical: 9)),
        backgroundColor: WidgetStatePropertyAll(
            selected ? tokens.accentDim : Colors.transparent),
        overlayColor: const WidgetStatePropertyAll(Colors.transparent),
        minimumSize: const WidgetStatePropertyAll(Size.zero),
        tapTargetSize: MaterialTapTargetSize.shrinkWrap,
        shape: WidgetStatePropertyAll(RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(JeliyaRadii.row),
          side: BorderSide(
              color: selected ? tokens.accentLine : Colors.transparent),
        )),
      ),
      child: Row(
        children: [
          ExcludeSemantics(
            child: Container(
              width: 34,
              height: 34,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: tokens.tileBg(room.roomId),
                borderRadius: BorderRadius.circular(JeliyaRadii.btn),
              ),
              child: Text(Tokens.sidebarRoomHexGlyph,
                  style: TextStyle(fontSize: 18, color: tint)),
            ),
          ),
          const SizedBox(width: JeliyaSpacing.x10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(room.name ?? s.shellUntitledRoom,
                    style: JeliyaText.name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis),
                _metaLine(s, tokens, stateLabel, departed),
              ],
            ),
          ),
        ],
      ),
    );

    if (departed) {
      row = Tooltip(
        message: room.status == 'left'
            ? s.sidebarLeftRoomTitle
            : s.sidebarRemovedRoomTitle,
        child: Opacity(opacity: 0.62, child: row),
      );
    }
    return Semantics(selected: selected, child: row);
  }

  /// '{n} members · {state}' stays ONE translatable message
  /// (sidebarRoomMeta); the {state} slot is swapped for a dot + label
  /// segment via templateParts, so translations reorder freely and no
  /// sentence is assembled in the tree. The count segment truncates first —
  /// the state stays readable at any width.
  Widget _metaLine(
      AppStrings s, JeliyaTokens tokens, String stateLabel, bool departed) {
    final open = room.open && !departed;
    Widget state = Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Container(
          width: 7,
          height: 7,
          decoration: BoxDecoration(
            // Truthful hue: accent only for a log-proven open session; the
            // glow is the sidebar's session-open signal, earned the same way.
            color: open ? tokens.accent : tokens.textMute,
            shape: BoxShape.circle,
            boxShadow: open
                ? [
                    BoxShadow(
                        color: tokens.accent.withValues(alpha: 0.7),
                        blurRadius: 6),
                  ]
                : null,
          ),
        ),
        const SizedBox(width: 5),
        Text(stateLabel,
            style: JeliyaText.meta,
            maxLines: 1,
            overflow: TextOverflow.ellipsis),
      ],
    );
    if (open) {
      state = Tooltip(message: s.sidebarSessionOpen, child: state);
    }
    return Row(
      children: [
        for (final part
            in templateParts(s.sidebarRoomMeta(room.memberCount, '{state}')))
          if (part.slot == 'state')
            state
          else
            Flexible(
              // An unmatched slot renders its literal marker (fail-visible,
              // matching templateSpans' contract).
              child: Text(part.text ?? '{${part.slot}}',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: JeliyaText.meta),
            ),
      ],
    );
  }
}

/// Create/join entry rows (the sidebar affordance rows at phone width),
/// min 44dp tall (touch floor). Solid hairline borders — Border-Not-Shadow.
class _MobileAffordanceRow extends StatelessWidget {
  const _MobileAffordanceRow({
    required this.glyph,
    required this.label,
    required this.emphasized,
    required this.onTap,
  });

  final String glyph;
  final String label;

  /// The create row reads a step brighter than the join row (web parity).
  final bool emphasized;

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final fg = emphasized ? tokens.textDim : tokens.textMute;
    final borderColor = emphasized ? tokens.borderStrong : tokens.border;
    final radius = BorderRadius.circular(JeliyaRadii.row);
    return Semantics(
      button: true,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: onTap,
          borderRadius: radius,
          child: Container(
            constraints: const BoxConstraints(minHeight: 44),
            padding: const EdgeInsets.symmetric(
                horizontal: JeliyaSpacing.x12, vertical: JeliyaSpacing.x8),
            decoration: BoxDecoration(
              borderRadius: radius,
              border: Border.all(color: borderColor),
            ),
            child: Row(
              children: [
                ExcludeSemantics(
                  child:
                      Text(glyph, style: TextStyle(fontSize: 14, color: fg)),
                ),
                const SizedBox(width: JeliyaSpacing.x8),
                Expanded(
                  child: Text(label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 14, color: fg)),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
