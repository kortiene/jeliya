/// The compact shell — one pane at a time, below [kShellBreakpoint]
/// (docs/room-workbench.md, decision 3; web parity: the `pane-*` rules in
/// ui/src/styles.css and ui/src/components/MobileTabBar.tsx).
///
/// Which pane shows is DERIVED from the shell's route, never held here. That
/// is the whole point of the compact shell: a phone shows one thing, so the
/// question "which one" has exactly one answer, and it is the same answer the
/// rail and the inspector give on a desktop.
///
///   /rooms                    the rooms list
///   /rooms/:id/activity       the room: app bar + timeline + composer
///   /rooms/:id/`<tool>`       the room's tool, pushed over the room
///   /fleet                    the Agent Fleet
///   /settings                 Settings
///
/// - The bottom bar carries the three GLOBAL destinations, and only those.
///   Room destinations are nested navigation inside Rooms — they were never
///   bottom-bar tabs, and the ambiguity of pretending otherwise (a global
///   Files tab that was always secretly about one room, chosen elsewhere) is
///   what the record exists to remove.
/// - Inside a room the bar is GONE: the room's app bar replaces it, and the
///   ~72dp it stops reserving is what buys the timeline its height back on a
///   568dp phone.
/// - The panes are hidden, not unmounted (IndexedStack — the mobile analogue
///   of the desktop shell's Visibility maintainState), so opening and closing
///   a room tool preserves the timeline's scroll position. IndexedStack also
///   keeps its offstage children out of the semantics tree, so the room's nav
///   strip and the inspector's are never both live at once.
/// - Agent Fleet keeps FleetDashboard mounted (the IndexedStack retains its
///   search/filter/scroll); its FleetStore poll is gated on the pane being
///   active AND the app foregrounded, so it still never runs in the background.
/// - Settings reuses SettingsPanel (already a max-640 single column).
///
/// Presentation only: every intent arrives as a shell callback, and Back is
/// the shell's PopScope — there is no nested navigator here to hold a second
/// opinion about where the user is.
library;

import 'package:flutter/material.dart' hide ConnectionState;

import '../l10n/strings_context.dart';
import '../l10n/tokens.dart';
import '../routes.dart';
import '../session/daemon_session.dart';
import '../theme.dart';
import '../widgets/connection_banner.dart';
import 'fleet_dashboard.dart';
import 'mobile_panel.dart';
import 'mobile_room.dart';
import 'mobile_rooms.dart';
import 'settings_panel.dart';

/// The single pane the compact shell shows. Derived — so the bar, the pane,
/// and the room's tab strip cannot disagree, because there is nothing left for
/// them to disagree *with*.
enum _Pane { rooms, room, inspector, fleet, settings }

_Pane _paneOf(JeliyaRoute route) => switch (route) {
      GlobalRoute(dest: GlobalDest.rooms) => _Pane.rooms,
      GlobalRoute(dest: GlobalDest.fleet) => _Pane.fleet,
      GlobalRoute(dest: GlobalDest.settings) => _Pane.settings,
      RoomRoute(:final dest) =>
        dest == RoomDest.activity ? _Pane.room : _Pane.inspector,
    };

class MobileShell extends StatelessWidget {
  const MobileShell({
    super.key,
    required this.route,
    required this.onGlobal,
    required this.onSelectRoom,
    required this.onCreateRoom,
    required this.onJoinRoom,
    required this.onDest,
    required this.onBackToRooms,
    required this.onInvite,
    required this.onLeaveRoom,
  });

  /// The navigation state. Everything this widget renders is read off it.
  final JeliyaRoute route;

  /// Bottom-bar taps.
  final ValueChanged<GlobalDest> onGlobal;

  /// Room-row and fleet-card taps.
  final ValueChanged<String> onSelectRoom;

  final VoidCallback onCreateRoom;
  final VoidCallback onJoinRoom;

  /// Room-nav taps (Activity included — closing a tool is navigating to it).
  final ValueChanged<RoomDest> onDest;

  /// The room app bar's Back.
  final VoidCallback onBackToRooms;

  final VoidCallback onInvite;
  final VoidCallback onLeaveRoom;

  @override
  Widget build(BuildContext context) {
    final session = SessionScope.of(context);
    final tokens = JeliyaTokens.of(context);
    final pane = _paneOf(route);
    // The bar belongs to the global destinations. Inside a room the app bar
    // carries the room, so there is nothing for it to say.
    final showBar = route.roomId == null;

    return Scaffold(
      backgroundColor: tokens.bg,
      body: SafeArea(
        // The tab bar owns the bottom inset while it exists; without it these
        // panes owe themselves the reservation, or the composer sits under the
        // home indicator.
        bottom: !showBar,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // A row above the panes, not an overlay: it may not cover the room
            // app bar's Back or the first rows of a list. It renders itself
            // away while connected.
            ConnectionBanner(
                conn: session.conn, wsUrl: session.transportDescription),
            Expanded(
              // The children are in [_Pane] order — `pane.index` is the
              // selector, so a child added here needs its case there.
              child: IndexedStack(
                index: pane.index,
                children: [
                  MobileRoomsScreen(
                    currentRoomId: route.roomId,
                    onSelectRoom: onSelectRoom,
                    onCreateRoom: onCreateRoom,
                    onJoinRoom: onJoinRoom,
                  ),
                  MobileRoomScreen(
                    roomId: route.roomId,
                    onBack: onBackToRooms,
                    onInvite: onInvite,
                    onDest: onDest,
                  ),
                  MobileInspectorPane(
                    roomId: route.roomId,
                    // Offstage on every route that names no tool. People is
                    // simply the first one — nothing reads it there, and a
                    // remembered last tool would be route state kept twice.
                    dest: route.inspectorDest ?? RoomDest.people,
                    onDest: onDest,
                    onLeaveRoom: onLeaveRoom,
                  ),
                  // Always mounted — the IndexedStack retains its state, so its
                  // search, filter, and scroll survive leaving the pane; the
                  // `active` flag gates the poll instead of mount/unmount, so it
                  // still never runs off-pane or in the background (#69).
                  FleetDashboard(
                    onOpenRoom: onSelectRoom,
                    active: pane == _Pane.fleet,
                  ),
                  SettingsPanel(onCreateRoom: onCreateRoom),
                ],
              ),
            ),
          ],
        ),
      ),
      bottomNavigationBar: showBar
          ? MobileTabBar(active: route.activeGlobal, onNav: onGlobal)
          : null,
    );
  }
}

// -- tab bar -------------------------------------------------------------------------

/// 58dp bottom bar, three glyph+label tabs, active = accent TEXT only
/// (One Emerald Voice — no fills), with bottom/left/right safe-area padding
/// (DESIGN.md "Mobile tab bar"; ui/src/styles.css .tabbar). It sits under
/// the soft keyboard when one opens (the Scaffold body resizes above it).
/// 58dp is a MINIMUM, not a fixed height: at large accessibility font
/// scales (Android "largest" ≈ textScale 2.0) the scaled glyph+label
/// column needs ~85dp, and the a11y floor is that the user's font size
/// wins over the design height — so the bar grows to fit instead of
/// clamping the text or clipping it (a fixed 58dp overflowed by ~27px
/// at scale 2.0). At normal scales the column stays well under 58dp and
/// the bar renders its exact DESIGN.md height.
class MobileTabBar extends StatelessWidget {
  const MobileTabBar({super.key, required this.active, required this.onNav});

  /// The highlighted tab. A room route highlights Rooms — the workbench is
  /// somewhere you stand inside Rooms, never a fourth global destination.
  final GlobalDest active;

  final ValueChanged<GlobalDest> onNav;

  /// 58dp bar MIN height (--tabbar-h), before the bottom safe-area inset;
  /// large font scales grow the bar past it (see the class doc).
  static const double height = 58;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final entries = [
      (GlobalDest.rooms, Tokens.sidebarGlyphRooms, s.sidebarNavRooms),
      (GlobalDest.fleet, Tokens.sidebarGlyphAgents, s.sidebarNavFleet),
      (GlobalDest.settings, Tokens.sidebarGlyphSettings, s.sidebarNavSettings),
    ];
    return Container(
      decoration: BoxDecoration(
        color: tokens.bgRaise,
        border: Border(top: BorderSide(color: tokens.border)),
      ),
      child: SafeArea(
        top: false,
        child: Semantics(
          container: true,
          label: s.sidebarNavPrimaryLabel,
          // minHeight (not a fixed SizedBox) + IntrinsicHeight: the bar is
          // exactly [height] until the text-scaled tab columns' intrinsic
          // height exceeds it, then grows to the exact scaled need (no
          // linear height*scale guess). IntrinsicHeight also gives the
          // stretch Row a bounded height, so every tab stays a full-bar
          // touch target (>= 58dp >= the 44dp floor) instead of shrinking
          // to its content. The Scaffold accommodates the growth — the
          // body resizes above the taller bottomNavigationBar.
          child: ConstrainedBox(
            constraints: const BoxConstraints(minHeight: height),
            child: IntrinsicHeight(
              child: Material(
                color: Colors.transparent,
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    for (final (key, glyph, label) in entries)
                      Expanded(
                        child: _TabItem(
                          glyph: glyph,
                          label: label,
                          active: key == active,
                          onTap: () => onNav(key),
                        ),
                      ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _TabItem extends StatelessWidget {
  const _TabItem({
    required this.glyph,
    required this.label,
    required this.active,
    required this.onTap,
  });

  final String glyph;
  final String label;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    // Active = accent TEXT only; inactive tabs read text-mute (styles.css
    // .tab / .tab.active). Each tab spans width/5 x the full bar height
    // (>= 58dp) — over the 44dp touch floor at any phone width.
    final color = active ? tokens.accent : tokens.textMute;
    return Semantics(
      selected: active, // aria-current="page"
      button: true,
      child: InkWell(
        onTap: onTap,
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            ExcludeSemantics(
              child: Text(glyph, style: TextStyle(fontSize: 17, color: color)),
            ),
            const SizedBox(height: JeliyaSpacing.x2),
            Text(
              label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 10.5, color: color),
            ),
          ],
        ),
      ),
    );
  }
}

