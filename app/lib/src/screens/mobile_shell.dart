/// Mobile bottom-tab shell (issue #17) — the below-[kShellBreakpoint] fork of
/// the app shell, one pane at a time behind a fixed 58dp five-tab bar
/// (Rooms / Agents / Pipes / Files / Settings, DESIGN.md "Mobile tab bar";
/// executable spec: ui/src/components/MobileTabBar.tsx + the styles.css
/// mv-* pane mapping):
///
/// - Rooms hosts a NESTED navigator: rooms list → pushed chat route
///   (RoomHeader + Timeline + Composer) → pushed room-detail route (the
///   RightPanel tabs, Members first). Chat is a sub-view of Rooms — the tab
///   stays highlighted while it is open (web `active === 'home'` rule) and
///   the routes stay mounted across tab switches (IndexedStack) so timeline
///   scroll survives, mirroring the desktop shell's Visibility contract.
/// - Agents mounts FleetDashboard ONLY while active (its FleetStore polls
///   every 4s and must never run in the background — web parity).
/// - Pipes/Files pin the room-scoped RightPanel full width to that panel
///   tab, with an honest select-a-room empty state when no room is open.
/// - Settings reuses SettingsPanel (already a max-640 single column).
///
/// All state and handlers live in the shell screen (shared with the desktop
/// three-pane layout); this file is presentation only. Back policy
/// (PopScope): pushed routes first, then a non-Rooms tab returns to Rooms,
/// then the app exits.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter/services.dart' show SystemNavigator;
import 'package:jeliya_protocol/jeliya_protocol.dart'
    show ConnectionState, RoomSummary;

import '../l10n/strings_context.dart';
import '../l10n/tokens.dart';
import '../session/daemon_session.dart';
import '../theme.dart';
import '../widgets/connection_banner.dart';
import '../widgets/error_note.dart';
import '../widgets/tree_mark.dart';
import 'composer.dart';
import 'fleet_dashboard.dart';
import 'right_panel.dart';
import 'room_header.dart';
import 'settings_panel.dart';
import 'sidebar.dart' show IdentityFooter, NavKey;
import 'timeline.dart';

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

/// The chat screen pushed onto the Rooms tab's nested navigator. Pushed by
/// the shell (room selection, create/join/fleet deep links) so every entry
/// point shares one route construction.
Route<void> mobileRoomRoute({
  required VoidCallback onInvite,
  required VoidCallback onLeaveRoom,
}) =>
    MaterialPageRoute<void>(
      builder: (_) =>
          _MobileRoomScreen(onInvite: onInvite, onLeaveRoom: onLeaveRoom),
    );

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
                        builder: (_) => _MobileRoomsScreen(
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
                    _PanelSurface(
                      tab: PanelTab.pipes,
                      onTab: onPanelTab,
                      onLeaveRoom: onLeaveRoom,
                    ),
                    _PanelSurface(
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

// -- rooms tab: the list screen -------------------------------------------------------

/// The phone Rooms home: brand row, 'Your Rooms' list from session.rooms,
/// create/join affordances, and the identity footer with the connection
/// badge. Built fresh against the session (the sidebar's row widgets are
/// private and its nav rail is redundant with the tab bar — web parity:
/// styles.css hides .nav-list on phones).
class _MobileRoomsScreen extends StatelessWidget {
  const _MobileRoomsScreen({
    required this.onSelectRoom,
    required this.onCreateRoom,
    required this.onJoinRoom,
  });

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
/// colorForId, name, member/state meta, session-open dot, departed rows
/// disabled and receded) at phone width. 52dp tall: over the 44dp floor.
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
                Text(s.sidebarRoomMeta(room.memberCount, stateLabel),
                    style: JeliyaText.meta,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis),
              ],
            ),
          ),
          if (room.open) ...[
            const SizedBox(width: JeliyaSpacing.x6),
            Tooltip(
              message: s.sidebarSessionOpen,
              child: Container(
                width: 7,
                height: 7,
                decoration: BoxDecoration(
                  color: tokens.accent,
                  shape: BoxShape.circle,
                  boxShadow: [
                    BoxShadow(
                        color: tokens.accent.withValues(alpha: 0.7),
                        blurRadius: 6),
                  ],
                ),
              ),
            ),
          ],
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
}

/// Create/join entry rows (the sidebar affordance rows at phone width);
/// 45dp tall. Solid hairline borders — Border-Not-Shadow.
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
    return Material(
      color: Colors.transparent,
      child: InkWell(
        onTap: onTap,
        borderRadius: radius,
        child: DecoratedBox(
          decoration: BoxDecoration(
            borderRadius: radius,
            border: Border.all(color: borderColor),
          ),
          child: Padding(
            padding: const EdgeInsets.symmetric(
                horizontal: JeliyaSpacing.x12, vertical: JeliyaSpacing.x12),
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

// -- rooms tab: the chat route ----------------------------------------------------------

/// The chat screen: back affordance + RoomHeader (its own <560 stack), the
/// room-keyed timeline, and the composer. The room-detail route (RightPanel
/// tabs) is pushed from here for Members / Share file / Open pipe / timeline
/// pipe tiles — every desktop deep-link callback keeps its intent.
class _MobileRoomScreen extends StatelessWidget {
  const _MobileRoomScreen({required this.onInvite, required this.onLeaveRoom});

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
      // empty state with the back affordance still in reach.
      return ColoredBox(
        color: tokens.bg,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Align(
              alignment: Alignment.centerLeft,
              child: BackButton(color: tokens.text),
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
      child: ListenableBuilder(
        listenable: room,
        builder: (context, _) => Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            RoomHeader(
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
                onShowPipes: () => _pushDetail(context, PanelTab.pipes),
              ),
            ),
            const Composer(),
          ],
        ),
      ),
    );
  }
}

// -- rooms tab: the room-detail route ---------------------------------------------------

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
    String? name;
    if (room != null) {
      for (final r in session.rooms) {
        if (r.roomId == room.roomId) name = r.name;
      }
    }
    return ColoredBox(
      color: tokens.bg,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(
            padding: const EdgeInsets.fromLTRB(
                JeliyaSpacing.x4, JeliyaSpacing.x4, JeliyaSpacing.x14,
                JeliyaSpacing.x4),
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
                        : (name ?? s.shellUntitledRoom),
                    style: JeliyaText.cardTitle,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
              ],
            ),
          ),
          Expanded(
            child: room == null
                ? Center(
                    child: Text(s.shellSelectRoom,
                        style:
                            TextStyle(fontSize: 13.5, color: tokens.textDim)),
                  )
                : RightPanel(
                    tab: _tab,
                    onTab: (tab) => setState(() => _tab = tab),
                    onLeaveRoom: widget.onLeaveRoom,
                  ),
          ),
        ],
      ),
    );
  }
}

// -- pipes / files tab surfaces ----------------------------------------------------------

/// The room-scoped RightPanel pinned full width to one panel tab, with the
/// honest select-a-room empty state when no room is open.
class _PanelSurface extends StatelessWidget {
  const _PanelSurface({
    required this.tab,
    required this.onTab,
    required this.onLeaveRoom,
  });

  final PanelTab tab;
  final ValueChanged<PanelTab> onTab;
  final VoidCallback onLeaveRoom;

  @override
  Widget build(BuildContext context) {
    final session = SessionScope.of(context);
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    if (session.room == null) {
      return ColoredBox(
        color: tokens.bg,
        child: Center(
          child: Text(s.shellSelectRoom,
              style: TextStyle(fontSize: 13.5, color: tokens.textDim)),
        ),
      );
    }
    return RightPanel(tab: tab, onTab: onTab, onLeaveRoom: onLeaveRoom);
  }
}
