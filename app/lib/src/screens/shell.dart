/// App shell (phase 'ready') — the Room Workbench on three shells
/// (docs/room-workbench.md, decisions 2 and 3; web parity: ui/src/App.tsx).
///
/// ONE [JeliyaRoute] is the navigation state. This screen used to hold three
/// fields — an `_overlay`, a right-panel `_tab`, and a `_navView` intent — that
/// could disagree with each other and with the session's open room, and every
/// new entry point had to remember to set all of them. They are gone.
/// Everything below is derived from `_route` on read: which global destination
/// the rail highlights, which room is open, whether the inspector is showing
/// and what it shows, which single pane a phone displays, and what Back does.
/// Flutter has no URL bar, so the route lives in this State — the point is a
/// single source, not a browser address. It is the same route family the web
/// parses (routes.dart), so a route means the same destination in both.
///
/// The three shells, one topology:
///
///   compact  one pane at a time; the bottom bar carries the three global
///            destinations and disappears inside a room, where the room's app
///            bar replaces it.
///   medium   rail + workspace; the inspector is a drawer pinned over the
///            workspace's right edge.
///   wide     rail + workspace + inspector column.
///
/// Fleet and Settings paint OVER the workspace (the rail stays) with the
/// obscured panes kept alive but invisible — visibility, not unmount — so the
/// timeline scroll position survives (the web's `visibility:hidden` contract).
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter/services.dart' show SystemNavigator;
import 'package:jeliya_protocol/jeliya_protocol.dart' show RoomSummary;

import '../l10n/strings_context.dart';
import '../layout.dart';
import '../routes.dart';
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
import 'room_nav.dart';
import 'settings_panel.dart';
import 'sidebar.dart';
import 'timeline.dart';

class ShellScreen extends StatefulWidget {
  const ShellScreen({super.key});

  @override
  State<ShellScreen> createState() => _ShellScreenState();
}

class _ShellScreenState extends State<ShellScreen> {
  // Pane widths, reconciled with the web (ui/src/styles.css) rather than kept
  // at the 280/320 this file used to carry. The record's arithmetic is written
  // in these numbers — 900 - 232 = a 668dp medium workspace, 1280 - 272 - 360 =
  // a 648dp wide one with the inspector present — and it is the arithmetic that
  // justifies the 1280 breakpoint. Two clients disagreeing about the width of
  // the same column would make that sum true on one of them only.
  static const double _railWide = 272;

  /// The rail gives back 40dp at medium: the whole point of the medium shell
  /// is to stop paying for a third column before there is room for one, and
  /// the workspace is what it is paid back into.
  static const double _railMedium = 232;

  static const double _inspectorWidth = 360;

  /// The navigation state. Not a mirror of one — the only one.
  JeliyaRoute _route = kRoomsRoute;

  /// True once the route expresses an opinion of its own.
  ///
  /// The initial route is "no opinion", which is the only state in which the
  /// session's restored room (`prefs.lastRoomId`, resolved by the daemon
  /// bootstrap) may fill it in. After that nothing re-picks a room behind the
  /// route's back: the bootstrap re-runs on every reconnect, and letting it
  /// drag the user back into the room they had just left is the web's issue
  /// #88. An explicit route always wins over a restored room.
  bool _routeChosen = false;

  /// The build-time form-factor fork; handlers re-read it after awaits.
  bool get _isMobile => isMobileWidth(context);

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // Adopt, do not open. The bootstrap has already opened the restored room
    // by the time this shell mounts; naming it here is what makes the route
    // agree with the session instead of racing it.
    if (_routeChosen) return;
    final roomId = SessionScope.of(context).currentRoomId;
    if (roomId == null) return;
    _routeChosen = true;
    _route = RoomRoute(roomId);
  }

  // -- navigation ------------------------------------------------------------
  //
  // Every entry point navigates. This is the ONE place that moves the session
  // in response, so a rail row, a fleet card, a create result and a restored
  // room all take the same path — and none of them can open a room the route
  // does not name (the web learned that one the hard way: its create/join
  // callbacks opened the room imperatively and the route effect promptly
  // snapped the session back to the room the URL still named).

  void _navigate(JeliyaRoute next) {
    _routeChosen = true;
    if (next == _route) return;
    setState(() => _route = next);
    final roomId = next.roomId;
    // selectRoom guards departed rooms and re-opening the room already open;
    // a route that names no room leaves the session alone, because the open
    // session is not navigation state — the rooms list says so out loud, with
    // the room's Open/Closed label.
    if (roomId != null) SessionScope.of(context).selectRoom(roomId);
  }

  /// Room-row and fleet-card clicks. A room's canonical landing surface is its
  /// timeline, so selecting one is navigating to its Activity — unless it is a
  /// room this identity cannot open, in which case the only honest destination
  /// is Rooms.
  ///
  /// A fleet card can name a joined-then-left archive (`agents.fleet` aggregates
  /// over left/removed archives too — docs/PROTOCOL.md), and a stale row can
  /// name one the roster has since departed. `selectRoom` refuses to open those,
  /// so routing INTO one would leave the workspace empty behind a hidden bottom
  /// bar — a dead end with no way out. Mirror the web's `selectRoom`: land on
  /// Rooms, which is always a way back.
  void _selectRoom(String roomId) {
    final summary = _summaryOf(SessionScope.of(context), roomId);
    if (summary == null ||
        summary.status == 'left' ||
        summary.status == 'removed') {
      _navigate(kRoomsRoute);
      return;
    }
    _navigate(RoomRoute(roomId));
  }

  void _navigateGlobal(GlobalDest dest) => _navigate(GlobalRoute(dest));

  /// The room nav's single handler. Activity closes the inspector, a tool
  /// opens it, and both are the same navigation — which is what lets one
  /// destination mean one thing on all three shells.
  void _goToDest(RoomDest dest) {
    final roomId = _route.roomId;
    if (roomId == null) return;
    _navigate(RoomRoute(roomId, dest));
  }

  void _closeInspector() => _goToDest(RoomDest.activity);

  void _backToRooms() => _navigate(kRoomsRoute);

  /// System and predictive Back, on every shell: a room tool falls back to the
  /// room's Activity, everything else falls back to Rooms, and Rooms hands the
  /// gesture to the platform. Back never mutates state the user cannot see —
  /// it only ever changes where they are standing.
  void _back() {
    final route = _route;
    if (route is RoomRoute && route.dest != RoomDest.activity) {
      _navigate(RoomRoute(route.id));
      return;
    }
    if (route != kRoomsRoute) {
      _navigate(kRoomsRoute);
      return;
    }
    SystemNavigator.pop();
  }

  // -- room lookup -----------------------------------------------------------

  RoomSummary? _summaryOf(DaemonSession session, String? roomId) {
    for (final r in session.rooms) {
      if (r.roomId == roomId) return r;
    }
    return null;
  }

  // -- modals ----------------------------------------------------------------

  Future<void> _openCreateRoom() async {
    final session = SessionScope.of(context);
    // Create room STAYS a dialog on phones (one small field).
    final roomId = await showJeliyaModal<String>(
      context,
      builder: (_) => const CreateRoomModal(),
    );
    if (roomId == null) return;
    // Refresh first, and await it: the rail resolves a room against this list,
    // so naming one room.list has not returned yet would show the user the
    // "no such room" treatment for the room they just made.
    await session.refreshRooms();
    if (!mounted) return;
    _navigate(RoomRoute(roomId));
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
    await session.refreshRooms(); // see _openCreateRoom
    if (!mounted) return;
    _navigate(RoomRoute(roomId));
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
    final summary = _summaryOf(session, _route.roomId);
    if (room == null) return;
    // A destructive confirm stays a centered dialog on every form factor.
    final left = await showJeliyaModal<bool>(
      context,
      builder: (_) => LeaveRoomModal(
        roomId: room.roomId,
        roomName: summary?.name,
      ),
    );
    if (left != true || !mounted) return;
    session.leaveCurrentRoom();
    // The route named a room this identity has now published a departure from.
    // It is not a destination any more.
    _navigate(kRoomsRoute);
  }

  // -- build -----------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    final shell = shellOf(context);
    return PopScope(
      canPop: false,
      // Dialogs and full-screen modal routes live on the root navigator ABOVE
      // this route, so the system back reaches here only with the shell on
      // top. The shell claims every back and answers all of them from the
      // route: with no nested navigator left to dispatch its own
      // NavigationNotification, this PopScope is the only authority the engine
      // hears, and WidgetsApp keeps reporting setFrameworkHandlesBack(true)
      // for as long as the shell is up. (Android predictive back takes the
      // NEXT gesture entirely once the framework reports false, so a stray
      // `false` here would silently retire this policy — pinned by
      // predictive_back_test.dart.)
      onPopInvokedWithResult: (didPop, _) {
        if (didPop) return;
        _back();
      },
      child: shell == Shell.compact
          ? MobileShell(
              route: _route,
              onGlobal: _navigateGlobal,
              onSelectRoom: _selectRoom,
              onCreateRoom: _openCreateRoom,
              onJoinRoom: _openJoinRoom,
              onDest: _goToDest,
              onBackToRooms: _backToRooms,
              onInvite: _openInvite,
              onLeaveRoom: _openLeaveRoom,
            )
          : _buildDesktop(context, shell),
    );
  }

  Widget _buildDesktop(BuildContext context, Shell shell) {
    final s = context.strings;
    final session = SessionScope.of(context);
    final tokens = JeliyaTokens.of(context);
    final fleetOpen = _route == const GlobalRoute(GlobalDest.fleet);
    final settingsOpen = _route == const GlobalRoute(GlobalDest.settings);
    final overlayActive = fleetOpen || settingsOpen;
    final inspector = _route.inspectorDest;

    return Scaffold(
      body: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // A row, not a Positioned overlay: connection status reserves its
          // space so it can never cover Back, a header, or list content. It
          // renders itself away while connected.
          ConnectionBanner(
              conn: session.conn, wsUrl: session.transportDescription),
          Expanded(
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                SizedBox(
                  width: shell == Shell.wide ? _railWide : _railMedium,
                  child: Sidebar(
                    activeNav: _route.activeGlobal,
                    currentRoomId: _route.roomId,
                    onNav: _navigateGlobal,
                    onSelectRoom: _selectRoom,
                    onCreateRoom: _openCreateRoom,
                    onJoinRoom: _openJoinRoom,
                  ),
                ),
                // Workspace + inspector, with the fleet/settings surfaces
                // stacked over them. Visibility (not removal) preserves
                // timeline scroll.
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
                              Expanded(
                                  child: _buildWorkspace(
                                      context, shell, session, s, tokens)),
                              // Wide only: the inspector stops being a drawer
                              // and takes a column of its own, in flow.
                              if (shell == Shell.wide && inspector != null)
                                SizedBox(
                                  width: _inspectorWidth,
                                  child: RightPanel(
                                    tab: inspector,
                                    onDest: _goToDest,
                                    onClose: _closeInspector,
                                    shell: shell,
                                    onLeaveRoom: _openLeaveRoom,
                                  ),
                                ),
                            ],
                          ),
                        ),
                      ),
                      // FleetDashboard stays mounted (Offstage) so its search,
                      // filter, and scroll survive navigation; its poll loop is
                      // gated on `active` + app lifecycle instead of on
                      // mount/unmount, so it still never runs in the
                      // background (#69).
                      Positioned.fill(
                        child: Offstage(
                          offstage: !fleetOpen,
                          child: FleetDashboard(
                            onOpenRoom: _selectRoom,
                            active: fleetOpen,
                          ),
                        ),
                      ),
                      // Settings stays mounted (cheap, stateful copy feedback).
                      Positioned.fill(
                        child: Offstage(
                          offstage: !settingsOpen,
                          child: SettingsPanel(onCreateRoom: _openCreateRoom),
                        ),
                      ),
                      // Medium only: the inspector is a drawer pinned to the
                      // workspace's right edge. No scrim — it is dismissible,
                      // not modal, and the timeline beside it keeps working
                      // (web parity: .app > .right-panel at medium).
                      if (shell == Shell.medium && inspector != null)
                        Positioned(
                          top: 0,
                          bottom: 0,
                          right: 0,
                          width: _inspectorWidth,
                          child: Material(
                            elevation: 12,
                            color: tokens.bgRaise,
                            child: RightPanel(
                              tab: inspector,
                              onDest: _goToDest,
                              onClose: _closeInspector,
                              shell: shell,
                              onLeaveRoom: _openLeaveRoom,
                            ),
                          ),
                        ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildWorkspace(BuildContext context, Shell shell,
      DaemonSession session, AppStrings s, JeliyaTokens tokens) {
    final room = session.room;
    final routeRoomId = _route.roomId;
    // Render the session's open room whenever the route names NO room — a
    // Fleet/Settings overlay (or Rooms) sits OVER this workspace, which stays
    // mounted behind it (Visibility maintainState) so the timeline's scroll and
    // the composer's draft survive the round trip. Clearing on a null route id
    // would unmount the TimelineView the moment the overlay opens, and the
    // maintained state would be gone before the user came back. Only clear when
    // there is genuinely no open room, or the route names a DIFFERENT room than
    // the session holds — drawing that store under this route's name is the
    // disagreement the route model exists to make impossible.
    if (room == null || (routeRoomId != null && room.roomId != routeRoomId)) {
      return ColoredBox(
        color: tokens.bg,
        child: Center(
          child: Text(s.shellSelectRoom,
              style: TextStyle(fontSize: 13.5, color: tokens.textDim)),
        ),
      );
    }
    final summary = _summaryOf(session, room.roomId);
    return ColoredBox(
      color: tokens.bg,
      child: ListenableBuilder(
        listenable: room,
        builder: (context, _) => Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            RoomHeader(
              name: summary?.name ?? s.shellUntitledRoom,
              summary: summary,
              compact: false,
              onBack: _backToRooms,
              onInvite: _openInvite,
              onShareFile: () => _goToDest(RoomDest.files),
              onOpenPipe: () => _goToDest(RoomDest.pipes),
            ),
            // The room's nested navigation, under its header. Without it,
            // People and Agents & Runs would have no entry point at all while
            // the inspector is closed.
            //
            // At medium an open drawer floats over most of this strip, and the
            // drawer carries its own — so this one stands down rather than
            // lying buried underneath, where it would still report selection,
            // still take focus, and still swallow taps meant for what floats
            // on top of it. On wide the inspector opens beside the workspace,
            // so this stays the one strip.
            if (shell == Shell.wide || _route.inspectorDest == null)
              RoomNav(
                dest: _route.inspectorDest ?? RoomDest.activity,
                counts: roomNavCounts(room),
                onDest: _goToDest,
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
                onShowPipes: () => _goToDest(RoomDest.pipes),
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
