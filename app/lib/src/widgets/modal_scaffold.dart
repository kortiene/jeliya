/// Modal primitives (ui.tsx `Modal`): [showJeliyaModal] opens a dialog over
/// the rgba(3,7,9,0.72) backdrop; [ModalScaffold] renders the card (radius 16,
/// bgRaise, borderStrong, header with title + ✕). Flutter's dialog route
/// supplies the reference keyboard/focus contract for free: Escape closes,
/// focus is trapped in the route, and focus returns to the opener on close.
library;

import 'package:flutter/material.dart';

import '../l10n/strings_context.dart';
import '../l10n/tokens.dart';
import '../theme.dart';

/// Open a modal. The [builder] should return a [ModalScaffold] (a stub modal
/// or a full one). Returns the value passed to `Navigator.pop`.
Future<T?> showJeliyaModal<T>(
  BuildContext context, {
  required WidgetBuilder builder,
}) {
  final tokens = JeliyaTokens.of(context);
  return showDialog<T>(
    context: context,
    barrierColor: tokens.modalBarrier,
    builder: builder,
  );
}

/// Open the SAME modal content as a full-screen route — the phone
/// presentation for long forms (join ticket, invite, add agent) that don't
/// fit a dialog under a soft keyboard. The [ModalScaffold] inside renders as
/// a page instead of a [Dialog]; the awaited `Navigator.pop` result contract
/// is identical to [showJeliyaModal], and the system back gesture dismisses
/// like Escape does for the dialog route.
Future<T?> showJeliyaModalScreen<T>(
  BuildContext context, {
  required WidgetBuilder builder,
}) {
  return Navigator.of(context, rootNavigator: true).push<T>(
    MaterialPageRoute<T>(
      fullscreenDialog: true,
      builder: (context) =>
          _ModalScreenScope(child: Builder(builder: builder)),
    ),
  );
}

/// Marks a subtree as full-screen-presented so [ModalScaffold] can pick the
/// page rendering without any modal changing its own API.
class _ModalScreenScope extends InheritedWidget {
  const _ModalScreenScope({required super.child});

  static bool of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<_ModalScreenScope>() != null;

  @override
  bool updateShouldNotify(_ModalScreenScope oldWidget) => false;
}

class ModalScaffold extends StatelessWidget {
  const ModalScaffold({
    super.key,
    required this.title,
    required this.child,
    this.wide = false,
    this.onClose,
  });

  final String title;
  final Widget child;

  /// Wide variant: max-width 560 instead of 440.
  final bool wide;

  /// Defaults to popping the enclosing dialog route.
  final VoidCallback? onClose;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final close = onClose ?? () => Navigator.of(context).maybePop();
    final header = Padding(
      padding: const EdgeInsets.fromLTRB(18, 16, 12, 12),
      child: Row(
        children: [
          Expanded(
            child: Semantics(
              header: true,
              child: Text(title, style: JeliyaText.modalTitle),
            ),
          ),
          IconButton(
            onPressed: close,
            tooltip: context.strings.commonClose,
            icon: Text(Tokens.closeGlyph,
                style: TextStyle(fontSize: 14, color: tokens.textDim)),
            constraints: const BoxConstraints(minWidth: 26, minHeight: 26),
            padding: EdgeInsets.zero,
          ),
        ],
      ),
    );
    if (_ModalScreenScope.of(context)) {
      // Full-screen presentation (showJeliyaModalScreen): same header/body
      // anatomy as the dialog, page-sized, with safe-area insets. The
      // Scaffold keeps the form above the soft keyboard.
      return Scaffold(
        backgroundColor: tokens.bgRaise,
        body: SafeArea(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              header,
              Divider(height: 1, color: tokens.border),
              Expanded(
                child: SingleChildScrollView(
                  padding: const EdgeInsets.fromLTRB(18, 16, 18, 20),
                  child: child,
                ),
              ),
            ],
          ),
        ),
      );
    }
    return Dialog(
      backgroundColor: tokens.bgRaise,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(JeliyaRadii.modal),
        side: BorderSide(color: tokens.borderStrong),
      ),
      child: ConstrainedBox(
        constraints: BoxConstraints(maxWidth: wide ? 560 : 440),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            header,
            Divider(height: 1, color: tokens.border),
            Flexible(
              child: SingleChildScrollView(
                padding: const EdgeInsets.fromLTRB(18, 16, 18, 20),
                child: child,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
