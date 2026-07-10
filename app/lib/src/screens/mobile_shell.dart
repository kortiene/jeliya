/// Mobile bottom-tab shell (issue #17) — the below-[kShellBreakpoint] fork of
/// the app shell, one pane at a time behind a fixed 58dp five-tab bar
/// (Rooms / Agents / Pipes / Files / Settings, DESIGN.md "Mobile tab bar";
/// executable spec: ui/src/components/MobileTabBar.tsx + the styles.css
/// mv-* pane mapping):
///
/// - Rooms hosts a NESTED navigator: rooms list (mobile_rooms.dart) →
///   pushed chat route (mobile_room.dart: RoomHeader + Timeline + Composer)
///   → pushed room-detail route (mobile_panel.dart: the RightPanel tabs,
///   Members first). Chat is a sub-view of Rooms — the tab stays highlighted
///   while it is open (web `active === 'home'` rule) and the routes stay
///   mounted across tab switches (IndexedStack) so timeline scroll survives,
///   mirroring the desktop shell's Visibility contract.
/// - Agents mounts FleetDashboard ONLY while active (its FleetStore polls
///   every 4s and must never run in the background — web parity).
/// - Pipes/Files pin the room-scoped RightPanel full width to that panel
///   tab (mobile_panel.dart), with an honest select-a-room empty state when
///   no room is open.
/// - Settings reuses SettingsPanel (already a max-640 single column).
///
/// All state and handlers live in the shell screen (shared with the desktop
/// three-pane layout); this file is presentation only. Back policy
/// (PopScope): pushed routes first, then a non-Rooms tab returns to Rooms,
/// then the app exits.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter/services.dart' show SystemNavigator;
import 'package:jeliya_protocol/jeliya_protocol.dart' show ConnectionState;

import '../l10n/strings_context.dart';
import '../l10n/tokens.dart';
import '../session/daemon_session.dart';
import '../theme.dart';
import '../widgets/connection_banner.dart';
import 'fleet_dashboard.dart';
import 'mobile_panel.dart';
import 'mobile_rooms.dart';
import 'right_panel.dart';
import 'settings_panel.dart';
import 'sidebar.dart' show NavKey;

// The chat route is defined next to the rooms screen it is pushed over, and
// the room-detail route next to the panel surfaces it hosts; both are
// re-exported here so the shell keeps a single mobile-IA import seam.
export 'mobile_panel.dart' show mobileRoomDetailRoute;
export 'mobile_room.dart' show mobileRoomRoute;

/// The five bottom tabs, in bar order (MobileTabBar.tsx `TABS`).
const List<NavKey> _tabs = [
  NavKey.rooms,
  NavKey.agents,
  NavKey.pipes,
  NavKey.files,
  NavKey.settings,
];

/// Web rule (MobileTabBar.tsx): the chat view ('home') and 'calls' keep the
/// Rooms tab highlighted — chat is a sub-view of Rooms, never its own tab.
NavKey _foldNav(NavKey nav) => switch (nav) {
      NavKey.home || NavKey.calls => NavKey.rooms,
      _ => nav,
    };

class MobileShell extends StatelessWidget {
  const MobileShell({
    super.key,
    required this.activeNav,
    required this.onNav,
    required this.roomsNavigatorKey,
    required this.onSelectRoom,
    required this.onCreateRoom,
    required this.onJoinRoom,
    required this.onOpenRoomFromFleet,
    required this.onPanelTab,
    required this.onLeaveRoom,
  });

  /// The shell's last navigation intent ([_foldNav] maps it onto a tab).
  final NavKey activeNav;

  /// Bottom-tab taps — the shell's mobile-aware `navigate`.
  final ValueChanged<NavKey> onNav;

  /// The Rooms tab's nested navigator — owned by the shell so its handlers
  /// (create/join/fleet-open/leave) can push and pop the chat route.
  final GlobalKey<NavigatorState> roomsNavigatorKey;

  /// Room-row taps; the shell guards departed rooms and pushes the chat.
  final ValueChanged<String> onSelectRoom;

  final VoidCallback onCreateRoom;
  final VoidCallback onJoinRoom;

  /// Fleet card "Open room" — the shell ignores departed rooms.
  final ValueChanged<String> onOpenRoomFromFleet;

  /// RightPanel tab-strip taps on the pinned Pipes/Files surfaces — the
  /// shell translates them (pipes/files → bottom tabs, members/agents → the
  /// room-detail route).
  final ValueChanged<PanelTab> onPanelTab;

  final VoidCallback onLeaveRoom;

  @override
  Widget build(BuildContext context) {
    final session = SessionScope.of(context);
    final tokens = JeliyaTokens.of(context);
    final tab = _foldNav(activeNav);
    final index = _tabs.indexOf(tab);

    return PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, _) {
        if (didPop) return;
        // Dialogs and full-screen modal routes live on the root navigator
        // ABOVE this route, so the system back reaches here only with the
        // shell on top: close the Rooms tab's pushed routes first, then
        // return to Rooms from any other tab, then exit.
        final rooms = roomsNavigatorKey.currentState;
        if (rooms != null && rooms.canPop()) {
          rooms.pop();
          return;
        }
        if (tab != NavKey.rooms) {
          onNav(NavKey.rooms);
          return;
        }
        SystemNavigator.pop();
      },
      child: Scaffold(
        backgroundColor: tokens.bg,
        body: SafeArea(
          bottom: false, // the tab bar owns the bottom inset
          child: Stack(
            children: [
              Positioned.fill(
                // IndexedStack (not conditional mounting) is the mobile
                // analogue of the desktop shell's Visibility maintainState:
                // the Rooms stack — including a pushed chat route — stays
                // alive across tab switches, preserving timeline scroll.
                child: IndexedStack(
                  index: index,
                  children: [
                    Navigator(
                      key: roomsNavigatorKey,
                      onGenerateRoute: (settings) => MaterialPageRoute<void>(
                        settings: settings,
                        builder: (_) => MobileRoomsScreen(
                          onSelectRoom: onSelectRoom,
                          onCreateRoom: onCreateRoom,
                          onJoinRoom: onJoinRoom,
                        ),
                      ),
                    ),
                    // FleetDashboard mounts only while its tab is active —
                    // the FleetStore 4s poll loop must not run offstage.
                    if (tab == NavKey.agents)
                      FleetDashboard(onOpenRoom: onOpenRoomFromFleet)
                    else
                      const SizedBox.shrink(),
                    MobilePanelSurface(
                      tab: PanelTab.pipes,
                      onTab: onPanelTab,
                      onLeaveRoom: onLeaveRoom,
                    ),
                    MobilePanelSurface(
                      tab: PanelTab.files,
                      onTab: onPanelTab,
                      onLeaveRoom: onLeaveRoom,
                    ),
                    SettingsPanel(onCreateRoom: onCreateRoom),
                  ],
                ),
              ),
              // Connection banner above every mobile surface (desktop shell
              // parity) whenever conn != connected.
              if (session.conn != ConnectionState.connected)
                Positioned(
                  top: 0,
                  left: 0,
                  right: 0,
                  child: Center(
                    child: ConnectionBanner(
                      conn: session.conn,
                      wsUrl: session.transportDescription,
                    ),
                  ),
                ),
            ],
          ),
        ),
        bottomNavigationBar: MobileTabBar(active: tab, onNav: onNav),
      ),
    );
  }
}

// -- tab bar -------------------------------------------------------------------------

/// Fixed 58dp bottom bar, five glyph+label tabs, active = accent TEXT only
/// (One Emerald Voice — no fills), with bottom/left/right safe-area padding
/// (DESIGN.md "Mobile tab bar"; ui/src/styles.css .tabbar). It sits under
/// the soft keyboard when one opens (the Scaffold body resizes above it).
class MobileTabBar extends StatelessWidget {
  const MobileTabBar({super.key, required this.active, required this.onNav});

  /// The highlighted tab (already folded: chat/calls highlight Rooms).
  final NavKey active;

  final ValueChanged<NavKey> onNav;

  /// 58dp bar height (--tabbar-h), before the bottom safe-area inset.
  static const double height = 58;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final entries = [
      (NavKey.rooms, Tokens.sidebarGlyphRooms, s.sidebarNavRooms),
      (NavKey.agents, Tokens.sidebarGlyphAgents, s.sidebarNavAgents),
      (NavKey.pipes, Tokens.sidebarGlyphPipes, s.sidebarNavPipes),
      (NavKey.files, Tokens.sidebarGlyphFiles, s.sidebarNavFiles),
      (NavKey.settings, Tokens.sidebarGlyphSettings, s.sidebarNavSettings),
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
          child: SizedBox(
            height: height,
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
    // .tab / .tab.active). Each tab spans width/5 x 58dp — over the 44dp
    // touch floor at any phone width.
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

