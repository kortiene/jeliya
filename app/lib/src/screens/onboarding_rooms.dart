/// Onboarding — rooms step (phase 'no-rooms'), exact port of Onboarding.tsx
/// `RoomsStep` per phase3-features.json: identity card (with copy button),
/// side-by-side Create / Join cards, join retry via the package
/// `joinRoomWithRetry` + `splitInvite`, and the live progress row.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:jeliya_protocol/jeliya_protocol.dart';

import '../l10n/strings_context.dart';
import '../l10n/tokens.dart';
import '../layout.dart';
import '../session/daemon_session.dart';
import '../theme.dart';
import '../widgets/buttons.dart';
import '../widgets/copy_button.dart';
import '../widgets/error_note.dart';
import '../widgets/template_text.dart';
import 'onboarding_identity.dart' show OnboardingBrand, OnboardingCard;

class OnboardingRoomsScreen extends StatefulWidget {
  const OnboardingRoomsScreen({super.key});

  @override
  State<OnboardingRoomsScreen> createState() => _OnboardingRoomsScreenState();
}

class _OnboardingRoomsScreenState extends State<OnboardingRoomsScreen> {
  final TextEditingController _name = TextEditingController();
  bool _creating = false;
  RequestError? _createError;

  final TextEditingController _ticket = TextEditingController();
  final TextEditingController _peerAddr = TextEditingController();
  bool _joining = false;
  RequestError? _joinError;
  JoinProgress? _joinProgress;

  @override
  void initState() {
    super.initState();
    // Re-render the submit-enabled state as the fields change.
    _name.addListener(_onFieldChanged);
    _ticket.addListener(_onFieldChanged);
  }

  void _onFieldChanged() => setState(() {});

  @override
  void dispose() {
    _name.dispose();
    _ticket.dispose();
    _peerAddr.dispose();
    super.dispose();
  }

  Future<void> _create() async {
    final session = SessionScope.of(context);
    final client = session.client;
    final name = _name.text.trim();
    if (client == null || name.isEmpty || _creating) return;
    setState(() {
      _creating = true;
      _createError = null;
    });
    try {
      await client.roomCreate(name);
      session.advanceOnboarding();
    } catch (e) {
      if (mounted) setState(() => _createError = errorShape(e));
    } finally {
      if (mounted) setState(() => _creating = false);
    }
  }

  Future<void> _join() async {
    final session = SessionScope.of(context);
    final client = session.client;
    if (client == null || _ticket.text.trim().isEmpty || _joining) return;
    setState(() {
      _joining = true;
      _joinError = null;
      _joinProgress = null;
    });
    try {
      final invite = splitInvite(_ticket.text, _peerAddr.text);
      await joinRoomWithRetry(
        client,
        ticket: invite.ticket,
        peers: invite.peerAddr.isEmpty ? null : [invite.peerAddr],
        onProgress: (progress) {
          if (mounted) setState(() => _joinProgress = progress);
        },
      );
      session.advanceOnboarding();
    } catch (e) {
      if (mounted) {
        setState(() {
          _joinError = errorShape(e);
          _joinProgress = null;
        });
      }
    } finally {
      if (mounted) setState(() => _joining = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final session = SessionScope.of(context);
    final identityId = session.selfId;
    return Scaffold(
      body: Center(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(JeliyaSpacing.page),
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 880),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                const OnboardingBrand(),
                const SizedBox(height: JeliyaSpacing.x24),
                if (identityId != null) ...[
                  _IdentityCard(identityId: identityId),
                  const SizedBox(height: JeliyaSpacing.x16),
                ],
                // Side-by-side cards need ~880px; below the breakpoint they
                // stack (same cards, same copy — only the axis forks).
                if (isMobileWidth(context))
                  Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      _buildCreateCard(context),
                      const SizedBox(height: JeliyaSpacing.x16),
                      _buildJoinCard(context),
                    ],
                  )
                else
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Expanded(child: _buildCreateCard(context)),
                      const SizedBox(width: JeliyaSpacing.x16),
                      Expanded(child: _buildJoinCard(context)),
                    ],
                  ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildCreateCard(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final canSubmit = !_creating && _name.text.trim().isNotEmpty;
    return OnboardingCard(
      width: double.infinity,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(s.modalCreateRoomTitle,
              style: JeliyaText.onboardingCardTitle),
          const SizedBox(height: JeliyaSpacing.x8),
          Text(s.onboardingCreateRoomCopy,
              style: TextStyle(fontSize: 13, color: tokens.textDim)),
          const SizedBox(height: JeliyaSpacing.x12),
          _FieldLabel(s.modalRoomNameLabel),
          TextField(
            controller: _name,
            autofocus: true,
            decoration: InputDecoration(
                hintText: s.modalRoomNamePlaceholder),
            onSubmitted: (_) => _create(),
          ),
          const SizedBox(height: JeliyaSpacing.x12),
          JeliyaButton(
            label: _creating
                ? s.modalCreatingRoom
                : s.modalCreateRoom,
            variant: JeliyaButtonVariant.primary,
            busy: _creating,
            onPressed: canSubmit ? _create : null,
          ),
          ErrorNote(error: _createError),
        ],
      ),
    );
  }

  Widget _buildJoinCard(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final canSubmit = !_joining && _ticket.text.trim().isNotEmpty;
    final progress = _joinProgress;
    return OnboardingCard(
      width: double.infinity,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(s.modalJoinRoomTitle,
              style: JeliyaText.onboardingCardTitle),
          const SizedBox(height: JeliyaSpacing.x8),
          // 'ticket#address' renders mono inside this copy.
          templateText(
            s.modalJoinCopy('{combined}'),
            style: TextStyle(fontSize: 13, color: tokens.textDim),
            slots: {
              'combined': TextSpan(
                  text: Tokens.modalJoinCopyMono,
                  style: JeliyaText.mono(fontSize: 12, color: tokens.textDim)),
            },
          ),
          const SizedBox(height: JeliyaSpacing.x12),
          _FieldLabel(s.modalTicketLabel),
          TextField(
            controller: _ticket,
            minLines: 3,
            maxLines: 3,
            style: JeliyaText.mono(fontSize: 12.5),
            decoration: InputDecoration(
                hintText: s.modalTicketPlaceholder),
          ),
          const SizedBox(height: JeliyaSpacing.x10),
          _FieldLabel(fillTemplate(s.commonOptionalFieldLabel(
              '{label}', '{optional}'), {
            'label': s.modalPeerAddrLabel,
            'optional': s.modalPeerAddrOptional,
          })),
          TextField(
            controller: _peerAddr,
            style: JeliyaText.mono(fontSize: 12.5),
            decoration: const InputDecoration(
                hintText: Tokens.modalPeerAddrPlaceholder),
            onSubmitted: (_) => _join(),
          ),
          const SizedBox(height: JeliyaSpacing.x12),
          JeliyaButton(
            label: _joining
                ? s.modalJoiningRoom
                : s.modalJoinRoom,
            variant: JeliyaButtonVariant.primary,
            busy: _joining,
            onPressed: canSubmit ? _join : null,
          ),
          if (progress != null) ...[
            const SizedBox(height: JeliyaSpacing.x10),
            JoinProgressRow(progress: progress),
          ],
          ErrorNote(error: _joinError),
        ],
      ),
    );
  }
}

/// The identity card shown above the two onboarding columns.
class _IdentityCard extends StatelessWidget {
  const _IdentityCard({required this.identityId});

  final String identityId;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    return OnboardingCard(
      width: double.infinity,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(s.onboardingYourIdentityId,
                    style: JeliyaText.microLabel),
              ),
              CopyButton(
                text: identityId,
                label: s.commonCopy,
                semanticLabel: s.commonCopyIdentityId,
              ),
            ],
          ),
          const SizedBox(height: JeliyaSpacing.x6),
          SelectableText(
            identityId,
            style: JeliyaText.mono(fontSize: 12.5, color: tokens.text),
          ),
          const SizedBox(height: JeliyaSpacing.x8),
          Text(s.onboardingIdentityCardCopy1,
              style: TextStyle(fontSize: 12.5, color: tokens.textDim)),
          const SizedBox(height: JeliyaSpacing.x4),
          Text(s.onboardingIdentityCardCopy2,
              style: TextStyle(fontSize: 12.5, color: tokens.textDim)),
        ],
      ),
    );
  }
}

/// Join progress row (role='status'): spinner + message + 'Attempt {n}/{max}'.
/// Shared with the Join Room modal (feature agent).
class JoinProgressRow extends StatelessWidget {
  const JoinProgressRow({super.key, required this.progress});

  final JoinProgress progress;

  /// Localized narration for the package's structured [JoinProgress] facts.
  String _message(AppStrings s) {
    final delay = progress.retryDelay;
    if (progress.phase == JoinPhases.retrying && delay != null) {
      return s.onboardingJoinRetryWait(
          (delay.inMilliseconds / 1000).round());
    }
    return progress.attempt == 1
        ? s.onboardingJoinFinding
        : s.onboardingJoinRetryingAttempt(
            progress.attempt, progress.maxAttempts);
  }

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    return Semantics(
      liveRegion: true, // role="status"
      child: Container(
        padding: const EdgeInsets.symmetric(
            horizontal: JeliyaSpacing.x10, vertical: JeliyaSpacing.x8),
        decoration: BoxDecoration(
          color: tokens.accentDim,
          borderRadius: BorderRadius.circular(JeliyaRadii.btn),
        ),
        child: Row(
          children: [
            SizedBox(
              width: 12,
              height: 12,
              child: CircularProgressIndicator(
                  strokeWidth: 1.6, color: tokens.accent),
            ),
            const SizedBox(width: JeliyaSpacing.x8),
            Expanded(
              child: Text(_message(s),
                  style: TextStyle(fontSize: 12.5, color: tokens.text)),
            ),
            Text(
              s.onboardingJoinAttempt(
                  progress.attempt, progress.maxAttempts),
              style: TextStyle(
                  fontSize: 11.5,
                  fontStyle: FontStyle.italic,
                  color: tokens.textMute),
            ),
          ],
        ),
      ),
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
      child: Text(text,
          style: TextStyle(fontSize: 12.5, color: tokens.textDim)),
    );
  }
}
