/// Copy-to-clipboard button (ui.tsx `CopyButton`): ghost small button whose
/// label swaps to 'Copied ✓' for 1400ms after copying — the ONLY transient
/// feedback pattern in the app (no toasts exist). The semantic label applies
/// only while idle so the visible confirmation is what gets announced.
///
/// A clipboard write can fail (some Linux desktops have no clipboard owner, a
/// headless session, a denied portal). The button then must NOT show the false
/// 'Copied ✓'; instead it swaps to an actionable "copy it manually" label and
/// records the miss in diagnostics via the synthetic seam (no raw text). The
/// button is a leaf with no session of its own, so it reads the ambient
/// [SessionScope] without subscribing — absent (in isolated tests) it simply
/// shows the label and skips the recording.
library;

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../l10n/strings_context.dart';
import '../layout.dart';
import '../session/daemon_session.dart';
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
  bool _failed = false;
  Timer? _timer;

  Future<void> _copy() async {
    try {
      await Clipboard.setData(ClipboardData(text: widget.text));
    } catch (_) {
      if (!mounted) return;
      // No session in isolated tests; record when one is above us. Read the
      // scope without registering a dependency (this is not build).
      context
          .getInheritedWidgetOfExactType<SessionScope>()
          ?.notifier
          ?.recordLocalFailure('clipboard.copy', 'clipboard_write_failed');
      // Never the false 'Copied ✓'; show the manual-copy guidance instead.
      setState(() {
        _done = false;
        _failed = true;
      });
      _timer?.cancel();
      _timer = Timer(const Duration(milliseconds: 3200), () {
        if (mounted) setState(() => _failed = false);
      });
      return;
    }
    if (!mounted) return;
    setState(() {
      _done = true;
      _failed = false;
    });
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
    final s = context.strings;
    final button = TextButton(
      onPressed: () => _copy(),
      style: TextButton.styleFrom(
        foregroundColor: _failed
            ? tokens.red
            : _done
                ? tokens.accent
                : tokens.textDim,
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
      // The failure guidance is a full sentence; let it wrap rather than force
      // a tight row to overflow.
      child: Text(
        _failed
            ? s.commonCopyFailed
            : _done
                ? s.commonCopied
                : widget.label ?? s.commonCopy,
        softWrap: _failed,
      ),
    );
    // A clipboard failure is an alert the reader should hear announced.
    if (_failed) return Semantics(liveRegion: true, child: button);
    final semanticLabel = widget.semanticLabel;
    if (semanticLabel == null || _done) return button;
    return Semantics(label: semanticLabel, child: button);
  }
}
