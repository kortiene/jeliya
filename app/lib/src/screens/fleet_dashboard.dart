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
        AttentionReason,
        FleetAgent,
        FleetResult,
        HistoryPoint,
        LabelTone,
        LivenessValues,
        attentionRank,
        attentionReason,
        hasNumericProgress,
        labelTone,
        needsAttention,
        shortId,
        statusUnverified;

import '../format.dart';
import '../l10n/strings_context.dart';
import '../l10n/tokens.dart';
import '../layout.dart';
import '../session/daemon_session.dart';
import '../session/fleet_store.dart';
import '../session/room_homonyms.dart';
import '../theme.dart';
import '../widgets/buttons.dart';
import '../widgets/copy_button.dart';
import '../widgets/error_note.dart';
import '../widgets/avatar.dart';
import '../widgets/modal_scaffold.dart';
import '../widgets/progress_bar.dart';
import '../widgets/room_short_id.dart';
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

enum _FleetFilter { all, live, needsAttention, working, offline }

bool _matchesFilter(FleetAgent a, _FleetFilter f) => switch (f) {
  // `working || online-idle` — an agent whose peer is reachable. "Live" says
  // that; "Active" said it in a word the room rail used for a local session
  // and the wire uses for signed membership (docs/room-workbench.md,
  // decision 4).
  _FleetFilter.live =>
    a.liveness == LivenessValues.working ||
        a.liveness == LivenessValues.onlineIdle,
  // The full closed set (docs/room-attention.md, decision 4), not the old
  // blue-only match that silently dropped failed/blocked, stale, and
  // offline-after-work agents.
  _FleetFilter.needsAttention =>
    needsAttention(a.liveness, a.latest?.label),
  _FleetFilter.working => a.liveness == LivenessValues.working,
  _FleetFilter.offline =>
    a.liveness == LivenessValues.offline || a.liveness == LivenessValues.stale,
  _FleetFilter.all => true,
};

String _filterLabel(AppStrings s, _FleetFilter f) => switch (f) {
  _FleetFilter.all => s.fleetFilterAll,
  _FleetFilter.live => s.fleetFilterLive,
  _FleetFilter.needsAttention => s.fleetFilterNeedsAttention,
  _FleetFilter.working => s.fleetFilterWorking,
  _FleetFilter.offline => s.fleetFilterOffline,
};

// -- dashboard ---------------------------------------------------------------------

class FleetDashboard extends StatefulWidget {
  const FleetDashboard({super.key, required this.onOpenRoom, this.active = true});

  /// Room chip / Open Room clicks — the shell ignores departed rooms and
  /// switches back to the chat surface.
  final ValueChanged<String> onOpenRoom;

  /// Whether the Agents surface is the current one. The dashboard stays mounted
  /// (Offstage) either way — this gates the poll loop, not the widget.
  final bool active;

  @override
  State<FleetDashboard> createState() => _FleetDashboardState();
}

class _FleetDashboardState extends State<FleetDashboard>
    with WidgetsBindingObserver {
  FleetStore? _store;
  _FleetFilter _filter = _FleetFilter.all;
  final TextEditingController _search = TextEditingController();

  @override
  void initState() {
    super.initState();
    // The dashboard is always mounted, so it can watch the app lifecycle and
    // pause the poll when the app is backgrounded (not just when it is off the
    // current route).
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // The client is available by the time the shell mounts this surface; create
    // the store once. It does not poll until it is active AND foregrounded, so
    // creating it Offstage at boot starts nothing.
    final client = SessionScope.of(context).client;
    if (_store == null && client != null) {
      _store = FleetStore(client: client)..setActive(widget.active);
    }
  }

  @override
  void didUpdateWidget(FleetDashboard oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.active != oldWidget.active) _store?.setActive(widget.active);
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    // Only `resumed` is truly foreground; inactive/paused/hidden/detached all
    // pause the poll.
    _store?.setForeground(state == AppLifecycleState.resumed);
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
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
    final addAgent = JeliyaButton(
      label: s.fleetAddAgent,
      variant: JeliyaButtonVariant.primary,
      onPressed: _openAddAgent,
    );
    final title = <Widget>[
      const TreeMark(size: 26),
      const SizedBox(width: JeliyaSpacing.x10),
      // Flexible with ellipsis: the title shares its row with the mark (and,
      // on desktop, the search + Add Agent), and "Flotte d'agents" is wide
      // enough to overrun a 360dp phone. It shortens rather than overflowing —
      // and the same guard holds at 200% text, where even the English title
      // needs the room to give.
      Flexible(
        child: Semantics(
          header: true,
          child: Text(
            // The same words as the rail entry that opens it (decision 1). A
            // rail reading "Agent Fleet" that lands on a page titled "Agents" —
            // the name the room's own Agents & Runs destination also answers
            // to — leaves the user to guess which question they just asked.
            s.sidebarNavFleet,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(
              fontSize: 22,
              fontWeight: FontWeight.w700,
            ),
          ),
        ),
      ),
    ];
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
          // Below the breakpoint the head wraps like the web's
          // .fleet-head-top (flex-wrap): the title keeps its own line and the
          // fixed 200px search yields to flex beside Add Agent, so the
          // actions row holds at 360dp even under the wider French copy.
          // Desktop keeps the reference single-row layout.
          if (mobile) ...[
            Row(children: title),
            const SizedBox(height: JeliyaSpacing.x10),
            Row(
              children: [
                Expanded(child: search),
                const SizedBox(width: JeliyaSpacing.x10),
                addAgent,
              ],
            ),
          ] else
            Row(
              children: [
                ...title,
                const Spacer(),
                SizedBox(width: 200, child: search),
                const SizedBox(width: JeliyaSpacing.x10),
                addAgent,
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
    final fmt = context.formats;
    final fleet = store.fleet;
    final q = _search.text.trim().toLowerCase();
    final visible = (fleet?.agents ?? const <FleetAgent>[]).where((a) {
      if (!_matchesFilter(a, _filter)) return false;
      if (q.isEmpty) return true;
      return session.displayName(s, a.identityId).toLowerCase().contains(q) ||
          a.identityId.toLowerCase().contains(q);
    }).toList();

    // Homonyms across EVERY local room, not just the ones the fleet
    // references. A name is ambiguous when two of the user's rooms share it —
    // whether or not both have agents — so a fleet chip whose twin has no
    // agents yet must still be disambiguated (it would otherwise read as
    // unique here while the rooms rail shows it is not). A fleet chip DISPLAYS
    // `name ?? shortId(room_id)`, so that resolved string is what we group on:
    // two rooms named the same collide and get the disambiguator, while
    // untitled rooms already show their own short id and never do
    // (docs/room-workbench.md, decision 6).
    final roomHomonyms = homonymousRoomIds(
      [
        for (final r in session.rooms)
          (roomId: r.roomId, name: r.name ?? shortId(r.roomId)),
      ],
      untitledLabel: '',
    );

    return ListView(
      padding: const EdgeInsets.fromLTRB(
        JeliyaSpacing.page,
        JeliyaSpacing.section,
        JeliyaSpacing.page,
        26,
      ),
      children: [
        if (store.error != null) ErrorNote(error: store.error),
        // Honest freshness: when THIS client last refreshed (a device-local
        // fact, never a signed event), so a paused poll's staleness is visible.
        if (store.lastLoadedAtMs != null)
          Padding(
            padding: const EdgeInsets.only(bottom: JeliyaSpacing.x10),
            child: Text(
              s.fleetRefreshedAt(fmt.relTime(store.lastLoadedAtMs!)),
              style: TextStyle(fontSize: 11.5, color: tokens.textMute),
            ),
          ),
        // Actionable agents before the aggregate totals (#69) — on every shell,
        // unlike the KPI tiles below.
        if (store.loaded && fleet != null && fleet.total > 0)
          _NeedsAttention(agents: fleet.agents, onOpenRoom: widget.onOpenRoom),
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
            key: const Key('fleetAgentGrid'),
            agents: visible,
            store: store,
            roomHomonyms: roomHomonyms,
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

// -- needs attention (actionable agents, before the aggregate tiles) ----------

String _reasonLabel(AppStrings s, AttentionReason r) => switch (r) {
  AttentionReason.failed => s.fleetAttentionFailed,
  AttentionReason.review => s.fleetAttentionReview,
  AttentionReason.stale => s.fleetAttentionStale,
  AttentionReason.offline => s.fleetAttentionOffline,
};

/// The prioritized section the epic is named for: agents that need a human,
/// ranked most-actionable first, rendered ABOVE the aggregate tiles. Membership
/// and order come from the shared classifier (docs/room-attention.md,
/// decision 4), so a failed or stale agent is never silently dropped again.
class _NeedsAttention extends StatelessWidget {
  const _NeedsAttention({required this.agents, required this.onOpenRoom});

  final List<FleetAgent> agents;
  final ValueChanged<String> onOpenRoom;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;

    final items = <({FleetAgent agent, AttentionReason reason})>[];
    for (final a in agents) {
      final r = attentionReason(a.liveness, a.latest?.label);
      if (r != null) items.add((agent: a, reason: r));
    }
    items.sort((x, y) {
      final byRank = attentionRank(x.agent.liveness, x.agent.latest?.label)
          .compareTo(attentionRank(y.agent.liveness, y.agent.latest?.label));
      return byRank != 0
          ? byRank
          : (y.agent.lastSeenTs ?? 0).compareTo(x.agent.lastSeenTs ?? 0);
    });

    return Container(
      margin: const EdgeInsets.only(bottom: JeliyaSpacing.section),
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: tokens.bgCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: tokens.border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Semantics(
            header: true,
            child: Row(
              children: [
                Flexible(
                  child: Text(
                    s.fleetNeedsAttention,
                    style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      color: tokens.text,
                    ),
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                const SizedBox(width: JeliyaSpacing.x8),
                Container(
                  constraints:
                      const BoxConstraints(minWidth: 18, minHeight: 18),
                  padding: const EdgeInsets.symmetric(horizontal: 6),
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: tokens.bgCard2,
                    borderRadius: BorderRadius.circular(JeliyaRadii.pill),
                    border: Border.all(color: tokens.borderStrong),
                  ),
                  child: Text(
                    '${items.length}',
                    style: TextStyle(fontSize: 10.5, color: tokens.textDim),
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(height: JeliyaSpacing.x10),
          if (items.isEmpty)
            Text(
              s.fleetNeedsAttentionEmpty,
              style: TextStyle(fontSize: 13, color: tokens.textDim),
            )
          else
            for (var i = 0; i < items.length; i++) ...[
              if (i > 0) const SizedBox(height: JeliyaSpacing.x8),
              _AttentionRow(
                agent: items[i].agent,
                reason: items[i].reason,
                onOpenRoom: onOpenRoom,
              ),
            ],
        ],
      ),
    );
  }
}

class _AttentionRow extends StatelessWidget {
  const _AttentionRow({
    required this.agent,
    required this.reason,
    required this.onOpenRoom,
  });

  final FleetAgent agent;
  final AttentionReason reason;
  final ValueChanged<String> onOpenRoom;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final fmt = context.formats;
    final room = agent.latest?.roomId ??
        (agent.rooms.isEmpty ? null : agent.rooms.first.roomId);
    final message = agent.latest?.message;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 11, vertical: 9),
      decoration: BoxDecoration(
        color: tokens.bgCard2,
        borderRadius: BorderRadius.circular(11),
        border: Border.all(color: tokens.border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // name + reason chip wrap so they never overflow at 360dp / FR.
          Wrap(
            spacing: JeliyaSpacing.x8,
            runSpacing: JeliyaSpacing.x6,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Avatar(id: agent.identityId, size: 26),
                  const SizedBox(width: JeliyaSpacing.x8),
                  ConstrainedBox(
                    constraints: const BoxConstraints(maxWidth: 170),
                    child: SenderName(
                      id: agent.identityId,
                      style: TextStyle(fontSize: 13.5, color: tokens.text),
                    ),
                  ),
                ],
              ),
              _ReasonChip(reason: reason),
            ],
          ),
          if (message != null) ...[
            const SizedBox(height: JeliyaSpacing.x6),
            Text(
              message,
              style: TextStyle(fontSize: 12.5, color: tokens.textDim),
            ),
          ],
          const SizedBox(height: JeliyaSpacing.x8),
          Row(
            children: [
              Expanded(
                child: Text(
                  agent.lastSeenTs != null
                      ? s.fleetLastUpdate(fmt.relTime(agent.lastSeenTs!))
                      : s.fleetNeverSeen,
                  style: TextStyle(fontSize: 11.5, color: tokens.textMute),
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              if (room != null)
                JeliyaButton(
                  label: s.fleetOpenRoom,
                  size: JeliyaButtonSize.sm,
                  onPressed: () => onOpenRoom(room),
                ),
            ],
          ),
        ],
      ),
    );
  }
}

/// Reason chip: dot + label, never colour alone (WCAG AA). Failed/review reuse
/// the red/blue label tones; stale is amber (degraded); offline-after-work
/// recedes to a quiet mark, not a hue that claims urgency.
class _ReasonChip extends StatelessWidget {
  const _ReasonChip({required this.reason});

  final AttentionReason reason;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final (Color fg, Color bg, Color border) = switch (reason) {
      AttentionReason.failed => (
        tokens.toneColor(LabelTone.red),
        tokens.toneBg(LabelTone.red) ?? Colors.transparent,
        tokens.toneBorder(LabelTone.red),
      ),
      AttentionReason.review => (
        tokens.toneColor(LabelTone.blue),
        tokens.toneBg(LabelTone.blue) ?? Colors.transparent,
        tokens.toneBorder(LabelTone.blue),
      ),
      AttentionReason.stale => (
        tokens.amber,
        tokens.amber.withValues(alpha: 0.08),
        tokens.amberLine,
      ),
      AttentionReason.offline => (
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
        border: Border.all(color: border),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 6,
            height: 6,
            decoration: BoxDecoration(color: fg, shape: BoxShape.circle),
          ),
          const SizedBox(width: 5),
          Text(
            _reasonLabel(s, reason),
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
                // The truthful metric: agents in the `working` liveness state
                // (a live peer + a fresh working status), not an inferred
                // "running tasks" count — there is no task registry.
                label: s.fleetStatWorkingNow,
                value: '${fleet.working}',
                sub: s.fleetStatWorkingNowSub,
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

  /// One agent-card placeholder (avatar tile, name/id bars, status band,
  /// footer bar) — static tonal shapes only.
  Widget _card() => Container(
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
                borderRadius: BorderRadius.circular(JeliyaRadii.btn),
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
  );

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final mobile = isMobileWidth(context);
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
              // The KPI strip is hidden below the breakpoint, so its
              // skeleton row is too — a placeholder for a row that never
              // renders would be a lie.
              if (!mobile) ...[
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
              ],
              // Card placeholders mirror the grid: three across on desktop,
              // a single column below the breakpoint (the grid's one-column
              // floor at phone widths — fixed inner bars cannot share a row
              // at 360dp).
              if (mobile)
                for (var i = 0; i < 3; i++) ...[
                  if (i > 0) const SizedBox(height: JeliyaSpacing.x14),
                  _card(),
                ]
              else
                Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    for (var i = 0; i < 3; i++) ...[
                      if (i > 0) const SizedBox(width: JeliyaSpacing.x14),
                      Expanded(child: _card()),
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
    super.key,
    required this.agents,
    required this.store,
    required this.roomHomonyms,
    required this.onOpenRoom,
  });

  final List<FleetAgent> agents;
  final FleetStore store;

  /// room_ids that share a displayed name with another fleet room (decision 6).
  final Set<String> roomHomonyms;

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
                  roomHomonyms: roomHomonyms,
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
    required this.roomHomonyms,
    required this.onOpenRoom,
  });

  final FleetAgent agent;

  /// Sparkline history: null while loading, empty when none.
  final List<HistoryPoint>? points;

  /// room_ids that share a displayed name with another fleet room (decision 6).
  final Set<String> roomHomonyms;

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
                          _LabelChip(
                            label: latest.label,
                            unverified: statusUnverified(agent.liveness),
                          ),
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
                child: _StatusStrip(points: points, color: tint, muted: muted),
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
                    roomId: r.roomId,
                    name: r.name ?? shortId(r.roomId),
                    // A named room shared with another fleet room repeats its
                    // short id; an untitled room already shows one as its name.
                    homonym: roomHomonyms.contains(r.roomId) && r.name != null,
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
/// labels render neutral, no accent, no glow). When [unverified] (the agent is
/// stale/offline), the label is shown past-tense ("Last: X") and drops the
/// earned-green accent, so a Stale pill can never sit beside a live "Working"
/// chip — the contradiction #69 removes.
class _LabelChip extends StatelessWidget {
  const _LabelChip({required this.label, this.unverified = false});

  final String label;
  final bool unverified;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final rawTone = labelTone(label);
    final tone = unverified && rawTone == LabelTone.green
        ? LabelTone.neutral
        : rawTone;
    final text = unverified
        ? s.fleetLastStatus(prettyLabel(label))
        : prettyLabel(label);
    return Tooltip(
      message: unverified ? s.fleetLastStatusHint : text,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
        decoration: BoxDecoration(
          color: tokens.toneBg(tone),
          borderRadius: BorderRadius.circular(JeliyaRadii.pill),
          border: Border.all(color: tokens.toneBorder(tone)),
        ),
        child: Text(
          text,
          style: TextStyle(
            fontSize: 11,
            letterSpacing: 0.22,
            color: tokens.toneColor(tone),
          ),
          overflow: TextOverflow.ellipsis,
        ),
      ),
    );
  }
}

class _RoomChip extends StatelessWidget {
  const _RoomChip({
    required this.roomId,
    required this.name,
    required this.homonym,
    required this.tooltip,
    required this.onTap,
  });

  final String roomId;
  final String name;

  /// True when this room shares its DISPLAYED name with another fleet room —
  /// then the chip repeats the short id so the two can be told apart (decision
  /// 6). An untitled room already shows its short id as [name] and is never a
  /// homonym here.
  final bool homonym;

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
            // The name yields first; the short id is the disambiguator and
            // must stay readable at any width.
            if (homonym) ...[
              const SizedBox(width: 5),
              RoomShortId(roomId: roomId, fontSize: 11),
            ],
          ],
        ),
      ),
    );
  }
}

// -- sparkline (points-only, no interpolation) ---------------------------------------------
//
// One mark per real agent_status event from agent.history, positioned by its
// REAL timestamp. No connecting line and no area fill — a curve between events
// would fabricate intermediate state the log never recorded. Single series per
// card, so no legend.

class _StatusStrip extends StatelessWidget {
  const _StatusStrip({
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
          painter: _StatusStripPainter(
            points: pts,
            stroke: muted ? tokens.textMute : color,
            baseline: tokens.borderStrong,
            muted: muted,
            toneRed: tokens.toneColor(LabelTone.red),
            toneBlue: tokens.toneColor(LabelTone.blue),
            toneGreen: tokens.toneColor(LabelTone.green),
            toneNeutral: tokens.toneColor(LabelTone.neutral),
          ),
        ),
      ),
    );
  }
}

/// A numeric-progress event rises to its measured height as a stem; a
/// categorical (label-only) event is a dot on the baseline tinted by its label
/// tone, never lifted to a fabricated y-value (docs/room-attention.md,
/// decision 6). No interpolation joins the marks.
class _StatusStripPainter extends CustomPainter {
  const _StatusStripPainter({
    required this.points,
    required this.stroke,
    required this.baseline,
    required this.muted,
    required this.toneRed,
    required this.toneBlue,
    required this.toneGreen,
    required this.toneNeutral,
  });

  final List<HistoryPoint>? points;
  final Color stroke;
  final Color baseline;
  final bool muted;
  final Color toneRed;
  final Color toneBlue;
  final Color toneGreen;
  final Color toneNeutral;

  static const double _pad = 4;

  Color _toneColor(LabelTone tone) => switch (tone) {
    LabelTone.red => toneRed,
    LabelTone.blue => toneBlue,
    LabelTone.green => toneGreen,
    LabelTone.neutral => toneNeutral,
  };

  @override
  void paint(Canvas canvas, Size size) {
    final w = size.width;
    final h = size.height;
    final base = h - _pad;
    final pts = points;

    // Loading (null): solid baseline. Empty: dashed baseline.
    if (pts == null || pts.isEmpty) {
      final paint = Paint()
        ..color = baseline
        ..strokeWidth = 1.5
        ..style = PaintingStyle.stroke;
      if (pts == null) {
        canvas.drawLine(Offset(_pad, base), Offset(w - _pad, base), paint);
      } else {
        // strokeDasharray "2 3"
        var x = _pad;
        while (x < w - _pad) {
          final end = (x + 2).clamp(_pad, w - _pad);
          canvas.drawLine(Offset(x, base), Offset(end, base), paint);
          x += 5;
        }
      }
      return;
    }

    // The time axis: a static baseline, not a data line.
    canvas.drawLine(
      Offset(_pad, base),
      Offset(w - _pad, base),
      Paint()
        ..color = baseline
        ..strokeWidth = 1
        ..style = PaintingStyle.stroke,
    );

    final tsMin = pts.first.ts;
    final tsMax = pts.last.ts;
    final span = tsMax - tsMin;

    double xAt(int i) {
      if (pts.length == 1) return w - _pad;
      final t = span > 0 ? (pts[i].ts - tsMin) / span : i / (pts.length - 1);
      return _pad + t * (w - 2 * _pad);
    }

    final stem = Paint()
      ..color = stroke.withValues(alpha: muted ? 0.7 : 1)
      ..strokeWidth = 2
      ..style = PaintingStyle.stroke
      ..strokeCap = StrokeCap.round;

    for (var i = 0; i < pts.length; i++) {
      final x = xAt(i);
      final p = pts[i];
      if (hasNumericProgress(p.progress)) {
        final y = base - (p.progress!.clamp(0, 100) / 100) * (h - 2 * _pad);
        canvas.drawLine(Offset(x, base), Offset(x, y), stem);
        canvas.drawCircle(Offset(x, y), 2.6, Paint()..color = stroke);
      } else {
        canvas.drawCircle(
          Offset(x, base),
          2.6,
          Paint()..color = muted ? stroke : _toneColor(labelTone(p.label)),
        );
      }
    }
  }

  @override
  bool shouldRepaint(_StatusStripPainter oldDelegate) =>
      oldDelegate.points != points ||
      oldDelegate.stroke != stroke ||
      oldDelegate.baseline != baseline ||
      oldDelegate.muted != muted;
}
