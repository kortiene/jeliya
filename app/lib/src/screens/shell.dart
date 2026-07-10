/// App shell (phase 'ready'). At or above [kShellBreakpoint] it is the
/// 3-column desktop layout per phase3-features.json "App shell" and
/// phase3-shell.json:
///
///   Sidebar 280px | center (RoomHeader + Timeline + Composer) | RightPanel 320px
///
/// Fleet and Settings paint OVER the center+right columns (the sidebar stays)
/// with the obscured panes kept alive but invisible — visibility, not
/// unmount — so the timeline scroll position survives (the web contract's
/// `visibility:hidden` behavior). The connection banner renders above
/// everything whenever conn != connected.
///
/// Below the breakpoint the SAME state and handlers drive [MobileShell]
/// (bottom tab bar, one pane at a time — issue #17); only build forks. The
/// `_navView` intent machine already mirrors the web's `mobileView`, so it
/// maps 1:1 onto the bottom tabs.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:jeliya_protocol/jeliya_protocol.dart'
    show ConnectionState, RoomSummary;

import '../l10n/strings_context.dart';
import '../layout.dart';
import '../session/daemon_session.dart';
import '../theme.dart';
import '../widgets/connection_banner.dart';
import '../widgets/error_note.dart';
import '../widgets/modal_scaffold.dart';
import 'composer.dart';
import 'fleet_dashboard.dart';
import 'mobile_shell.dart';
import 'modals/create_room.dart';
import 'modals/invite.dart';
import 'modals/join_room.dart';
import 'modals/leave_room.dart';
import 'right_panel.dart';
import 'room_header.dart';
import 'settings_panel.dart';
import 'sidebar.dart';
import 'timeline.dart';

/// Which full surface paints over the center+right columns.
enum _Overlay { none, fleet, settings }

class ShellScreen extends StatefulWidget {
  const ShellScreen({super.key});

  @override
  State<ShellScreen> createState() => _ShellScreenState();
}

class _ShellScreenState extends State<ShellScreen> {
  static const double _sidebarWidth = 280;
  static const double _rightPanelWidth = 320;

  _Overlay _overlay = _Overlay.none;
  PanelTab _tab = PanelTab.members;

  /// Desktop mirror of the web's `mobileView` state (initial 'rooms'): the
  /// active-nav highlight is DERIVED from the last navigation intent, not
  /// from the right-panel tab (App.tsx `activeNav`: settings→settings,
  /// agents→agents, pipes→pipes, files→files, chat→home, rooms→rooms).
  /// `NavKey.home` stands in for the web's 'chat' view.
  NavKey _navView = NavKey.rooms;

  /// The mobile Rooms tab's nested navigator (rooms list → chat → detail);
  /// unmounted at desktop widths.
  final GlobalKey<NavigatorState> _mobileRoomsNav =
      GlobalKey<NavigatorState>();

  /// The build-time form-factor fork; handlers re-read it after awaits.
  bool get _isMobile => isMobileWidth(context);

  // -- navigation (App.tsx `navigate` mapped to desktop) -------------------------

  NavKey get _activeNav => _navView;

  /// App.tsx `navigate('home')`: 'chat' if a room is open, else 'rooms'.
  NavKey get _homeView => SessionScope.of(context).currentRoomId != null
      ? NavKey.home
      : NavKey.rooms;

  void _navigate(NavKey key) {
    setState(() {
      switch (key) {
        case NavKey.agents:
          // Top-level fleet dashboard — distinct from the in-room Agents tab.
          _overlay = _Overlay.fleet;
          _navView = NavKey.agents;
        case NavKey.settings:
          _overlay = _Overlay.settings;
          _navView = NavKey.settings;
        case NavKey.pipes:
          // Panel-tab deep link: sets the right-panel tab AND the nav view.
          _tab = PanelTab.pipes;
          _overlay = _Overlay.none;
          _navView = NavKey.pipes;
        case NavKey.files:
          _tab = PanelTab.files;
          _overlay = _Overlay.none;
          _navView = NavKey.files;
        case NavKey.home:
          _overlay = _Overlay.none;
          _navView = _homeView;
        case NavKey.rooms:
          _overlay = _Overlay.none;
          _navView = NavKey.rooms;
        case NavKey.calls:
          break; // disabled ('Soon')
      }
    });
  }

  void _setTab(PanelTab tab) => setState(() {
        _tab = tab;
        _overlay = _Overlay.none;
      });

  // -- room selection --------------------------------------------------------------

  void _selectRoom(String roomId) {
    final session = SessionScope.of(context);
    final summary = _summaryOf(session, roomId);
    final departed =
        summary?.status == 'left' || summary?.status == 'removed';
    setState(() {
      _overlay = _Overlay.none;
      // Web onSelectRoom: departed → just show the rooms list (no open),
      // else view 'chat'.
      _navView = departed ? NavKey.rooms : NavKey.home;
    });
    session.selectRoom(roomId); // guards departed rooms internally
  }

  void _openRoomFromFleet(String roomId) {
    final session = SessionScope.of(context);
    final summary = _summaryOf(session, roomId);
    if (summary == null ||
        summary.status == 'left' ||
        summary.status == 'removed') {
      return; // fleet clicks ignore departed rooms
    }
    setState(() {
      _overlay = _Overlay.none;
      _navView = NavKey.home; // web: mobileView 'chat'
    });
    if (roomId != session.currentRoomId) session.openRoom(roomId);
  }

  RoomSummary? _summaryOf(DaemonSession session, String? roomId) {
    for (final r in session.rooms) {
      if (r.roomId == roomId) return r;
    }
    return null;
  }

  // -- mobile (below the width breakpoint) ---------------------------------------
  //
  // Thin wrappers around the shared handlers: same intents, plus the route
  // pushes the phone IA needs (chat and room-detail live on the Rooms tab's
  // nested navigator).

  bool _isDeparted(String roomId) {
    final summary = _summaryOf(SessionScope.of(context), roomId);
    return summary == null ||
        summary.status == 'left' ||
        summary.status == 'removed';
  }

  void _popMobileRoomsToList() =>
      _mobileRoomsNav.currentState?.popUntil((route) => route.isFirst);

  /// Opens the chat route fresh over the rooms list (never stacks two).
  void _pushMobileChat() {
    final nav = _mobileRoomsNav.currentState;
    if (nav == null) return;
    nav.popUntil((route) => route.isFirst);
    nav.push(
        mobileRoomRoute(onInvite: _openInvite, onLeaveRoom: _openLeaveRoom));
  }

  /// Bottom-tab taps. Re-tapping Rooms while its stack is open pops back to
  /// the list (web `navigate('rooms')` always shows the list); merely
  /// RETURNING to Rooms from another tab leaves the stack alone so the chat
  /// route — and its timeline scroll — survives the round trip.
  void _onMobileNav(NavKey key) {
    if (key == NavKey.rooms &&
        (_navView == NavKey.rooms || _navView == NavKey.home)) {
      _popMobileRoomsToList();
    }
    _navigate(key);
  }

  void _selectRoomMobile(String roomId) {
    final departed = _isDeparted(roomId);
    _selectRoom(roomId);
    if (!departed) _pushMobileChat();
  }

  void _openRoomFromFleetMobile(String roomId) {
    if (_isDeparted(roomId)) return; // fleet clicks ignore departed rooms
    _openRoomFromFleet(roomId);
    _pushMobileChat();
  }

  /// RightPanel tab-strip taps on the pinned Pipes/Files surfaces:
  /// pipes/files map to their bottom tabs; the room-scoped Members/Agents
  /// land on the room-detail route under the Rooms tab.
  void _onMobilePanelTab(PanelTab tab) {
    switch (tab) {
      case PanelTab.pipes:
        _navigate(NavKey.pipes);
      case PanelTab.files:
        _navigate(NavKey.files);
      case PanelTab.members:
      case PanelTab.agents:
        _navigate(NavKey.home);
        _mobileRoomsNav.currentState?.push(mobileRoomDetailRoute(
            initialTab: tab, onLeaveRoom: _openLeaveRoom));
    }
  }

  // -- modals ------------------------------------------------------------------------

  Future<void> _openCreateRoom() async {
    final session = SessionScope.of(context);
    // Create room STAYS a dialog on phones (one small field).
    final roomId = await showJeliyaModal<String>(
      context,
      builder: (_) => const CreateRoomModal(),
    );
    if (roomId == null) return;
    await session.refreshRooms();
    await session.openRoom(roomId);
    if (mounted) {
      setState(() {
        _overlay = _Overlay.none;
        _navView = NavKey.home;
      });
      // Web parity: mobileView 'chat' after create.
      if (_isMobile) _pushMobileChat();
    }
  }

  Future<void> _openJoinRoom() async {
    final session = SessionScope.of(context);
    // Long mono paste + soft keyboard + live progress: full screen on phones.
    // Same awaited pop-a-roomId contract either way.
    String? roomId;
    if (_isMobile) {
      roomId = await showJeliyaModalScreen<String>(
        context,
        builder: (_) => const JoinRoomModal(),
      );
    } else {
      roomId = await showJeliyaModal<String>(
        context,
        builder: (_) => const JoinRoomModal(),
      );
    }
    if (roomId == null) return;
    await session.refreshRooms();
    await session.openRoom(roomId);
    if (mounted) {
      setState(() {
        _overlay = _Overlay.none;
        _navView = NavKey.home;
      });
      if (_isMobile) _pushMobileChat();
    }
  }

  Future<void> _openInvite() async {
    final session = SessionScope.of(context);
    final room = session.room;
    if (room == null) return;
    // Full screen on phones (4-row mono ticket boxes); the modal keeps
    // observing the LIVE RoomStore either way, so a reconnect's new
    // endpointAddr still updates the combined invite while it is open.
    Widget builder(BuildContext _) =>
        InviteModal(roomId: room.roomId, endpointAddr: room.endpointAddr);
    if (_isMobile) {
      await showJeliyaModalScreen<void>(context, builder: builder);
    } else {
      await showJeliyaModal<void>(context, builder: builder);
    }
  }

  Future<void> _openLeaveRoom() async {
    final session = SessionScope.of(context);
    final room = session.room;
    final summary = _currentSummary(session);
    if (room == null) return;
    // A destructive confirm stays a centered dialog on every form factor.
    final left = await showJeliyaModal<bool>(
      context,
      builder: (_) => LeaveRoomModal(
        roomId: room.roomId,
        roomName: summary?.name,
      ),
    );
    if (left == true) {
      session.leaveCurrentRoom();
      // Web leaveCurrentRoom sets mobileView 'rooms'.
      if (mounted) {
        setState(() => _navView = NavKey.rooms);
        // The chat/detail routes point at the departed room — back to the list.
        if (_isMobile) _popMobileRoomsToList();
      }
    }
  }

  RoomSummary? _currentSummary(DaemonSession session) =>
      _summaryOf(session, session.currentRoomId);

  // -- build -------------------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final session = SessionScope.of(context);
    final tokens = JeliyaTokens.of(context);
    final overlayActive = _overlay != _Overlay.none;

    // The ONLY form-factor fork: state, handlers, and _navView above are
    // shared; below the breakpoint the same machine drives the bottom-tab
    // shell (issue #17). At or above it, the three-pane layout renders
    // exactly as before.
    if (_isMobile) {
      return MobileShell(
        activeNav: _activeNav,
        onNav: _onMobileNav,
        roomsNavigatorKey: _mobileRoomsNav,
        onSelectRoom: _selectRoomMobile,
        onCreateRoom: _openCreateRoom,
        onJoinRoom: _openJoinRoom,
        onOpenRoomFromFleet: _openRoomFromFleetMobile,
        onPanelTab: _onMobilePanelTab,
        onLeaveRoom: _openLeaveRoom,
      );
    }

    return Scaffold(
      body: Stack(
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              SizedBox(
                width: _sidebarWidth,
                child: Sidebar(
                  activeNav: _activeNav,
                  onNav: _navigate,
                  onSelectRoom: _selectRoom,
                  onCreateRoom: _openCreateRoom,
                  onJoinRoom: _openJoinRoom,
                ),
              ),
              // Center + right, with the fleet/settings surfaces stacked over
              // them. Visibility (not removal) preserves timeline scroll.
              Expanded(
                child: Stack(
                  children: [
                    Positioned.fill(
                      child: Visibility(
                        visible: !overlayActive,
                        maintainState: true,
                        maintainAnimation: true,
                        maintainSize: true,
                        child: Row(
                          crossAxisAlignment: CrossAxisAlignment.stretch,
                          children: [
                            Expanded(child: _buildCenter(s, session, tokens)),
                            SizedBox(
                              width: _rightPanelWidth,
                              child: RightPanel(
                                tab: _tab,
                                onTab: _setTab,
                                onLeaveRoom: _openLeaveRoom,
                              ),
                            ),
                          ],
                        ),
                      ),
                    ),
                    // FleetDashboard mounts only while active (its 4s poll
                    // loop must not run in the background) — web parity.
                    if (_overlay == _Overlay.fleet)
                      Positioned.fill(
                        child: FleetDashboard(onOpenRoom: _openRoomFromFleet),
                      ),
                    // Settings stays mounted (cheap, stateful copy feedback).
                    Positioned.fill(
                      child: Offstage(
                        offstage: _overlay != _Overlay.settings,
                        child: SettingsPanel(onCreateRoom: _openCreateRoom),
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
          // Connection banner above everything when not connected.
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
    );
  }

  Widget _buildCenter(
      AppStrings s, DaemonSession session, JeliyaTokens tokens) {
    final room = session.room;
    final summary = _currentSummary(session);
    if (room == null) {
      return ColoredBox(
        color: tokens.bg,
        child: Center(
          child: Text(s.shellSelectRoom,
              style: TextStyle(fontSize: 13.5, color: tokens.textDim)),
        ),
      );
    }
    return ColoredBox(
      color: tokens.bg,
      child: ListenableBuilder(
        listenable: room,
        builder: (context, _) => Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            RoomHeader(
              name: summary?.name ?? s.shellUntitledRoom,
              memberCount: room.members.isNotEmpty
                  ? room.members.length
                  : summary?.memberCount ?? 0,
              onInvite: _openInvite,
              onShareFile: () => _setTab(PanelTab.files),
              onOpenPipe: () => _setTab(PanelTab.pipes),
            ),
            if (room.openError != null)
              Padding(
                padding: const EdgeInsets.symmetric(
                    horizontal: JeliyaSpacing.page),
                child: ErrorNote(error: room.openError),
              ),
            // Keyed by room so the live-region/scroll state resets on switch.
            Expanded(
              child: TimelineView(
                key: ValueKey(room.roomId),
                onShowPipes: () => _setTab(PanelTab.pipes),
              ),
            ),
            const Composer(),
          ],
        ),
      ),
    );
  }
}

// The cross-cutting connection banner lives in ../widgets/connection_banner.dart
// (shared verbatim with the mobile shell).
