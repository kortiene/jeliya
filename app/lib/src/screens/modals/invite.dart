/// Invite modal (wide) — InviteModal.tsx port per phase3-features.json
/// "Invite modal (wide)": mints an identity-bound ticket via `invite.create`
/// and presents the combined `ticket#address` paste. The dialable address is
/// the `room.open` result the shell passes in (RoomStore.endpointAddr) — NOT
/// a fresh call. Client-side expiry validation mints a local `invalid_params`
/// [RequestError] before any wire call.
library;

import 'package:flutter/material.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart'
    show ErrorCodes, JeliyaMethods, RequestError, Roles, errorShape;

import '../../l10n/error_display.dart';
import '../../l10n/strings_context.dart';
import '../../l10n/tokens.dart';
import '../../l10n/wire_display.dart';
import '../../layout.dart';
import '../../session/daemon_session.dart';
import '../../theme.dart';
import '../../widgets/buttons.dart';
import '../../widgets/copy_button.dart';
import '../../widgets/error_note.dart';
import '../../widgets/modal_scaffold.dart';
import '../../widgets/template_text.dart';

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
  final TextEditingController _expiry = TextEditingController();

  /// Wire role value ([Roles]) — the dropdown DISPLAYS the mapped label but
  /// stores and sends the protocol constant.
  String _role = Roles.member;
  bool _busy = false;
  String? _ticket;
  RequestError? _error;

  /// Marker for the client-local expiry validation failure — its copy is
  /// built AT RENDER TIME (never cached in state, so a live locale switch
  /// re-resolves it). A wire invalid_params must NOT set this: it keeps the
  /// generic friendlyError mapping.
  bool _expiryInvalid = false;

  @override
  void initState() {
    super.initState();
    _identityId.addListener(() => setState(() {}));
  }

  @override
  void dispose() {
    _identityId.dispose();
    _expiry.dispose();
    super.dispose();
  }

  Future<void> _generate() async {
    final client = SessionScope.of(context).client;
    final invitee = _identityId.text.trim();
    if (client == null || invitee.isEmpty || _busy) return;
    setState(() {
      _busy = true;
      _error = null;
      _expiryInvalid = false;
      _ticket = null;
    });
    try {
      final expiryText = _expiry.text.trim();
      int? expirySecs;
      if (expiryText.isNotEmpty) {
        final value = int.tryParse(expiryText);
        if (value == null || value <= 0) {
          setState(() => _expiryInvalid = true);
          return;
        }
        expirySecs = value;
      }
      final ticket = await client.inviteCreate(
        roomId: widget.roomId,
        identityId: invitee,
        role: _role,
        expiry: expirySecs,
      );
      if (mounted) setState(() => _ticket = ticket);
    } catch (e) {
      if (mounted) setState(() => _error = errorShape(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final ticket = _ticket;
    // Live address, not the value frozen at open: a reconnect re-open can
    // update RoomStore.endpointAddr while this modal is up, and the combined
    // invite must follow it (widget param is the fallback for a swapped room).
    final store = SessionScope.of(context).room;
    final liveStore = store != null && store.roomId == widget.roomId ? store : null;
    return ModalScaffold(
      title: s.inviteTitle,
      wide: true,
      child: ListenableBuilder(
        listenable: liveStore ?? Listenable.merge(const []),
        builder: (context, _) {
          final endpointAddr = liveStore?.endpointAddr ?? widget.endpointAddr;
          return ticket == null
              ? _buildForm(context)
              : endpointAddr != null
                  ? _buildCombinedResult(context, ticket, endpointAddr)
                  : _buildTicketOnlyResult(context, ticket);
        },
      ),
    );
  }

  // -- form view -------------------------------------------------------------------

  Widget _buildForm(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final canSubmit = !_busy && _identityId.text.trim().isNotEmpty;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(s.inviteIntro,
            style: TextStyle(fontSize: 13, color: tokens.textDim)),
        const SizedBox(height: JeliyaSpacing.x12),
        _ReadinessBlock(
          tone: _ReadinessTone.neutral,
          heading: s.inviteRoomOpenForInviting,
          copy: s.inviteRoomOpenForInvitingCopy,
        ),
        const SizedBox(height: JeliyaSpacing.x12),
        _FieldLabel(s.inviteInviteeIdentityId),
        TextField(
          controller: _identityId,
          autofocus: true,
          style: JeliyaText.mono(fontSize: 12.5, color: tokens.text),
          decoration:
              InputDecoration(hintText: s.inviteInviteePlaceholder),
          onSubmitted: (_) => _generate(),
        ),
        const SizedBox(height: JeliyaSpacing.x10),
        ..._buildRoleAndExpiry(context),
        const SizedBox(height: JeliyaSpacing.x12),
        JeliyaButton(
          label: _busy ? s.inviteGenerating : s.inviteGenerateTicket,
          variant: JeliyaButtonVariant.primary,
          busy: _busy,
          onPressed: canSubmit ? _generate : null,
        ),
        // The client-local expiry error resolves its copy here, per build.
        ErrorNote(
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
        ),
      ],
    );
  }

  /// Role + expiry: two columns sized for the 560 dialog; stacked below the
  /// width breakpoint (the phone full-screen route) so each field keeps a
  /// usable width.
  List<Widget> _buildRoleAndExpiry(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final role = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _FieldLabel(s.inviteRoleLabel),
        DropdownButtonFormField<String>(
          initialValue: _role,
          items: [
            DropdownMenuItem(
              value: Roles.member,
              child: Text(s.roleInline(Roles.member)),
            ),
            DropdownMenuItem(
              value: Roles.agent,
              child: Text(s.roleInline(Roles.agent)),
            ),
          ],
          onChanged: (value) => setState(() => _role = value ?? Roles.member),
          style: TextStyle(fontSize: 14, color: tokens.text),
          dropdownColor: tokens.bgCard,
        ),
      ],
    );
    final expiry = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _FieldLabel(
            fillTemplate(s.commonOptionalFieldLabel('{label}', '{optional}'), {
          'label': s.inviteExpiryLabel,
          'optional': s.inviteExpiryOptional,
        })),
        TextField(
          controller: _expiry,
          keyboardType: TextInputType.number,
          decoration:
              const InputDecoration(hintText: Tokens.expiryPlaceholderExample),
          onSubmitted: (_) => _generate(),
        ),
      ],
    );
    if (isMobileWidth(context)) {
      return [role, const SizedBox(height: JeliyaSpacing.x10), expiry];
    }
    return [
      Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(child: role),
          const SizedBox(width: JeliyaSpacing.x10),
          Expanded(child: expiry),
        ],
      ),
    ];
  }

  // -- result: combined ticket#address ------------------------------------------------

  Widget _buildCombinedResult(
      BuildContext context, String ticket, String endpointAddr) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final combined = '$ticket#$endpointAddr';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _ReadinessBlock(
          tone: _ReadinessTone.ready,
          heading: s.inviteReadyToSend,
          copy: s.inviteReadyToSendCopy,
        ),
        const SizedBox(height: JeliyaSpacing.x10),
        Text(s.inviteCombinedCopy,
            style: TextStyle(fontSize: 13, color: tokens.textDim)),
        const SizedBox(height: JeliyaSpacing.x10),
        _TicketBox(
          value: combined,
          semanticLabel: s.inviteCombinedInviteLabel,
          copyLabel: s.inviteCopyInvite,
        ),
        const SizedBox(height: JeliyaSpacing.x10),
        _SeparatePartsDisclosure(ticket: ticket, endpointAddr: endpointAddr),
        const SizedBox(height: JeliyaSpacing.x14),
        JeliyaButton(
          label: s.inviteNewInvite,
          variant: JeliyaButtonVariant.ghost,
          onPressed: () => setState(() => _ticket = null),
        ),
      ],
    );
  }

  // -- result: ticket only (no dialable address) ----------------------------------------

  Widget _buildTicketOnlyResult(BuildContext context, String ticket) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _ReadinessBlock(
          tone: _ReadinessTone.caution,
          heading: s.inviteNoDialableAddress,
          copy: s.inviteNoDialableAddressCopy,
        ),
        const SizedBox(height: JeliyaSpacing.x10),
        Text(s.inviteTicketOnlyCopy,
            style: TextStyle(fontSize: 13, color: tokens.textDim)),
        const SizedBox(height: JeliyaSpacing.x10),
        _TicketBox(
          value: ticket,
          semanticLabel: s.inviteInviteTicketLabel,
          copyLabel: s.inviteCopyTicket,
        ),
        const SizedBox(height: JeliyaSpacing.x10),
        Text(s.inviteNoDialableAddressNote,
            style: TextStyle(fontSize: 13, color: tokens.textDim)),
        const SizedBox(height: JeliyaSpacing.x14),
        JeliyaButton(
          label: s.inviteNewInvite,
          variant: JeliyaButtonVariant.ghost,
          onPressed: () => setState(() => _ticket = null),
        ),
      ],
    );
  }
}

// -- pieces ----------------------------------------------------------------------------

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
