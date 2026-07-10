/// Token-driven buttons (styles.css `.btn` family). One widget, four
/// variants — primary is a TINTED OUTLINE (accent text on accent-dim fill),
/// never a solid fill; every mutating form disables its submit while busy and
/// swaps the label to a gerund (the caller passes the busy label).
library;

import 'package:flutter/material.dart';

import '../theme.dart';

enum JeliyaButtonVariant { normal, primary, ghost, danger }

enum JeliyaButtonSize { sm, md, lg }

class JeliyaButton extends StatelessWidget {
  const JeliyaButton({
    super.key,
    required this.label,
    required this.onPressed,
    this.variant = JeliyaButtonVariant.normal,
    this.size = JeliyaButtonSize.md,
    this.busy = false,
    this.autofocus = false,
    this.semanticLabel,
  });

  final String label;

  /// Null disables the button (0.55 opacity per the tokens).
  final VoidCallback? onPressed;

  final JeliyaButtonVariant variant;
  final JeliyaButtonSize size;

  /// Shows a small spinner before the label (Sending…/Joining…/etc).
  final bool busy;

  /// Initial focus (the reference autofocuses e.g. the Leave-room danger
  /// submit so Enter confirms).
  final bool autofocus;

  final String? semanticLabel;

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);

    final (Color fg, Color bg, Color borderColor) = switch (variant) {
      JeliyaButtonVariant.primary => (tokens.accent, tokens.accentDim, tokens.accentLine),
      JeliyaButtonVariant.ghost => (tokens.textDim, Colors.transparent, Colors.transparent),
      JeliyaButtonVariant.danger => (tokens.red, tokens.bgCard, tokens.redLine),
      JeliyaButtonVariant.normal => (tokens.text, tokens.bgCard, tokens.borderStrong),
    };

    final (EdgeInsets padding, double fontSize, double radius) = switch (size) {
      JeliyaButtonSize.sm => (
          const EdgeInsets.symmetric(horizontal: 11, vertical: 5),
          12.5,
          JeliyaRadii.btnSm
        ),
      JeliyaButtonSize.md => (
          const EdgeInsets.symmetric(horizontal: 14, vertical: 8),
          14.0,
          JeliyaRadii.btn
        ),
      JeliyaButtonSize.lg => (
          const EdgeInsets.symmetric(horizontal: 22, vertical: 11),
          15.0,
          JeliyaRadii.btn
        ),
    };

    // Truncation on every user-text surface (web: `.btn { white-space:
    // nowrap }`): a width-squeezed button (phone-width Wraps, wide French
    // labels) ellipsizes its label instead of overflowing; with room to
    // spare it renders at intrinsic width exactly as before. Deliberately
    // NOT a Flexible — buttons also sit in unbounded-width Rows, where a
    // flex child is a layout error.
    final text = Text(label,
        maxLines: 1, overflow: TextOverflow.ellipsis, softWrap: false);
    final child = busy
        ? Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              SizedBox(
                width: fontSize - 2,
                height: fontSize - 2,
                child: CircularProgressIndicator(strokeWidth: 1.6, color: fg),
              ),
              const SizedBox(width: JeliyaSpacing.x6),
              text,
            ],
          )
        : text;

    final button = TextButton(
      onPressed: onPressed,
      autofocus: autofocus,
      style: TextButton.styleFrom(
        foregroundColor: fg,
        disabledForegroundColor: fg.withValues(alpha: 0.55),
        backgroundColor: bg,
        disabledBackgroundColor:
            bg == Colors.transparent ? bg : bg.withValues(alpha: 0.55),
        padding: padding,
        minimumSize: Size.zero,
        tapTargetSize: MaterialTapTargetSize.shrinkWrap,
        textStyle: TextStyle(fontSize: fontSize, fontWeight: FontWeight.w500),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(radius),
          side: BorderSide(color: borderColor),
        ),
      ),
      child: child,
    );

    final semanticLabel = this.semanticLabel;
    if (semanticLabel == null) return button;
    return Semantics(label: semanticLabel, child: button);
  }
}
