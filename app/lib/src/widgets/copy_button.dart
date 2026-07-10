/// Copy-to-clipboard button (ui.tsx `CopyButton`): ghost small button whose
/// label swaps to 'Copied ✓' for 1400ms after copying — the ONLY transient
/// feedback pattern in the app (no toasts exist). The semantic label applies
/// only while idle so the visible confirmation is what gets announced.
library;

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../l10n/strings_context.dart';
import '../layout.dart';
import '../theme.dart';

class CopyButton extends StatefulWidget {
  const CopyButton({
    super.key,
    required this.text,
    this.label,
    this.semanticLabel,
  });

  /// What gets copied.
  final String text;

  /// Visible idle label (e.g. 'Copy', 'Copy ticket', '⧉'); defaults to the
  /// localized 'Copy' (resolved at build — defaults can't be locale-aware).
  final String? label;

  /// Accessible name while idle (e.g. 'Copy identity ID').
  final String? semanticLabel;

  @override
  State<CopyButton> createState() => _CopyButtonState();
}

class _CopyButtonState extends State<CopyButton> {
  bool _done = false;
  Timer? _timer;

  Future<void> _copy() async {
    await Clipboard.setData(ClipboardData(text: widget.text));
    if (!mounted) return;
    setState(() => _done = true);
    _timer?.cancel();
    _timer = Timer(const Duration(milliseconds: 1400), () {
      if (mounted) setState(() => _done = false);
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final tokens = JeliyaTokens.of(context);
    final button = TextButton(
      onPressed: () => _copy(),
      style: TextButton.styleFrom(
        foregroundColor: _done ? tokens.accent : tokens.textDim,
        padding: const EdgeInsets.symmetric(horizontal: 11, vertical: 5),
        // Web mobile parity: the copy affordance is a `.btn btn-sm`, so it
        // grows to the 44dp touch floor below the shell breakpoint.
        minimumSize:
            isMobileWidth(context) ? const Size(0, 44) : Size.zero,
        tapTargetSize: MaterialTapTargetSize.shrinkWrap,
        textStyle: const TextStyle(fontSize: 12.5),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(JeliyaRadii.btnSm),
        ),
      ),
      child: Text(_done
          ? context.strings.commonCopied
          : widget.label ?? context.strings.commonCopy),
    );
    final semanticLabel = widget.semanticLabel;
    if (semanticLabel == null || _done) return button;
    return Semantics(label: semanticLabel, child: button);
  }
}
