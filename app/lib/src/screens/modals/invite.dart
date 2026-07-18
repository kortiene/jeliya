/// Invite modal (wide) — the guided invitation + re-invitation flow (issue #66,
/// P15), the Flutter mirror of ui/src/components/InviteModal.tsx. Presents
/// identity → role → expiry → sharing: the invitee id is validated inline with
/// [isIdentityId] (an obvious typo fails in the form, not as a daemon
/// invalid_params); the role carries a one-line consequence and, for agent, the
/// same security warning as the Add-Agent modal; expiry is a chooser over the
/// shared [expiryPresets] with an Advanced custom-seconds override; and once a
/// ticket is minted the combined `ticket#address` is built with
/// [buildCombinedInvite] (not string concat) and offered via Copy + platform
/// Share (mobile only). NO QR (deferred to follow-up #103).
///
/// After minting, the invite's lifecycle is derived from PROVABLE state only
/// ([inviteState]): the chip reads Joined ONLY when the room's roster shows an
/// `active` row for the invitee — never from the ticket. A 1s tick flips
/// waiting→expired without reopening; roster pushes flip waiting→joined live
/// (the [ListenableBuilder] on the room store). Reopening over a still-pending
/// `invited` row restores its waiting state instead of a blank draft.
library;

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart'
    show
        ErrorCodes,
        InviteStates,
        JeliyaMethods,
        LabelTone,
        Member,
        MemberStatuses,
        RequestError,
        Roles,
        buildCombinedInvite,
        errorShape,
        expiryPresets,
        inviteState,
        isIdentityId;
import 'package:share_plus/share_plus.dart';

import '../../l10n/error_display.dart';
import '../../l10n/strings_context.dart';
import '../../l10n/tokens.dart';
import '../../l10n/wire_display.dart';
import '../../layout.dart';
import '../../qr/qr_view.dart';
import '../../session/daemon_session.dart';
import '../../theme.dart';
import '../../widgets/buttons.dart';
import '../../widgets/copy_button.dart';
import '../../widgets/error_note.dart';
import '../../widgets/modal_scaffold.dart';
import '../../widgets/template_text.dart';

/// The default preset when the flow opens fresh — a bounded, single-day ticket
/// is a safer default than a never-expiring one (mirrors the React DEFAULT).
const String _defaultPreset = '24h';

/// Wall clock for the lifecycle chip's waiting→expired flip. Overridable in
/// widget tests (there is no fake wall clock under `pump()`); production reads
/// the real clock. Both the minted expiry and "now" flow through it, so a test
/// controls the whole timeline.
@visibleForTesting
int Function() inviteNowMs = () => DateTime.now().millisecondsSinceEpoch;

class InviteModal extends StatefulWidget {
  const InviteModal({super.key, required this.roomId, required this.endpointAddr});

  final String roomId;

  /// The current room session's dialable address (RoomStore.endpointAddr);
  /// null when the daemon reported none.
  final String? endpointAddr;

  @override
  State<InviteModal> createState() => _InviteModalState();
}

class _InviteModalState extends State<InviteModal> {
  final TextEditingController _identityId = TextEditingController();
  final TextEditingController _customExpiry = TextEditingController();

  /// Wire role value ([Roles]) — member or agent; sent as the protocol constant.
  String _role = Roles.member;

  /// The selected expiry preset key ([expiryPresets] key); a non-empty
  /// Advanced custom value overrides it at mint time.
  String _expiryKey = _defaultPreset;
  bool _advancedOpen = false;

  bool _busy = false;
  String? _ticket;
  RequestError? _error;

  /// Marker for the client-local expiry validation failure — its copy is built
  /// AT RENDER TIME (never cached in state, so a live locale switch re-resolves
  /// it). A wire invalid_params must NOT set this.
  bool _expiryInvalid = false;

  /// The invite being tracked ({identityId, expiresAtMs}). [_mintedIdentityId]
  /// non-null means "tracking an invite" — a fresh mint this session, or a
  /// restored still-`invited` roster row. [_mintedExpiresAtMs] is null for a
  /// never-expiry ticket AND for a restored row whose expiry we cannot know
  /// (keeping the chip honest: Waiting until the roster proves otherwise).
  String? _mintedIdentityId;
  int? _mintedExpiresAtMs;

  /// 1s tick that flips waiting→expired without reopening; only runs while a
  /// time-boxed invite is tracked (waiting→joined comes from the roster).
  Timer? _tick;

  /// The one-time roster restore (didChangeDependencies can fire more than once).
  bool _restoredFromRoster = false;

  @override
  void initState() {
    super.initState();
    _identityId.addListener(() => setState(() {}));
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_restoredFromRoster) return;
    _restoredFromRoster = true;
    // Restore ONCE from the roster: a still-pending `invited` row restores the
    // proven waiting state (identity, role, a Waiting chip) instead of a blank
    // form — the roster is the runtime proof, not the ticket.
    final store = SessionScope.of(context).room;
    final live = store != null && store.roomId == widget.roomId ? store : null;
    final pending = _pendingInvite(live?.members ?? const []);
    if (pending != null) {
      _identityId.text = pending.identityId;
      _role = pending.role == Roles.agent ? Roles.agent : Roles.member;
      _mintedIdentityId = pending.identityId;
      // Expiry is unknown for a restored row — null keeps the chip honest.
      _mintedExpiresAtMs = null;
    }
  }

  @override
  void dispose() {
    _tick?.cancel();
    _identityId.dispose();
    _customExpiry.dispose();
    super.dispose();
  }

  /// The first still-`invited` roster row, if any (the pending invitation).
  Member? _pendingInvite(List<Member> members) {
    for (final m in members) {
      if (m.status == MemberStatuses.invited) return m;
    }
    return null;
  }

  /// The preset's seconds for [key] (null = no expiry / single-use).
  int? _presetSeconds(String key) {
    for (final p in expiryPresets) {
      if (p.key == key) return p.seconds;
    }
    return null;
  }

  /// Keeps the 1s tick running iff a time-boxed invite is being tracked.
  void _reconcileTick() {
    final want = _mintedIdentityId != null && _mintedExpiresAtMs != null;
    if (want && _tick == null) {
      _tick = Timer.periodic(const Duration(seconds: 1), (_) {
        if (mounted) setState(() {});
      });
    } else if (!want && _tick != null) {
      _tick!.cancel();
      _tick = null;
    }
  }

  Future<void> _generate() async {
    final client = SessionScope.of(context).client;
    final invitee = _identityId.text.trim();
    if (client == null || !isIdentityId(invitee) || _busy) return;

    // Resolve the expiry: an Advanced custom value (validated positive integer)
    // overrides the chosen preset; otherwise the preset's seconds (null = none).
    int? expirySecs;
    final customText = _customExpiry.text.trim();
    if (_advancedOpen && customText.isNotEmpty) {
      final value = int.tryParse(customText);
      if (value == null || value <= 0) {
        setState(() => _expiryInvalid = true);
        return;
      }
      expirySecs = value;
    } else {
      expirySecs = _presetSeconds(_expiryKey);
    }

    setState(() {
      _busy = true;
      _error = null;
      _expiryInvalid = false;
      _ticket = null;
    });
    try {
      final ticket = await client.inviteCreate(
        roomId: widget.roomId,
        identityId: invitee,
        role: _role,
        expiry: expirySecs,
      );
      if (!mounted) return;
      final secs = expirySecs;
      setState(() {
        _ticket = ticket;
        _mintedIdentityId = invitee;
        _mintedExpiresAtMs = secs != null ? inviteNowMs() + secs * 1000 : null;
      });
      _reconcileTick();
    } catch (e) {
      if (mounted) setState(() => _error = errorShape(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// "New invite": a blank draft (mirrors the React newInvite — role/expiry
  /// choices are kept, identity and any tracked invite are cleared).
  void _newInvite() {
    setState(() {
      _ticket = null;
      _mintedIdentityId = null;
      _mintedExpiresAtMs = null;
      _identityId.text = '';
      _error = null;
      _expiryInvalid = false;
    });
    _reconcileTick();
  }

  /// Lifecycle of the tracked invite from PROVABLE state only (null = nothing
  /// tracked). Joined ONLY from an active roster row (never from the ticket).
  String? _lifecycle(List<Member> members) {
    final id = _mintedIdentityId;
    if (id == null) return null;
    return inviteState(
      id,
      _mintedExpiresAtMs,
      [for (final m in members) m.toJson()],
      inviteNowMs(),
    );
  }

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    // Live address + roster, not the values frozen at open: a reconnect re-open
    // can update RoomStore while this modal is up (endpointAddr feeds the
    // combined invite; members drive the lifecycle chip).
    final store = SessionScope.of(context).room;
    final liveStore = store != null && store.roomId == widget.roomId ? store : null;
    return ModalScaffold(
      title: s.inviteTitle,
      wide: true,
      // Ticket generation publishes a signed member_invited event — contain
      // the route while it is in flight (#55).
      busy: _busy,
      child: ListenableBuilder(
        listenable: liveStore ?? Listenable.merge(const []),
        builder: (context, _) {
          final endpointAddr = liveStore?.endpointAddr ?? widget.endpointAddr;
          final members = liveStore?.members ?? const <Member>[];
          final ticket = _ticket;
          if (ticket == null) return _buildForm(context, members);
          return endpointAddr != null
              ? _buildCombinedResult(context, ticket, endpointAddr, members)
              : _buildTicketOnlyResult(context, ticket, members);
        },
      ),
    );
  }

  // -- form view: identity → role → expiry ---------------------------------------

  Widget _buildForm(BuildContext context, List<Member> members) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final invitee = _identityId.text.trim();
    final idValid = isIdentityId(invitee);
    final idTouched = invitee.isNotEmpty;
    // A restored/pending invite (no ticket to re-show this session).
    final lifecycle = _lifecycle(members);
    final restoredWaiting = _mintedIdentityId != null;
    final canSubmit = !_busy && idValid;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (restoredWaiting && lifecycle != null)
          _LifecycleBlock(state: lifecycle, copy: s.inviteAlreadyInvited)
        else ...[
          Text(s.inviteIntro,
              style: TextStyle(fontSize: 13, color: tokens.textDim)),
          const SizedBox(height: JeliyaSpacing.x12),
          _ReadinessBlock(
            tone: _ReadinessTone.neutral,
            heading: s.inviteRoomOpenForInviting,
            copy: s.inviteRoomOpenForInvitingCopy,
          ),
        ],
        const SizedBox(height: JeliyaSpacing.x12),

        // 1. Identity — validated inline against isIdentityId (bare 64-hex).
        //    Submit is disabled until valid, so an obvious typo fails in the
        //    form, never as a daemon invalid_params error.
        _FieldLabel(s.inviteInviteeIdentityId),
        TextField(
          controller: _identityId,
          autofocus: true,
          style: JeliyaText.mono(fontSize: 12.5, color: tokens.text),
          decoration: InputDecoration(
            hintText: s.inviteInviteePlaceholder,
            errorText: idTouched && !idValid ? s.inviteIdentityInvalid : null,
          ),
          onSubmitted: (_) => _generate(),
        ),
        if (!(idTouched && !idValid)) ...[
          const SizedBox(height: JeliyaSpacing.x4),
          Text(s.inviteIdentityHint,
              style: TextStyle(fontSize: 12, color: tokens.textDim)),
        ],
        const SizedBox(height: JeliyaSpacing.x12),

        // 2. Role — a one-line consequence each, and the Add-Agent security
        //    warning when agent is selected.
        _FieldLabel(s.inviteRoleLabel),
        _RoleOption(
          selected: _role == Roles.member,
          title: s.rolePill(Roles.member),
          consequence: s.inviteRoleMemberConsequence,
          onTap: () => setState(() => _role = Roles.member),
        ),
        const SizedBox(height: JeliyaSpacing.x6),
        _RoleOption(
          selected: _role == Roles.agent,
          title: s.rolePill(Roles.agent),
          consequence: s.inviteRoleAgentConsequence,
          onTap: () => setState(() => _role = Roles.agent),
        ),
        if (_role == Roles.agent) ...[
          const SizedBox(height: JeliyaSpacing.x10),
          Semantics(
            liveRegion: true, // role="alert"
            child: Container(
              padding: const EdgeInsets.all(JeliyaSpacing.x10),
              decoration: BoxDecoration(
                color: tokens.errorNoteBg,
                borderRadius: BorderRadius.circular(JeliyaRadii.btn),
                border: Border.all(color: tokens.errorNoteBorder),
              ),
              child: Text(
                s.inviteAgentWarning,
                style: TextStyle(fontSize: 13, color: tokens.text),
              ),
            ),
          ),
        ],
        const SizedBox(height: JeliyaSpacing.x12),

        // 3. Expiry — presets over expiryPresets, plus an Advanced custom
        //    seconds field (when set, it overrides the selected preset).
        _FieldLabel(s.inviteTicketExpiryLabel),
        _ExpiryPresets(
          selectedKey: _expiryKey,
          customActive: _advancedOpen && _customExpiry.text.trim().isNotEmpty,
          onSelect: (key) => setState(() {
            _expiryKey = key;
            _customExpiry.clear();
          }),
        ),
        const SizedBox(height: JeliyaSpacing.x8),
        _AdvancedExpiry(
          open: _advancedOpen,
          controller: _customExpiry,
          onToggle: () => setState(() => _advancedOpen = !_advancedOpen),
          onChanged: () => setState(() {}),
          onSubmitted: _generate,
        ),
        const SizedBox(height: JeliyaSpacing.x12),

        JeliyaButton(
          label: _busy
              ? s.inviteGenerating
              : restoredWaiting
                  ? s.inviteSendFresh
                  : s.inviteGenerateTicket,
          variant: JeliyaButtonVariant.primary,
          busy: _busy,
          onPressed: canSubmit ? _generate : null,
        ),
        _errorNote(context),
      ],
    );
  }

  // -- result: combined ticket#address -------------------------------------------

  Widget _buildCombinedResult(BuildContext context, String ticket,
      String endpointAddr, List<Member> members) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final combined = buildCombinedInvite(ticket, endpointAddr);
    final lifecycle = _lifecycle(members);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _ReadinessBlock(
          tone: _ReadinessTone.ready,
          heading: s.inviteReadyToSend,
          copy: s.inviteReadyToSendCopy,
        ),
        if (lifecycle != null) ...[
          const SizedBox(height: JeliyaSpacing.x10),
          _LifecycleBlock(state: lifecycle, copy: _lifecycleCopy(s, lifecycle)),
        ],
        const SizedBox(height: JeliyaSpacing.x10),
        Text(s.inviteCombinedCopy,
            style: TextStyle(fontSize: 13, color: tokens.textDim)),
        const SizedBox(height: JeliyaSpacing.x10),
        _TicketBox(
          value: combined,
          semanticLabel: s.inviteCombinedInviteLabel,
          copyLabel: s.inviteCopyInvite,
        ),
        // Phones get the OS share sheet next to clipboard copy — a paste target
        // is rarely one alt-tab away on a phone. Desktop stays copy-only.
        if (isMobileWidth(context)) ...[
          const SizedBox(height: JeliyaSpacing.x10),
          _ShareButton(text: combined, label: s.inviteShareInvite),
        ],
        // QR of the SAME combined invite the Copy button carries (#103). The
        // hand-vendored encoder returns nothing if the payload is too large for
        // any symbol, leaving Copy/Share as the fallback.
        const SizedBox(height: JeliyaSpacing.x12),
        Center(
          child: QrView(
            value: combined,
            semanticLabel: s.inviteQrLabel,
            caption: s.inviteQrCombinedCaption,
            captionStyle: TextStyle(fontSize: 12.5, color: tokens.textDim),
          ),
        ),
        const SizedBox(height: JeliyaSpacing.x12),
        _SeparatePartsDisclosure(ticket: ticket, endpointAddr: endpointAddr),
        const SizedBox(height: JeliyaSpacing.x14),
        _lifecycleActions(context, lifecycle),
        _errorNote(context),
      ],
    );
  }

  // -- result: ticket only (no dialable address) ---------------------------------

  Widget _buildTicketOnlyResult(
      BuildContext context, String ticket, List<Member> members) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final lifecycle = _lifecycle(members);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _ReadinessBlock(
          tone: _ReadinessTone.caution,
          heading: s.inviteNoDialableAddress,
          copy: s.inviteNoDialableAddressCopy,
        ),
        if (lifecycle != null) ...[
          const SizedBox(height: JeliyaSpacing.x10),
          _LifecycleBlock(state: lifecycle, copy: _lifecycleCopy(s, lifecycle)),
        ],
        const SizedBox(height: JeliyaSpacing.x10),
        Text(s.inviteTicketOnlyCopy,
            style: TextStyle(fontSize: 13, color: tokens.textDim)),
        const SizedBox(height: JeliyaSpacing.x10),
        _TicketBox(
          value: ticket,
          semanticLabel: s.inviteInviteTicketLabel,
          copyLabel: s.inviteCopyTicket,
        ),
        if (isMobileWidth(context)) ...[
          const SizedBox(height: JeliyaSpacing.x10),
          _ShareButton(text: ticket, label: s.inviteShareTicket),
        ],
        // QR of the bare ticket the Copy button carries (#103); same graceful
        // fallback as the combined surface.
        const SizedBox(height: JeliyaSpacing.x12),
        Center(
          child: QrView(
            value: ticket,
            semanticLabel: s.inviteQrLabel,
            caption: s.inviteQrTicketCaption,
            captionStyle: TextStyle(fontSize: 12.5, color: tokens.textDim),
          ),
        ),
        const SizedBox(height: JeliyaSpacing.x10),
        Text(s.inviteNoDialableAddressNote,
            style: TextStyle(fontSize: 13, color: tokens.textDim)),
        const SizedBox(height: JeliyaSpacing.x14),
        _lifecycleActions(context, lifecycle),
        _errorNote(context),
      ],
    );
  }

  // -- shared result pieces ------------------------------------------------------

  /// The result-view actions: an explicit Invite-Again on expiry (re-mints the
  /// same identity/role/expiry), then the New-invite reset. Scale-down guarded
  /// so wide locales / large text scales shrink rather than overflow.
  Widget _lifecycleActions(BuildContext context, String? lifecycle) {
    final s = context.strings;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (lifecycle == InviteStates.expired) ...[
          FittedBox(
            fit: BoxFit.scaleDown,
            alignment: Alignment.centerLeft,
            child: JeliyaButton(
              label: _busy ? s.inviteGenerating : s.inviteAgain,
              variant: JeliyaButtonVariant.primary,
              busy: _busy,
              onPressed: _busy ? null : _generate,
            ),
          ),
          const SizedBox(height: JeliyaSpacing.x10),
        ],
        FittedBox(
          fit: BoxFit.scaleDown,
          alignment: Alignment.centerLeft,
          child: JeliyaButton(
            label: s.inviteNewInvite,
            variant: JeliyaButtonVariant.ghost,
            onPressed: _newInvite,
          ),
        ),
      ],
    );
  }

  /// The ErrorNote, resolving the client-local expiry error's copy per build.
  Widget _errorNote(BuildContext context) {
    final s = context.strings;
    return ErrorNote(
      error: _expiryInvalid
          ? RequestError(
              ErrorCodes.invalidParams,
              s.inviteExpiryErrorMessage,
              hint: s.inviteExpiryErrorHint,
            )
          : _error,
      friendly: _expiryInvalid
          ? FriendlyError(
              title: s.inviteExpiryErrorTitle,
              message: s.inviteExpiryErrorMessage,
              action: s.inviteExpiryErrorHint,
            )
          : null,
    );
  }

  String _lifecycleCopy(AppStrings s, String state) => switch (state) {
        InviteStates.joined => s.inviteLifecycleJoinedCopy,
        InviteStates.expired => s.inviteLifecycleExpiredCopy,
        _ => s.inviteLifecycleWaitingCopy,
      };
}

// -- pieces ------------------------------------------------------------------------------

/// A lifecycle chip + explanatory copy, announced as a live region (role=status).
class _LifecycleBlock extends StatelessWidget {
  const _LifecycleBlock({required this.state, required this.copy});

  final String state;
  final String copy;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    return Semantics(
      liveRegion: true,
      container: true,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          _LifecycleChip(state: state),
          const SizedBox(width: JeliyaSpacing.x8),
          Expanded(
            child: Padding(
              padding: const EdgeInsets.only(top: 2),
              child: Text(copy,
                  style: TextStyle(fontSize: 12.5, color: tokens.textDim)),
            ),
          ),
        ],
      ),
    );
  }
}

/// Waiting / Expired / Joined — a tinted pill with a status dot. Joined is
/// green (earned); expired is red; waiting is neutral.
class _LifecycleChip extends StatelessWidget {
  const _LifecycleChip({required this.state});

  final String state;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final (LabelTone tone, String label) = switch (state) {
      InviteStates.joined => (LabelTone.green, s.inviteLifecycleJoined),
      InviteStates.expired => (LabelTone.red, s.inviteLifecycleExpired),
      _ => (LabelTone.neutral, s.inviteLifecycleWaiting),
    };
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 3),
      decoration: BoxDecoration(
        color: tokens.toneBg(tone),
        borderRadius: BorderRadius.circular(JeliyaRadii.pill),
        border: Border.all(color: tokens.toneBorder(tone)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          ExcludeSemantics(
            child: Container(
              width: 7,
              height: 7,
              decoration: BoxDecoration(
                  color: tokens.toneColor(tone), shape: BoxShape.circle),
            ),
          ),
          const SizedBox(width: JeliyaSpacing.x6),
          Text(label,
              style: TextStyle(
                  fontSize: 11.5,
                  letterSpacing: 0.22,
                  color: tokens.toneColor(tone))),
        ],
      ),
    );
  }
}

/// A selectable role row: a radio indicator + bold title + one-line consequence.
class _RoleOption extends StatelessWidget {
  const _RoleOption({
    required this.selected,
    required this.title,
    required this.consequence,
    required this.onTap,
  });

  final bool selected;
  final String title;
  final String consequence;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    return Semantics(
      inMutuallyExclusiveGroup: true,
      checked: selected,
      button: true,
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(JeliyaRadii.row),
        child: Padding(
          padding: const EdgeInsets.symmetric(
              horizontal: JeliyaSpacing.x8, vertical: JeliyaSpacing.x8),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Padding(
                padding: const EdgeInsets.only(top: 2),
                child: Container(
                  width: 16,
                  height: 16,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    border: Border.all(
                        color: selected ? tokens.accent : tokens.borderStrong,
                        width: 2),
                  ),
                  child: selected
                      ? Center(
                          child: Container(
                            width: 8,
                            height: 8,
                            decoration: BoxDecoration(
                                color: tokens.accent, shape: BoxShape.circle),
                          ),
                        )
                      : null,
                ),
              ),
              const SizedBox(width: JeliyaSpacing.x10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(title,
                        style: TextStyle(
                            fontSize: 13.5,
                            fontWeight: FontWeight.w700,
                            color: tokens.text)),
                    const SizedBox(height: JeliyaSpacing.x2),
                    Text(consequence,
                        style:
                            TextStyle(fontSize: 12.5, color: tokens.textDim)),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// The expiry preset chooser: one selectable chip per [expiryPresets] entry.
class _ExpiryPresets extends StatelessWidget {
  const _ExpiryPresets({
    required this.selectedKey,
    required this.customActive,
    required this.onSelect,
  });

  final String selectedKey;

  /// True when an Advanced custom value is set — then no preset reads selected.
  final bool customActive;
  final ValueChanged<String> onSelect;

  static String _label(AppStrings s, String key) => switch (key) {
        '1h' => s.inviteExpiry1h,
        '24h' => s.inviteExpiry24h,
        '7d' => s.inviteExpiry7d,
        'never' => s.inviteExpiryNever,
        _ => key,
      };

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    return Wrap(
      spacing: JeliyaSpacing.x8,
      runSpacing: JeliyaSpacing.x8,
      children: [
        for (final p in expiryPresets)
          _ExpiryChip(
            label: _label(s, p.key),
            selected: !customActive && selectedKey == p.key,
            onTap: () => onSelect(p.key),
          ),
      ],
    );
  }
}

class _ExpiryChip extends StatelessWidget {
  const _ExpiryChip({
    required this.label,
    required this.selected,
    required this.onTap,
  });

  final String label;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    return Semantics(
      button: true,
      selected: selected,
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(JeliyaRadii.btn),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
          decoration: BoxDecoration(
            color: selected ? tokens.accentDim : tokens.bgInput,
            borderRadius: BorderRadius.circular(JeliyaRadii.btn),
            border: Border.all(
                color: selected ? tokens.accentLine : tokens.borderStrong),
          ),
          child: Text(
            label,
            style: TextStyle(
                fontSize: 12.5,
                color: selected ? tokens.accent : tokens.textDim),
          ),
        ),
      ),
    );
  }
}

/// The Advanced disclosure holding the custom-seconds field (overrides preset).
class _AdvancedExpiry extends StatelessWidget {
  const _AdvancedExpiry({
    required this.open,
    required this.controller,
    required this.onToggle,
    required this.onChanged,
    required this.onSubmitted,
  });

  final bool open;
  final TextEditingController controller;
  final VoidCallback onToggle;
  final VoidCallback onChanged;
  final Future<void> Function() onSubmitted;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Semantics(
          button: true,
          expanded: open,
          child: InkWell(
            onTap: onToggle,
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: JeliyaSpacing.x4),
              child: Text(
                '${open ? '▾' : '▸'} ${s.inviteAdvancedExpiry}',
                style: TextStyle(fontSize: 12.5, color: tokens.textDim),
              ),
            ),
          ),
        ),
        if (open) ...[
          const SizedBox(height: JeliyaSpacing.x6),
          _FieldLabel(fillTemplate(
              s.commonOptionalFieldLabel('{label}', '{optional}'), {
            'label': s.inviteCustomExpiryLabel,
            'optional': s.inviteCustomExpiryOverride,
          })),
          TextField(
            controller: controller,
            keyboardType: TextInputType.number,
            decoration:
                const InputDecoration(hintText: Tokens.expiryPlaceholderExample),
            onChanged: (_) => onChanged(),
            onSubmitted: (_) => onSubmitted(),
          ),
        ],
      ],
    );
  }
}

enum _ReadinessTone { neutral, ready, caution }

/// The invite-readiness block: status dot + bold heading + secondary copy.
class _ReadinessBlock extends StatelessWidget {
  const _ReadinessBlock({
    required this.tone,
    required this.heading,
    required this.copy,
  });

  final _ReadinessTone tone;
  final String heading;
  final String copy;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final (Color bg, Color borderColor, Color dotColor, List<BoxShadow>? glow) =
        switch (tone) {
      _ReadinessTone.ready => (
          tokens.accentDim,
          tokens.accentLine,
          tokens.accent,
          [
            BoxShadow(
                color: tokens.accent.withValues(alpha: 0.7), blurRadius: 6)
          ],
        ),
      _ReadinessTone.caution => (
          tokens.amberDim,
          tokens.amberLine,
          tokens.amber,
          null,
        ),
      _ReadinessTone.neutral => (
          tokens.bgCard,
          tokens.borderStrong,
          tokens.accent,
          [
            BoxShadow(
                color: tokens.accent.withValues(alpha: 0.7), blurRadius: 6)
          ],
        ),
    };
    return Container(
      padding: const EdgeInsets.symmetric(
          horizontal: JeliyaSpacing.x12, vertical: JeliyaSpacing.x10),
      decoration: BoxDecoration(
        color: bg,
        borderRadius: BorderRadius.circular(JeliyaRadii.row),
        border: Border.all(color: borderColor),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(top: 5),
            child: ExcludeSemantics(
              child: Container(
                width: 7,
                height: 7,
                decoration: BoxDecoration(
                  color: dotColor,
                  shape: BoxShape.circle,
                  boxShadow: glow,
                ),
              ),
            ),
          ),
          const SizedBox(width: JeliyaSpacing.x10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(heading,
                    style: TextStyle(
                        fontSize: 13.5,
                        fontWeight: FontWeight.w700,
                        color: tokens.text)),
                const SizedBox(height: JeliyaSpacing.x2),
                Text(copy,
                    style: TextStyle(fontSize: 12.5, color: tokens.textDim)),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// Read-only mono ticket textarea (select-all on focus) + copy button.
class _TicketBox extends StatefulWidget {
  const _TicketBox({
    required this.value,
    required this.semanticLabel,
    required this.copyLabel,
  });

  final String value;
  final String semanticLabel;
  final String copyLabel;

  @override
  State<_TicketBox> createState() => _TicketBoxState();
}

class _TicketBoxState extends State<_TicketBox> {
  late final TextEditingController _controller =
      TextEditingController(text: widget.value);
  late final FocusNode _focus = FocusNode();

  @override
  void initState() {
    super.initState();
    _focus.addListener(() {
      // Select-all on focus, like the reference textarea.
      if (_focus.hasFocus) {
        _controller.selection = TextSelection(
            baseOffset: 0, extentOffset: _controller.text.length);
      }
    });
  }

  @override
  void didUpdateWidget(covariant _TicketBox oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.value != widget.value) _controller.text = widget.value;
  }

  @override
  void dispose() {
    _focus.dispose();
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Expanded(
          child: Semantics(
            label: widget.semanticLabel,
            child: TextField(
              controller: _controller,
              focusNode: _focus,
              readOnly: true,
              minLines: 4,
              maxLines: 4,
              style: JeliyaText.mono(fontSize: 12, color: tokens.text),
            ),
          ),
        ),
        const SizedBox(width: JeliyaSpacing.x10),
        CopyButton(text: widget.value, label: widget.copyLabel),
      ],
    );
  }
}

/// OS share sheet affordance — rendered ONLY below the shell breakpoint (the
/// call sites gate on [isMobileWidth]). Hands the share sheet the SAME string
/// the sibling [CopyButton] copies; the sheet itself is the feedback.
class _ShareButton extends StatelessWidget {
  const _ShareButton({required this.text, required this.label});

  /// What gets shared — byte-identical to the copy button's payload.
  final String text;

  final String label;

  @override
  Widget build(BuildContext context) {
    return JeliyaButton(
      label: label,
      size: JeliyaButtonSize.sm,
      onPressed: () {
        // iPads present the sheet as a popover and require an anchor rect;
        // pass this button's bounds (ignored on phones/Android).
        final box = context.findRenderObject() as RenderBox?;
        final origin = box != null && box.hasSize
            ? box.localToGlobal(Offset.zero) & box.size
            : null;
        SharePlus.instance
            .share(ShareParams(text: text, sharePositionOrigin: origin));
      },
    );
  }
}

/// `<details>` 'Send the ticket and address separately'.
class _SeparatePartsDisclosure extends StatefulWidget {
  const _SeparatePartsDisclosure({required this.ticket, required this.endpointAddr});

  final String ticket;
  final String endpointAddr;

  @override
  State<_SeparatePartsDisclosure> createState() =>
      _SeparatePartsDisclosureState();
}

class _SeparatePartsDisclosureState extends State<_SeparatePartsDisclosure> {
  bool _open = false;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        InkWell(
          onTap: () => setState(() => _open = !_open),
          child: Text(
            '${_open ? '▾' : '▸'} ${s.inviteSeparatelySummary}',
            style: TextStyle(fontSize: 12.5, color: tokens.textDim),
          ),
        ),
        if (_open) ...[
          const SizedBox(height: JeliyaSpacing.x10),
          _TicketBox(
            value: widget.ticket,
            semanticLabel: s.inviteInviteTicketLabel,
            copyLabel: s.inviteCopyTicket,
          ),
          const SizedBox(height: JeliyaSpacing.x10),
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Container(
                  padding: const EdgeInsets.symmetric(
                      horizontal: JeliyaSpacing.x10, vertical: 9),
                  decoration: BoxDecoration(
                    color: tokens.bgInput,
                    borderRadius: BorderRadius.circular(JeliyaRadii.btn),
                    border: Border.all(color: tokens.borderStrong),
                  ),
                  child: SelectableText(
                    widget.endpointAddr,
                    style: JeliyaText.mono(fontSize: 12, color: tokens.textDim),
                  ),
                ),
              ),
              const SizedBox(width: JeliyaSpacing.x10),
              CopyButton(
                  text: widget.endpointAddr, label: s.inviteCopyAddress),
            ],
          ),
        ],
      ],
    );
  }
}

class _FieldLabel extends StatelessWidget {
  const _FieldLabel(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 5),
      child:
          Text(text, style: TextStyle(fontSize: 12.5, color: tokens.textDim)),
    );
  }
}
