/// Renders an invite string as a scannable QR code (issue #103), painted with a
/// pure-Dart [CustomPainter] over the hand-vendored encoder in qr.dart — no
/// native plugin, no package.
///
/// A QR is always dark-on-light with a 4-module quiet zone: scanners rely on
/// that contrast and margin, so — unlike the rest of the UI — it does NOT invert
/// in dark mode. The white plate is part of the drawing. When the payload is too
/// large for any symbol the widget collapses to nothing, leaving the caller's
/// Copy/Share affordances as the fallback. The raw invite stays reachable via
/// the adjacent copy button, so this is an affordance, not the only path (a
/// [Semantics] label names it; the modules themselves are decorative).
library;

import 'package:flutter/widgets.dart';

import 'qr.dart';

/// Dark and light module colours. Fixed, theme-independent: a QR must stay
/// dark-on-light to scan.
const Color _qrDark = Color(0xFF000000);
const Color _qrLight = Color(0xFFFFFFFF);

class QrView extends StatefulWidget {
  const QrView({
    super.key,
    required this.value,
    required this.semanticLabel,
    this.caption,
    this.captionStyle,
    this.size = 208,
  });

  /// The invite string to encode (the combined `ticket#address` where present).
  final String value;

  /// Accessible name for the code (the raw string is copyable separately).
  final String semanticLabel;

  /// Optional visible caption, drawn under the code inside the same widget so it
  /// never appears without the image (both are dropped when encoding fails).
  final String? caption;
  final TextStyle? captionStyle;

  /// The painted edge length in logical pixels (a square).
  final double size;

  @override
  State<QrView> createState() => _QrViewState();
}

class _QrViewState extends State<QrView> {
  /// The encoded matrix, cached against [QrView.value]. Encoding is not cheap
  /// (eight mask evaluations, each with a full-grid penalty scan), and the
  /// invite modal rebuilds once a SECOND while a time-boxed ticket counts down —
  /// so it must not run per build.
  QrMatrix? _matrix;

  @override
  void initState() {
    super.initState();
    _matrix = encodeQr(widget.value);
  }

  @override
  void didUpdateWidget(covariant QrView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.value != widget.value) _matrix = encodeQr(widget.value);
  }

  @override
  Widget build(BuildContext context) {
    final matrix = _matrix;
    if (matrix == null) return const SizedBox.shrink(); // graceful fallback
    final code = Semantics(
      label: widget.semanticLabel,
      image: true,
      child: Container(
        padding: const EdgeInsets.all(12),
        decoration: BoxDecoration(
          color: _qrLight,
          borderRadius: BorderRadius.circular(12),
        ),
        child: CustomPaint(
          size: Size.square(widget.size),
          painter: _QrPainter(matrix),
        ),
      ),
    );
    final caption = widget.caption;
    if (caption == null) return Center(child: code);
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        code,
        const SizedBox(height: 8),
        Text(caption, textAlign: TextAlign.center, style: widget.captionStyle),
      ],
    );
  }
}

class _QrPainter extends CustomPainter {
  const _QrPainter(this.matrix);

  final QrMatrix matrix;

  static const int _quiet = 4; // quiet-zone modules on every side

  @override
  void paint(Canvas canvas, Size size) {
    final dim = matrix.size + _quiet * 2;
    final scale = size.width / dim;

    // White plate (includes the quiet zone) — part of the code, not the theme.
    canvas.drawRect(
      Rect.fromLTWH(0, 0, size.width, size.height),
      Paint()..color = _qrLight,
    );

    // Coalesce horizontal runs of dark modules into one rect each — fewer draw
    // ops than a rect per module, and no seams between adjacent modules.
    final dark = Paint()
      ..color = _qrDark
      ..style = PaintingStyle.fill;
    final n = matrix.size;
    for (var r = 0; r < n; r++) {
      final row = matrix.modules[r];
      var c = 0;
      while (c < n) {
        if (!row[c]) {
          c++;
          continue;
        }
        final start = c;
        while (c < n && row[c]) {
          c++;
        }
        final x = (start + _quiet) * scale;
        final y = (r + _quiet) * scale;
        final w = (c - start) * scale;
        // +0.5px overpaint on width/height guards against hairline seams from
        // sub-pixel rounding between adjacent runs/rows on fractional scales.
        canvas.drawRect(Rect.fromLTWH(x, y, w + 0.5, scale + 0.5), dark);
      }
    }
  }

  @override
  bool shouldRepaint(_QrPainter oldDelegate) => oldDelegate.matrix != matrix;
}
