/// Boot screen (phase 'boot') — phase3-features.json "Boot screen". Shown
/// from app start until daemon.status resolves after the first successful
/// connect. Also renders the desktop-only bring-up failure state (the walking
/// skeleton's Boot.failed → Retry path, kept per the keep-list).
///
/// All copy is composed HERE from the session's structured [BootStage] facts
/// (the session holds no user-facing strings), so a live locale switch
/// re-renders correctly.
///
/// The failure surface is built to stay recoverable in the worst layout: it
/// lives inside a [SafeArea] + a scroll view (so Retry is reachable at 360×640
/// and 200% text, in every locale), leads with a friendly summary + Retry, and
/// tucks the raw technical text behind a collapsed disclosure (P1: simple by
/// default, truthful in the details — the same shape as ErrorNote). Initial
/// focus lands on the error heading, then moves predictably to Retry.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:jeliya_protocol/jeliya_protocol.dart' show ConnectionState;

import '../l10n/strings_context.dart';
import '../session/daemon_session.dart';
import '../theme.dart';
import '../widgets/buttons.dart';
import '../widgets/tree_mark.dart';

class BootScreen extends StatefulWidget {
  const BootScreen({super.key});

  @override
  State<BootScreen> createState() => _BootScreenState();
}

class _BootScreenState extends State<BootScreen> {
  /// Initial focus lands on the failure heading so a screen-reader user starts
  /// at the problem statement, not mid-way through the technical detail.
  final FocusNode _headingFocus = FocusNode(debugLabel: 'bootErrorHeading');

  bool _detailsOpen = false;

  /// One focus grab per failure episode: set when the failed surface first
  /// renders, cleared whenever we leave the failed state so a later failure
  /// (e.g. a Retry that fails again) re-focuses the heading.
  bool _focusedThisFailure = false;

  @override
  void dispose() {
    _headingFocus.dispose();
    super.dispose();
  }

  String _statusLine(AppStrings s, ConnectionState conn) => switch (conn) {
        ConnectionState.connected => s.bootSyncing,
        ConnectionState.disconnected => s.bootNotConnected,
        _ => s.bootContactingDaemon,
      };

  /// Localized narration of the session's structured boot facts.
  String _stageLine(AppStrings s, DaemonSession session) =>
      switch (session.bootStage) {
        BootStage.spawning => s.bootStartingDaemon,
        BootStage.evicting => s.bootEvictingIncumbent,
        BootStage.adopted =>
          s.bootAdoptedDaemon(session.bootPid ?? 0, session.bootPort ?? 0),
        BootStage.daemonUp =>
          s.bootDaemonUp(session.bootPid ?? 0, session.bootPort ?? 0),
        BootStage.failedBinaryMissing => s.bootBinaryNotFound,
        BootStage.failedMismatch => s.bootProtocolMismatch(
            session.bootMismatchActual ?? 0, session.bootMismatchExpected ?? 0),
        BootStage.failedStart => s.bootDaemonStartFailed,
        BootStage.failedTimeout => s.bootDaemonConnectTimeout,
        BootStage.failedGeneric => s.bootFailedGeneric,
        BootStage.none => '',
      };

  @override
  Widget build(BuildContext context) {
    final session = SessionScope.of(context);
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    final failed = session.boot == Boot.failed;

    if (!failed) {
      // Leaving (or never entering) the failed state re-arms the focus grab for
      // the next failure episode.
      _focusedThisFailure = false;
      return Scaffold(body: Center(child: _loading(context, session, tokens, s)));
    }

    if (!_focusedThisFailure) {
      _focusedThisFailure = true;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _headingFocus.requestFocus();
      });
    }
    return Scaffold(body: _failed(context, session, tokens, s));
  }

  /// The loading branch — unchanged behavior: branding, a status line, the
  /// transport target, the current stage, and (while reconnecting) the hint.
  Widget _loading(BuildContext context, DaemonSession session,
      JeliyaTokens tokens, AppStrings s) {
    final stageLine = _stageLine(s, session);
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        const TreeMark(size: 48),
        const SizedBox(height: JeliyaSpacing.x12),
        const Wordmark(fontSize: 26, asHeading: true),
        const SizedBox(height: JeliyaSpacing.x10),
        Text(_statusLine(s, session.conn),
            style: TextStyle(fontSize: 13, color: tokens.textDim)),
        const SizedBox(height: JeliyaSpacing.x6),
        // Transport target (the WS URL from client.describe()).
        Text(
          session.transportDescription,
          style: JeliyaText.mono(fontSize: 12, color: tokens.textMute),
        ),
        if (stageLine.isNotEmpty) ...[
          const SizedBox(height: JeliyaSpacing.x6),
          Text(stageLine, style: TextStyle(fontSize: 12, color: tokens.textMute)),
        ],
        if (session.conn == ConnectionState.reconnecting) ...[
          const SizedBox(height: JeliyaSpacing.x8),
          Text(
            s.bootRetryingHint,
            style: TextStyle(fontSize: 12.5, color: tokens.textDim),
          ),
        ],
      ],
    );
  }

  /// The resilient failure surface: SafeArea + scroll so Retry is always
  /// reachable, heading → Retry → collapsed technical details.
  Widget _failed(BuildContext context, DaemonSession session,
      JeliyaTokens tokens, AppStrings s) {
    final stageLine = _stageLine(s, session);
    final technical = session.bootTechnical;
    return SafeArea(
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(JeliyaSpacing.x16),
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 480),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.center,
              children: [
                const TreeMark(size: 48),
                const SizedBox(height: JeliyaSpacing.x12),
                const Wordmark(fontSize: 26, asHeading: true),
                const SizedBox(height: JeliyaSpacing.x10),
                // Friendly summary FIRST, and the focus target on boot.
                Focus(
                  focusNode: _headingFocus,
                  child: Semantics(
                    header: true,
                    child: Text(
                      s.bootCouldNotStart,
                      textAlign: TextAlign.center,
                      style: TextStyle(
                          fontSize: 15,
                          fontWeight: FontWeight.w700,
                          color: tokens.red),
                    ),
                  ),
                ),
                if (stageLine.isNotEmpty) ...[
                  const SizedBox(height: JeliyaSpacing.x8),
                  Text(
                    stageLine,
                    textAlign: TextAlign.center,
                    style: TextStyle(fontSize: 13, color: tokens.textDim),
                  ),
                ],
                const SizedBox(height: JeliyaSpacing.x14),
                // Retry BEFORE the technical detail — the primary recovery path
                // is never below the fold on a small screen.
                JeliyaButton(
                  label: s.commonRetry,
                  variant: JeliyaButtonVariant.primary,
                  onPressed: () => session.start(),
                ),
                if (technical.isNotEmpty) ...[
                  const SizedBox(height: JeliyaSpacing.x12),
                  _TechnicalDisclosure(
                    open: _detailsOpen,
                    onToggle: () =>
                        setState(() => _detailsOpen = !_detailsOpen),
                    // Raw exception text — deliberately English, like
                    // ErrorNote's technical details.
                    technical: technical,
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Collapsed "Technical details" disclosure (mirrors ErrorNote / FetchDetail:
/// an InkWell arrow toggle over the raw mono text).
class _TechnicalDisclosure extends StatelessWidget {
  const _TechnicalDisclosure({
    required this.open,
    required this.onToggle,
    required this.technical,
  });

  final bool open;
  final VoidCallback onToggle;
  final String technical;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final s = context.strings;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.center,
      children: [
        InkWell(
          onTap: onToggle,
          child: Text(
            '${open ? '▾' : '▸'} ${s.commonTechnicalDetails}',
            style: TextStyle(fontSize: 12, color: tokens.textMute),
          ),
        ),
        if (open)
          Padding(
            padding: const EdgeInsets.only(top: JeliyaSpacing.x6),
            child: Text(
              technical,
              textAlign: TextAlign.center,
              style: JeliyaText.mono(fontSize: 11.5, color: tokens.textMute),
            ),
          ),
      ],
    );
  }
}
