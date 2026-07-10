/// Top-level Agents view (FleetDashboard.tsx): cross-room agent fleet with
/// liveness, latest status, sparkline history, filters/search, and the Add
/// Agent invite-minting flow.
///
/// Honesty rules carried over verbatim:
/// - Liveness is the wire's derived value rendered as-is — a `stale` agent
///   whose last label was `working` shows STALE, never active (§1.2).
/// - The sparkline draws one mark per REAL agent_status event; y = progress
///   when the event carried one, else a band from the label tone — never a
///   fabricated intermediate point.
/// - Skeletons are static tonal bars (no shimmer); offline cards recede via
///   a muted ground + dimmed graphics only, text keeps full contrast.
///
/// Data lives in [FleetStore] (4s poll + 400ms push debounce), created when
/// this widget mounts and disposed with it — the shell mounts the dashboard
/// only while the Agents surface is active, so the poll loop never runs in
/// the background.
library;

import 'package:flutter/material.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart'
    show
        FleetAgent,
        FleetResult,
        HistoryPoint,
        LabelTone,
        LivenessValues,
        labelTone,
        shortId;

import '../format.dart';
import '../l10n/strings_context.dart';
import '../l10n/tokens.dart';
import '../layout.dart';
import '../session/daemon_session.dart';
import '../session/fleet_store.dart';
import '../theme.dart';
import '../widgets/buttons.dart';
import '../widgets/copy_button.dart';
import '../widgets/error_note.dart';
import '../widgets/avatar.dart';
import '../widgets/modal_scaffold.dart';
import '../widgets/progress_bar.dart';
import '../widgets/sender_name.dart';
import '../widgets/tree_mark.dart';
import 'modals/add_agent.dart';

// -- display helpers (format.ts ports that live app-side) -----------------------
// prettyLabel lives in ../format.dart — the shared single home.

// relTime lives in ../format.dart — the shared single home.

// -- liveness presentation (the four §1.2 states, truthful) ----------------------

enum _LivenessTone { live, idle, warn, off }

_LivenessTone _livenessTone(String liveness) => switch (liveness) {
  LivenessValues.working => _LivenessTone.live,
  LivenessValues.onlineIdle => _LivenessTone.idle,
  LivenessValues.stale => _LivenessTone.warn,
  _ => _LivenessTone.off,
};

String _livenessLabel(AppStrings s, String liveness) => switch (liveness) {
  LivenessValues.working => s.fleetLivenessWorking,
  LivenessValues.onlineIdle => s.fleetLivenessOnline,
  LivenessValues.stale => s.fleetLivenessStale,
  _ => s.fleetLivenessOffline,
};

// -- filters ---------------------------------------------------------------------

enum _FleetFilter { all, active, needsAttention, working, offline }

bool _matchesFilter(FleetAgent a, _FleetFilter f) => switch (f) {
  _FleetFilter.active =>
    a.liveness == LivenessValues.working ||
        a.liveness == LivenessValues.onlineIdle,
  _FleetFilter.needsAttention =>
    a.latest != null && labelTone(a.latest!.label) == LabelTone.blue,
  _FleetFilter.working => a.liveness == LivenessValues.working,
  _FleetFilter.offline =>
    a.liveness == LivenessValues.offline || a.liveness == LivenessValues.stale,
  _FleetFilter.all => true,
};

String _filterLabel(AppStrings s, _FleetFilter f) => switch (f) {
  _FleetFilter.all => s.fleetFilterAll,
  _FleetFilter.active => s.fleetFilterActive,
  _FleetFilter.needsAttention => s.fleetFilterNeedsAttention,
  _FleetFilter.working => s.fleetFilterWorking,
  _FleetFilter.offline => s.fleetFilterOffline,
};

// -- dashboard ---------------------------------------------------------------------

class FleetDashboard extends StatefulWidget {
  const FleetDashboard({super.key, required this.onOpenRoom});

  /// Room chip / Open Room clicks — the shell ignores departed rooms and
  /// switches back to the chat surface.
  final ValueChanged<String> onOpenRoom;

  @override
  State<FleetDashboard> createState() => _FleetDashboardState();
}

class _FleetDashboardState extends State<FleetDashboard> {
  FleetStore? _store;
  _FleetFilter _filter = _FleetFilter.all;
  final TextEditingController _search = TextEditingController();

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // The client is available by the time the shell mounts this surface;
    // create the store once (poll loop starts here, dies in dispose).
    final client = SessionScope.of(context).client;
    if (_store == null && client != null) {
      _store = FleetStore(client: client);
    }
  }

  @override
  void dispose() {
    _store?.dispose();
    _search.dispose();
    super.dispose();
  }

  /// Full screen below the breakpoint (mono launch-command textarea + soft
  /// keyboard); the dialog at desktop widths. Minting works on phones —
  /// running the command is the deliberate human step either way.
  Future<void> _openAddAgent() => isMobileWidth(context)
      ? showJeliyaModalScreen<void>(context,
          builder: (_) => const AddAgentModal())
      : showJeliyaModal<void>(context, builder: (_) => const AddAgentModal());

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final store = _store;
    if (store == null) {
      return ColoredBox(color: tokens.bg, child: const SizedBox.expand());
    }
    return ColoredBox(
      color: tokens.bg,
      child: ListenableBuilder(
        listenable: store,
        builder: (context, _) => Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _buildHeader(s, tokens, store),
            Expanded(child: _buildBody(s, tokens, store)),
          ],
        ),
      ),
    );
  }

  // -- header (brand + search + Add Agent + filter pills) --------------------------

  Widget _buildHeader(AppStrings s, JeliyaTokens tokens, FleetStore store) {
    final agents = store.fleet?.agents ?? const <FleetAgent>[];
    final mobile = isMobileWidth(context);
    final search = Semantics(
      label: s.fleetSearchAgents,
      child: TextField(
        controller: _search,
        onChanged: (_) => setState(() {}),
        style: const TextStyle(fontSize: 13.5),
        decoration: InputDecoration(
          hintText: s.fleetSearchPlaceholder,
        ),
      ),
    );
    return Container(
      padding: const EdgeInsets.fromLTRB(
        JeliyaSpacing.page,
        JeliyaSpacing.section,
        JeliyaSpacing.page,
        0,
      ),
      decoration: BoxDecoration(
        color: tokens.bgRaise,
        border: Border(bottom: BorderSide(color: tokens.border)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const TreeMark(size: 26),
              const SizedBox(width: JeliyaSpacing.x10),
              Semantics(
                header: true,
                child: Text(
                  s.fleetAgentsTitle,
                  style: const TextStyle(
                    fontSize: 22,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
              // Below the breakpoint the fixed 200px search yields to flex so
              // the row holds at 360dp; desktop keeps the reference layout.
              if (mobile) ...[
                const SizedBox(width: JeliyaSpacing.x10),
                Expanded(child: search),
              ] else ...[
                const Spacer(),
                SizedBox(width: 200, child: search),
              ],
              const SizedBox(width: JeliyaSpacing.x10),
              JeliyaButton(
                label: s.fleetAddAgent,
                variant: JeliyaButtonVariant.primary,
                onPressed: _openAddAgent,
              ),
            ],
          ),
          const SizedBox(height: JeliyaSpacing.x14),
          // Mutually-exclusive filter toggles — pressed-state buttons, not
          // tabs (no tabpanels, no roving tabindex behind that contract).
          Semantics(
            container: true,
            label: s.fleetFilterAgents,
            child: Padding(
              padding: const EdgeInsets.only(bottom: JeliyaSpacing.x12),
              // The web's .fleet-filters scrolls horizontally when narrow.
              child: SingleChildScrollView(
                scrollDirection: Axis.horizontal,
                child: Row(
                  children: [
                    for (final f in _FleetFilter.values) ...[
                      _FilterPill(
                        label: _filterLabel(s, f),
                        count: f == _FleetFilter.all
                            ? agents.length
                            : agents.where((a) => _matchesFilter(a, f)).length,
                        active: _filter == f,
                        onTap: () => setState(() => _filter = f),
                      ),
                      if (f != _FleetFilter.values.last)
                        const SizedBox(width: JeliyaSpacing.x8),
                    ],
                  ],
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  // -- body (error / stat tiles / skeleton / empty / grid) ---------------------------

  Widget _buildBody(AppStrings s, JeliyaTokens tokens, FleetStore store) {
    final session = SessionScope.of(context);
    final fleet = store.fleet;
    final q = _search.text.trim().toLowerCase();
    final visible = (fleet?.agents ?? const <FleetAgent>[]).where((a) {
      if (!_matchesFilter(a, _filter)) return false;
      if (q.isEmpty) return true;
      return session.displayName(s, a.identityId).toLowerCase().contains(q) ||
          a.identityId.toLowerCase().contains(q);
    }).toList();

    return ListView(
      padding: const EdgeInsets.fromLTRB(
        JeliyaSpacing.page,
        JeliyaSpacing.section,
        JeliyaSpacing.page,
        26,
      ),
      children: [
        if (store.error != null) ErrorNote(error: store.error),
        // The KPI strip is deliberately hidden on phones — room coverage
        // isn't check-in-relevant there; the filter pills carry live counts
        // (web parity: styles.css hides .fleet-stats below the breakpoint).
        if (fleet != null && !isMobileWidth(context)) _StatTiles(fleet: fleet),
        if (!store.loaded)
          _FleetSkeleton(tokens: tokens)
        else if (visible.isEmpty)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 48, horizontal: 20),
            child: Text(
              fleet != null && fleet.total == 0
                  ? s.fleetEmptyNoAgents
                  : s.fleetEmptyNoMatch,
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 13.5, color: tokens.textDim),
            ),
          )
        else
          _AgentGrid(
            agents: visible,
            store: store,
            onOpenRoom: widget.onOpenRoom,
          ),
      ],
    );
  }
}

// -- filter pill ---------------------------------------------------------------------

class _FilterPill extends StatelessWidget {
  const _FilterPill({
    required this.label,
    required this.count,
    required this.active,
    required this.onTap,
  });

  final String label;
  final int count;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final fg = active ? tokens.accent : tokens.textDim;
    // TextButton (not a bare GestureDetector) so the pill is keyboard
    // focusable and Enter/Space activates it, like the web's <button>.
    return Semantics(
      toggled: active, // aria-pressed
      child: TextButton(
        onPressed: onTap,
        style: ButtonStyle(
          padding: const WidgetStatePropertyAll(
              EdgeInsets.symmetric(horizontal: 13, vertical: 7)),
          backgroundColor:
              WidgetStatePropertyAll(active ? tokens.accentDim : tokens.bgCard),
          overlayColor: const WidgetStatePropertyAll(Colors.transparent),
          minimumSize: const WidgetStatePropertyAll(Size.zero),
          tapTargetSize: MaterialTapTargetSize.shrinkWrap,
          shape: WidgetStatePropertyAll(RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(JeliyaRadii.pill),
            side: BorderSide(
              color: active ? tokens.accentLine : tokens.borderStrong,
            ),
          )),
        ),
        child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(label, style: TextStyle(fontSize: 13, color: fg)),
                const SizedBox(width: JeliyaSpacing.x6),
                Container(
                  constraints: const BoxConstraints(
                    minWidth: 18,
                    minHeight: 18,
                  ),
                  padding: const EdgeInsets.symmetric(horizontal: 5),
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: tokens.bgCard2,
                    borderRadius: BorderRadius.circular(JeliyaRadii.pill),
                    border: Border.all(
                      color: active ? tokens.accentLine : tokens.borderStrong,
                    ),
                  ),
                  child: Text(
                    '$count',
                    style: TextStyle(fontSize: 10.5, color: fg),
                  ),
                ),
              ],
        ),
      ),
    );
  }
}

// -- stat tiles ------------------------------------------------------------------------

class _StatTiles extends StatelessWidget {
  const _StatTiles({required this.fleet});

  final FleetResult fleet;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final fmt = context.formats;
    final coverage = fleet.roomsTotal > 0
        ? ((fleet.roomsCovered / fleet.roomsTotal) * 100).round()
        : 0;
    return Padding(
      padding: const EdgeInsets.only(bottom: JeliyaSpacing.section),
      // IntrinsicHeight bounds the stretch inside the unbounded ListView so
      // the three tiles stay equal-height (the web grid's row behavior).
      child: IntrinsicHeight(
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(
              child: _StatTile(
                icon: Tokens.fleetStatActiveIcon,
                iconColor: tokens.accent,
                iconBg: tokens.accentDim,
                label: s.fleetStatActiveAgents,
                value: '${fleet.active}',
                sub: s.fleetStatOfTotal(fleet.total),
              ),
            ),
            const SizedBox(width: JeliyaSpacing.x12),
            Expanded(
              child: _StatTile(
                icon: Tokens.fleetStatTasksIcon,
                iconColor: tokens.amber,
                iconBg: tokens.amberDim,
                label: s.fleetStatRunningTasks,
                value: '${fleet.working}',
                sub: s.fleetStatOneTaskPerAgent,
              ),
            ),
            const SizedBox(width: JeliyaSpacing.x12),
            Expanded(
              child: _StatTile(
                icon: Tokens.fleetStatCoverageIcon,
                iconColor: tokens.blue,
                iconBg: tokens.blue.withValues(alpha: 0.12),
                label: s.fleetStatRoomCoverage,
                value: fmt.percent(coverage),
                sub: s.fleetStatRoomsCovered(
                  fleet.roomsCovered,
                  fleet.roomsTotal,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _StatTile extends StatelessWidget {
  const _StatTile({
    required this.icon,
    required this.iconColor,
    required this.iconBg,
    required this.label,
    required this.value,
    required this.sub,
  });

  final String icon;
  final Color iconColor;
  final Color iconBg;
  final String label;
  final String value;
  final String sub;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
      decoration: BoxDecoration(
        color: tokens.bgCard,
        borderRadius: BorderRadius.circular(JeliyaRadii.bubble),
        border: Border.all(color: tokens.border),
      ),
      child: Row(
        children: [
          ExcludeSemantics(
            child: Container(
              width: 38,
              height: 38,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: iconBg,
                borderRadius: BorderRadius.circular(JeliyaRadii.nav),
              ),
              child: Text(
                icon,
                style: TextStyle(fontSize: 18, color: iconColor),
              ),
            ),
          ),
          const SizedBox(width: JeliyaSpacing.x12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  label,
                  style: TextStyle(fontSize: 12, color: tokens.textDim),
                ),
                Text(
                  value,
                  style: const TextStyle(
                    fontSize: 26,
                    fontWeight: FontWeight.w700,
                    height: 1.1,
                  ),
                ),
                Text(
                  sub,
                  style: TextStyle(fontSize: 11.5, color: tokens.textMute),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

// -- skeleton (static tonal bars — honest "still fetching", no shimmer) ------------------

class _FleetSkeleton extends StatelessWidget {
  const _FleetSkeleton({required this.tokens});

  final JeliyaTokens tokens;

  Widget _bar(double width, double height) => Container(
    width: width,
    height: height,
    decoration: BoxDecoration(
      color: tokens.bgCard2,
      borderRadius: BorderRadius.circular(JeliyaRadii.iconBtn),
    ),
  );

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // The web's visually-hidden role='status' "Loading agents" node.
        Semantics(
          liveRegion: true,
          label: s.fleetLoadingAgents,
          child: const SizedBox(width: 1, height: 1),
        ),
        ExcludeSemantics(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  for (var i = 0; i < 3; i++) ...[
                    if (i > 0) const SizedBox(width: JeliyaSpacing.x12),
                    Expanded(
                      child: Container(
                        padding: const EdgeInsets.symmetric(
                          horizontal: 16,
                          vertical: 14,
                        ),
                        decoration: BoxDecoration(
                          color: tokens.bgCard,
                          borderRadius: BorderRadius.circular(
                            JeliyaRadii.bubble,
                          ),
                          border: Border.all(color: tokens.border),
                        ),
                        child: Row(
                          children: [
                            Container(
                              width: 38,
                              height: 38,
                              decoration: BoxDecoration(
                                color: tokens.bgCard2,
                                borderRadius: BorderRadius.circular(
                                  JeliyaRadii.nav,
                                ),
                              ),
                            ),
                            const SizedBox(width: JeliyaSpacing.x12),
                            Column(
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: [
                                _bar(92, 10),
                                const SizedBox(height: JeliyaSpacing.x8),
                                _bar(52, 22),
                              ],
                            ),
                          ],
                        ),
                      ),
                    ),
                  ],
                ],
              ),
              const SizedBox(height: JeliyaSpacing.section),
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  for (var i = 0; i < 3; i++) ...[
                    if (i > 0) const SizedBox(width: JeliyaSpacing.x14),
                    Expanded(
                      child: Container(
                        padding: const EdgeInsets.all(15),
                        decoration: BoxDecoration(
                          color: tokens.bgCard,
                          borderRadius: BorderRadius.circular(JeliyaRadii.hero),
                          border: Border.all(color: tokens.border),
                        ),
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.stretch,
                          children: [
                            Row(
                              children: [
                                Container(
                                  width: 34,
                                  height: 34,
                                  decoration: BoxDecoration(
                                    color: tokens.bgCard2,
                                    borderRadius: BorderRadius.circular(
                                      JeliyaRadii.btn,
                                    ),
                                  ),
                                ),
                                const SizedBox(width: JeliyaSpacing.x12),
                                Column(
                                  crossAxisAlignment: CrossAxisAlignment.start,
                                  children: [
                                    _bar(120, 12),
                                    const SizedBox(height: JeliyaSpacing.x8),
                                    _bar(96, 10),
                                  ],
                                ),
                              ],
                            ),
                            const SizedBox(height: JeliyaSpacing.x12),
                            _bar(double.infinity, 40),
                            const SizedBox(height: JeliyaSpacing.x12),
                            FractionallySizedBox(
                              alignment: Alignment.centerLeft,
                              widthFactor: 0.6,
                              child: _bar(double.infinity, 10),
                            ),
                          ],
                        ),
                      ),
                    ),
                  ],
                ],
              ),
            ],
          ),
        ),
      ],
    );
  }
}

// -- agent card grid (CSS auto-fill minmax(320px, 1fr), gap 14) ---------------------------

class _AgentGrid extends StatelessWidget {
  const _AgentGrid({
    required this.agents,
    required this.store,
    required this.onOpenRoom,
  });

  final List<FleetAgent> agents;
  final FleetStore store;
  final ValueChanged<String> onOpenRoom;

  @override
  Widget build(BuildContext context) {
    const minCard = 320.0;
    const gap = JeliyaSpacing.x14;
    return LayoutBuilder(
      builder: (context, constraints) {
        final width = constraints.maxWidth;
        // CSS auto-fill: the column count comes from the available width
        // alone (cards never widen to fill leftover columns).
        var cols = ((width + gap) / (minCard + gap)).floor();
        if (cols < 1) cols = 1;
        final cardWidth = (width - gap * (cols - 1)) / cols;
        return Wrap(
          spacing: gap,
          runSpacing: gap,
          children: [
            for (final agent in agents)
              SizedBox(
                width: cardWidth,
                child: _AgentCard(
                  agent: agent,
                  points: store.historyFor(agent.identityId),
                  onOpenRoom: onOpenRoom,
                ),
              ),
          ],
        );
      },
    );
  }
}

// -- per-agent card ---------------------------------------------------------------------

class _AgentCard extends StatelessWidget {
  const _AgentCard({
    required this.agent,
    required this.points,
    required this.onOpenRoom,
  });

  final FleetAgent agent;

  /// Sparkline history: null while loading, empty when none.
  final List<HistoryPoint>? points;

  final ValueChanged<String> onOpenRoom;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final fmt = context.formats;
    final tone = _livenessTone(agent.liveness);
    final tint = tokens.colorForId(agent.identityId);
    final latest = agent.latest;
    final openRoom =
        latest?.roomId ??
        (agent.rooms.isEmpty ? null : agent.rooms.first.roomId);
    final off = tone == _LivenessTone.off;
    final muted = off || tone == _LivenessTone.warn;

    final borderColor = switch (tone) {
      _LivenessTone.live => tokens.accentLine,
      _LivenessTone.warn => tokens.amber.withValues(alpha: 0.35),
      _ => tokens.border,
    };

    return Container(
      padding: const EdgeInsets.all(15),
      decoration: BoxDecoration(
        // Offline cards recede via the muted ground + dimmed graphics only —
        // never blanket opacity (text keeps its full AA contrast).
        color: off ? tokens.bgRaise : tokens.bgCard,
        borderRadius: BorderRadius.circular(JeliyaRadii.hero),
        border: Border.all(color: borderColor),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Opacity(
                opacity: off ? 0.5 : 1,
                child: Container(
                  padding: const EdgeInsets.all(3),
                  decoration: BoxDecoration(
                    color: tokens.tileBg(agent.identityId),
                    borderRadius: BorderRadius.circular(JeliyaRadii.nav),
                  ),
                  child: Avatar(id: agent.identityId, size: 34),
                ),
              ),
              const SizedBox(width: 11),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Wrap(
                      spacing: JeliyaSpacing.x8,
                      runSpacing: JeliyaSpacing.x2,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        SenderName(
                          id: agent.identityId,
                          style: const TextStyle(
                            fontSize: 14.5,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                        _LivePill(
                          tone: tone,
                          label: _livenessLabel(s, agent.liveness),
                        ),
                      ],
                    ),
                    const SizedBox(height: JeliyaSpacing.x2),
                    Tooltip(
                      message: agent.identityId,
                      child: Text(
                        '${agent.identityId.length > 12 ? agent.identityId.substring(0, 12) : agent.identityId}…',
                        style: JeliyaText.mono(
                          fontSize: 11,
                          color: tokens.textMute,
                        ),
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                  ],
                ),
              ),
              CopyButton(
                text: agent.identityId,
                label: Tokens.fleetCopyGlyph,
                semanticLabel: s.commonCopyIdentityId,
              ),
            ],
          ),
          const SizedBox(height: JeliyaSpacing.x12),
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: latest != null
                    ? Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          _LabelChip(label: latest.label),
                          if (latest.message != null) ...[
                            const SizedBox(height: JeliyaSpacing.x6),
                            Text(
                              latest.message!,
                              style: TextStyle(
                                fontSize: 13,
                                color: tokens.textDim,
                              ),
                            ),
                          ],
                        ],
                      )
                    : Text(
                        s.fleetNoStatusPosted,
                        style: TextStyle(fontSize: 13, color: tokens.textDim),
                      ),
              ),
              const SizedBox(width: JeliyaSpacing.x12),
              Opacity(
                opacity: off ? 0.5 : 1,
                child: _Sparkline(points: points, color: tint, muted: muted),
              ),
            ],
          ),
          if (latest?.progress != null) ...[
            const SizedBox(height: JeliyaSpacing.x12),
            Row(
              children: [
                Expanded(
                  child: ProgressBar(value: latest!.progress!.toDouble()),
                ),
                const SizedBox(width: JeliyaSpacing.x8),
                Text(
                  fmt.percent(latest.progress!.clamp(0, 100).round()),
                  style: JeliyaText.mono(fontSize: 11.5, color: tokens.textDim),
                ),
              ],
            ),
          ],
          if (agent.rooms.isNotEmpty) ...[
            const SizedBox(height: JeliyaSpacing.x12),
            Wrap(
              spacing: JeliyaSpacing.x6,
              runSpacing: JeliyaSpacing.x6,
              children: [
                for (final r in agent.rooms)
                  _RoomChip(
                    name: r.name ?? shortId(r.roomId),
                    tooltip: r.name ?? r.roomId,
                    onTap: () => onOpenRoom(r.roomId),
                  ),
              ],
            ),
          ],
          const SizedBox(height: JeliyaSpacing.x12),
          Row(
            children: [
              Expanded(
                child: Text(
                  agent.lastSeenTs != null
                      ? s.fleetLastUpdate(fmt.relTime(agent.lastSeenTs!))
                      : s.fleetNeverSeen,
                  style: TextStyle(fontSize: 11.5, color: tokens.textDim),
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              if (openRoom != null)
                JeliyaButton(
                  label: s.fleetOpenRoom,
                  size: JeliyaButtonSize.sm,
                  onPressed: () => onOpenRoom(openRoom),
                ),
            ],
          ),
        ],
      ),
    );
  }
}

/// Liveness pill: live = accent tinted + glow dot, idle = accent outline,
/// warn = amber @0.08 bg, off = mute. The glow is EARNED — only the live
/// state gets it.
class _LivePill extends StatelessWidget {
  const _LivePill({required this.tone, required this.label});

  final _LivenessTone tone;
  final String label;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final (Color fg, Color bg, Color borderColor) = switch (tone) {
      _LivenessTone.live => (
        tokens.accent,
        tokens.accentDim,
        tokens.accentLine,
      ),
      _LivenessTone.idle => (
        tokens.accent,
        Colors.transparent,
        tokens.accentLine,
      ),
      _LivenessTone.warn => (
        tokens.amber,
        tokens.amber.withValues(alpha: 0.08),
        tokens.amberLine,
      ),
      _LivenessTone.off => (
        tokens.textMute,
        Colors.transparent,
        tokens.borderStrong,
      ),
    };
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 2),
      decoration: BoxDecoration(
        color: bg,
        borderRadius: BorderRadius.circular(JeliyaRadii.pill),
        border: Border.all(color: borderColor),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 6,
            height: 6,
            decoration: BoxDecoration(
              color: fg,
              shape: BoxShape.circle,
              boxShadow: tone == _LivenessTone.live
                  ? [
                      BoxShadow(
                        color: tokens.accent.withValues(alpha: 0.7),
                        blurRadius: 6,
                      ),
                    ]
                  : null,
            ),
          ),
          const SizedBox(width: 5),
          Text(
            label,
            style: TextStyle(
              fontSize: 11,
              fontWeight: FontWeight.w600,
              color: fg,
            ),
          ),
        ],
      ),
    );
  }
}

/// Latest-status chip: tone-{labelTone} colors (green is earned — unknown
/// labels render neutral, no accent, no glow).
class _LabelChip extends StatelessWidget {
  const _LabelChip({required this.label});

  final String label;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final tone = labelTone(label);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
      decoration: BoxDecoration(
        color: tokens.toneBg(tone),
        borderRadius: BorderRadius.circular(JeliyaRadii.pill),
        border: Border.all(color: tokens.toneBorder(tone)),
      ),
      child: Text(
        prettyLabel(label),
        style: TextStyle(
          fontSize: 11,
          letterSpacing: 0.22,
          color: tokens.toneColor(tone),
        ),
        overflow: TextOverflow.ellipsis,
      ),
    );
  }
}

class _RoomChip extends StatelessWidget {
  const _RoomChip({
    required this.name,
    required this.tooltip,
    required this.onTap,
  });

  final String name;
  final String tooltip;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    // TextButton (not a bare GestureDetector) so the chip is keyboard
    // focusable and Enter/Space activates it, like the web's <button>.
    return Tooltip(
      message: tooltip,
      child: TextButton(
        onPressed: onTap,
        style: ButtonStyle(
          padding: const WidgetStatePropertyAll(
              EdgeInsets.symmetric(horizontal: 10, vertical: 3)),
          backgroundColor: WidgetStatePropertyAll(tokens.bgCard2),
          overlayColor: const WidgetStatePropertyAll(Colors.transparent),
          minimumSize: const WidgetStatePropertyAll(Size.zero),
          tapTargetSize: MaterialTapTargetSize.shrinkWrap,
          shape: WidgetStatePropertyAll(RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(JeliyaRadii.pill),
            side: BorderSide(color: tokens.borderStrong),
          )),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            ExcludeSemantics(
              child: Text(
                Tokens.fleetRoomChipGlyph,
                style: TextStyle(fontSize: 11.5, color: tokens.textDim),
              ),
            ),
            const SizedBox(width: 5),
            Flexible(
              child: Text(
                name,
                style: TextStyle(fontSize: 11.5, color: tokens.textDim),
                overflow: TextOverflow.ellipsis,
                maxLines: 1,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// -- sparkline (points-only, no interpolation) ---------------------------------------------
//
// One mark per real agent_status event from agent.history. y = progress when
// the event carried one, else a band derived from the label tone — never a
// fabricated intermediate point. Single series per card, so no legend.

/// The label-tone band (FleetDashboard.tsx `bandFor`): neutral (unknown
/// label) sits low-mid — not a failure, but not an earned healthy band.
double _bandFor(HistoryPoint p) {
  final progress = p.progress;
  if (progress != null) return progress.clamp(0, 100) / 100;
  return switch (labelTone(p.label)) {
    LabelTone.red => 0.18,
    LabelTone.neutral => 0.45,
    LabelTone.blue => 0.62,
    LabelTone.green => 0.8,
  };
}

class _Sparkline extends StatelessWidget {
  const _Sparkline({
    required this.points,
    required this.color,
    required this.muted,
  });

  /// Null = history not fetched yet — solid baseline placeholder so the
  /// dashed "no history" treatment is never flashed before data arrives.
  final List<HistoryPoint>? points;

  final Color color;

  /// Muted stroke for off/warn liveness tones.
  final bool muted;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final pts = points;
    final label = pts == null
        ? s.fleetSparkLoading
        : pts.isEmpty
        ? s.fleetSparkEmpty
        : s.fleetSparkEvents(pts.length);
    return Semantics(
      image: true, // role="img"
      label: label,
      child: Tooltip(
        message: label,
        child: CustomPaint(
          size: const Size(132, 40),
          painter: _SparklinePainter(
            points: pts,
            stroke: muted ? tokens.textMute : color,
            baseline: tokens.borderStrong,
            muted: muted,
          ),
        ),
      ),
    );
  }
}

class _SparklinePainter extends CustomPainter {
  const _SparklinePainter({
    required this.points,
    required this.stroke,
    required this.baseline,
    required this.muted,
  });

  final List<HistoryPoint>? points;
  final Color stroke;
  final Color baseline;
  final bool muted;

  static const double _pad = 4;

  @override
  void paint(Canvas canvas, Size size) {
    final w = size.width;
    final h = size.height;
    final pts = points;

    // Loading (null): solid baseline. Empty: dashed baseline.
    if (pts == null || pts.isEmpty) {
      final paint = Paint()
        ..color = baseline
        ..strokeWidth = 1.5
        ..style = PaintingStyle.stroke;
      final y = h - _pad;
      if (pts == null) {
        canvas.drawLine(Offset(_pad, y), Offset(w - _pad, y), paint);
      } else {
        // strokeDasharray "2 3"
        var x = _pad;
        while (x < w - _pad) {
          final end = (x + 2).clamp(_pad, w - _pad);
          canvas.drawLine(Offset(x, y), Offset(end, y), paint);
          x += 5;
        }
      }
      return;
    }

    final tsMin = pts.first.ts;
    final tsMax = pts.last.ts;
    final span = tsMax - tsMin;

    double xAt(int i) {
      if (pts.length == 1) return w - _pad;
      final t = span > 0 ? (pts[i].ts - tsMin) / span : i / (pts.length - 1);
      return _pad + t * (w - 2 * _pad);
    }

    double yAt(int i) => h - _pad - _bandFor(pts[i]) * (h - 2 * _pad);

    if (pts.length == 1) {
      canvas.drawCircle(Offset(xAt(0), yAt(0)), 3.2, Paint()..color = stroke);
      return;
    }

    final line = Path()..moveTo(xAt(0), yAt(0));
    for (var i = 1; i < pts.length; i++) {
      line.lineTo(xAt(i), yAt(i));
    }
    final last = pts.length - 1;
    final area = Path.from(line)
      ..lineTo(xAt(last), h - _pad)
      ..lineTo(xAt(0), h - _pad)
      ..close();

    // Gradient area fill (stroke @ 0.28, 0.16 muted → transparent).
    canvas.drawPath(
      area,
      Paint()
        ..shader = LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [
            stroke.withValues(alpha: muted ? 0.16 : 0.28),
            stroke.withValues(alpha: 0),
          ],
        ).createShader(Rect.fromLTWH(0, 0, w, h)),
    );

    canvas.drawPath(
      line,
      Paint()
        ..color = stroke.withValues(alpha: muted ? 0.7 : 1)
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2
        ..strokeJoin = StrokeJoin.round
        ..strokeCap = StrokeCap.round,
    );

    // Terminal dot marks the newest real event.
    canvas.drawCircle(Offset(xAt(last), yAt(last)), 3, Paint()..color = stroke);
  }

  @override
  bool shouldRepaint(_SparklinePainter oldDelegate) =>
      oldDelegate.points != points ||
      oldDelegate.stroke != stroke ||
      oldDelegate.baseline != baseline ||
      oldDelegate.muted != muted;
}
