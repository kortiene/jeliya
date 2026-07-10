/// Onboarding — identity step (phase 'no-identity'), exact port of
/// Onboarding.tsx `IdentityStep` per phase3-features.json. `identity_exists`
/// is treated as success (another client created it) and advances silently.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:jeliya_protocol/jeliya_protocol.dart';

import '../l10n/strings_context.dart';
import '../session/daemon_session.dart';
import '../theme.dart';
import '../widgets/buttons.dart';
import '../widgets/error_note.dart';
import '../widgets/tree_mark.dart';

/// Brand block shared by both onboarding steps.
class OnboardingBrand extends StatelessWidget {
  const OnboardingBrand({super.key});

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    return Column(
      children: [
        const TreeMark(size: 44),
        const SizedBox(height: JeliyaSpacing.x10),
        const Wordmark(fontSize: 30, asHeading: true),
        const SizedBox(height: JeliyaSpacing.x8),
        Text(
          s.onboardingTagline,
          textAlign: TextAlign.center,
          style: TextStyle(fontSize: 13.5, color: tokens.textDim),
        ),
      ],
    );
  }
}

/// The onboarding card chrome (radius 16, bgRaise, hairline border).
class OnboardingCard extends StatelessWidget {
  const OnboardingCard({super.key, required this.child, this.width = 420});

  final Widget child;

  /// A MAXIMUM, not a fixed width: desktop still gets the reference 420px
  /// card; a phone viewport narrower than that gets what fits (onboarding
  /// renders before the shell, so it is the first thing a phone user hits).
  final double width;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    return Container(
      width: double.infinity,
      constraints: BoxConstraints(maxWidth: width),
      padding: const EdgeInsets.all(JeliyaSpacing.x18),
      decoration: BoxDecoration(
        color: tokens.bgRaise,
        borderRadius: BorderRadius.circular(JeliyaRadii.modal),
        border: Border.all(color: tokens.border),
      ),
      child: child,
    );
  }
}

class OnboardingIdentityScreen extends StatefulWidget {
  const OnboardingIdentityScreen({super.key});

  @override
  State<OnboardingIdentityScreen> createState() =>
      _OnboardingIdentityScreenState();
}

class _OnboardingIdentityScreenState extends State<OnboardingIdentityScreen> {
  bool _busy = false;
  RequestError? _error;

  Future<void> _create() async {
    final session = SessionScope.of(context);
    final client = session.client;
    if (client == null || _busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await client.identityCreate();
      session.advanceOnboarding();
    } catch (e) {
      final err = errorShape(e);
      if (err.code == ErrorCodes.identityExists) {
        // Someone else created it — just re-sync.
        session.advanceOnboarding();
      } else if (mounted) {
        setState(() => _error = err);
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    return Scaffold(
      body: Center(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(JeliyaSpacing.page),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const OnboardingBrand(),
              const SizedBox(height: JeliyaSpacing.x24),
              OnboardingCard(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(s.onboardingIdentityTitle,
                        style: JeliyaText.onboardingCardTitle),
                    const SizedBox(height: JeliyaSpacing.x10),
                    Text(s.onboardingIdentityCopy1,
                        style:
                            TextStyle(fontSize: 13, color: tokens.textDim)),
                    const SizedBox(height: JeliyaSpacing.x8),
                    Text(s.onboardingIdentityCopy2,
                        style:
                            TextStyle(fontSize: 13, color: tokens.textDim)),
                    const SizedBox(height: JeliyaSpacing.x16),
                    JeliyaButton(
                      label: _busy
                          ? s.onboardingCreatingIdentity
                          : s.onboardingCreateIdentity,
                      variant: JeliyaButtonVariant.primary,
                      size: JeliyaButtonSize.lg,
                      busy: _busy,
                      onPressed: _busy ? null : _create,
                    ),
                    ErrorNote(error: _error),
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
