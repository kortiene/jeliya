/// The homonym disambiguator (docs/room-workbench.md, decision 6).
///
/// `room_id` is identity; `name` is a non-unique label. Wherever acting on the
/// wrong room matters, the short room id is shown next to the name so two rooms
/// that display the same name can be told apart. This renders it as a mono/code
/// element via the shared [shortId] helper — not a second id-shortening rule —
/// with an accessible label so a screen reader announces "Room ID {shortId}"
/// rather than a bare hash floating in the row's name.
library;

import 'package:flutter/material.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart' show shortId;

import '../l10n/strings_context.dart';
import '../theme.dart';

class RoomShortId extends StatelessWidget {
  const RoomShortId({super.key, required this.roomId, this.fontSize = 11.5});

  final String roomId;

  /// Matches the meta line it sits beside (11.5 in the room rows); the fleet
  /// chip passes its own smaller size.
  final double fontSize;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final short = shortId(roomId);
    // One clean semantics node: the visible mono span is excluded so the row's
    // accessible name reads "Room ID 4f2a…9c1b", not the raw truncation twice.
    return Semantics(
      label: s.roomShortIdLabel(short),
      child: ExcludeSemantics(
        child: Text(
          short,
          style: JeliyaText.mono(fontSize: fontSize, color: tokens.textMute),
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
      ),
    );
  }
}
