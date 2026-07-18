/// Design-system conformance gate, Flutter half (issue #75).
///
/// `assets/design-tokens.json` is the shared fixture BOTH clients answer to.
/// This asserts the Flutter palette in `lib/src/theme.dart` carries the values
/// the fixture pins; `scripts/check-design-tokens.mjs` asserts the same of
/// `ui/src/styles.css`. The two halves read one file, so the clients cannot
/// drift apart again silently.
///
/// They drifted before precisely because the drift was NAMED: the own-message
/// bubble had an accent gradient in CSS and a matching pair of gradient TOKENS
/// in Dart, so each side looked internally consistent and a reviewer saw a
/// token rather than a Named Rule being broken.
///
/// Where this test and the fixture disagree, one of them is a bug — say WHICH
/// in the pull request. Do not "fix" a failure by editing the expectation.
library;

import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/theme.dart';

void main() {
  // The test CWD is `app/`; the fixture is shared, so it lives at the repo root.
  final fixtureFile = File('../assets/design-tokens.json');
  final fixture =
      jsonDecode(fixtureFile.readAsStringSync()) as Map<String, dynamic>;
  final colors = (fixture['color'] as Map<String, dynamic>)
    ..removeWhere((key, _) => key.startsWith(r'$'));
  final radii = (fixture['radius'] as Map<String, dynamic>)
    ..removeWhere((key, _) => key.startsWith(r'$'));
  final contrast = fixture['contrast'] as Map<String, dynamic>;

  const tokens = JeliyaTokens.dark;

  /// Fixture colour name -> the [JeliyaTokens] getter that carries it.
  /// Mirrors the `CSS_VAR` map in `scripts/check-design-tokens.mjs` one-for-one
  /// so the two halves cover the same set.
  final dartToken = <String, Color>{
    'ground': tokens.bg, // --bg
    'chrome': tokens.bgRaise, // --bg-raise
    'card': tokens.bgCard, // --bg-card
    'card-nested': tokens.bgCard2, // --bg-card-2
    'input-well': tokens.bgInput, // --bg-input
    'bubble-remote': tokens.bubbleRemoteBg, // --bg-bubble-remote
    'border-quiet': tokens.border, // --border
    'border-strong': tokens.borderStrong, // --border-strong
    'border-interactive': tokens.borderInteractive, // --border-interactive
    'accent': tokens.accent, // --accent
    'accent-deep': tokens.accent2, // --accent-2
    'ink': tokens.text, // --text
    'ink-dim': tokens.textDim, // --text-dim
    'ink-mute': tokens.textMute, // --text-mute
    'amber': tokens.amber, // --amber
    'red': tokens.red, // --red
    'blue': tokens.blue, // --blue
    // The mjs half skips `scrim` and `shadow` as "composed"; Flutter carries
    // scrim as a real token, so it is checked here.
    'scrim': tokens.modalBarrier, // rgba(3, 7, 9, 0.72)
  };

  /// `shadow` has no Flutter consumer: DESIGN.md is flat by doctrine, and the
  /// one Flutter surface that lifts (the modal barrier) uses `scrim`. Listed
  /// explicitly so a new fixture colour cannot slip through unmapped.
  const unmapped = {'shadow'};

  /// Fixture radius name -> the [JeliyaRadii] const that carries it.
  const dartRadius = <String, double>{
    'tail': JeliyaRadii.bubbleSharp, // 4  — bubble sharp corner
    'tight': JeliyaRadii.iconBtn, // 7  — icon-btn / skeleton / pipe chip
    'sm': JeliyaRadii.btnSm, // 8  — btn-sm
    'control': JeliyaRadii.btn, // 9  — btn / inputs / 34px tiles
    'nav': JeliyaRadii.nav, // 10 — nav items / stat inner cells
    'tile': JeliyaRadii.row, // 11 — room rows / file+pipe tiles
    'card-sm': JeliyaRadii.card, // 12 — profile + settings cards
    'card': JeliyaRadii.composer, // 13 — composer / agent+pipe cards
    'stat': JeliyaRadii.bubble, // 14 — member rows / bubble round corners
    'card-lg': JeliyaRadii.hero, // 15 — heroes / fleet cards
    'surface': JeliyaRadii.modal, // 16 — modal / onboarding card
    'pill': JeliyaRadii.pill, // 999 — all pills/chips/badges
  };

  group('shared fixture', () {
    test('every pinned colour is mapped to a Dart token', () {
      final unaccounted = colors.keys
          .where((k) => !dartToken.containsKey(k) && !unmapped.contains(k))
          .toList();
      expect(
        unaccounted,
        isEmpty,
        reason:
            'assets/design-tokens.json pins $unaccounted, which no JeliyaTokens '
            'getter carries. Add the token to theme.dart and map it here, or '
            'record it in `unmapped` with the reason Flutter has no consumer.',
      );
    });

    test('every pinned colour matches its JeliyaTokens getter', () {
      for (final entry in dartToken.entries) {
        final pinned = _parseHex(colors[entry.key] as String);
        expect(
          entry.value.toARGB32(),
          pinned,
          reason: 'colour `${entry.key}`: theme.dart has '
              '${_hex(entry.value.toARGB32())}, the fixture pins '
              '${_hex(pinned)}. One of them is a bug — say which in the PR. '
              'The React half is scripts/check-design-tokens.mjs.',
        );
      }
    });

    test('every pinned radius matches its JeliyaRadii const', () {
      final unaccounted =
          radii.keys.where((k) => !dartRadius.containsKey(k)).toList();
      expect(unaccounted, isEmpty,
          reason: 'fixture pins radii $unaccounted with no JeliyaRadii mapping');

      for (final entry in dartRadius.entries) {
        expect(
          entry.value,
          (radii[entry.key] as num).toDouble(),
          reason: 'radius `${entry.key}`: JeliyaRadii has ${entry.value}, the '
              'fixture pins ${radii[entry.key]}',
        );
      }
    });
  });

  // -- contrast floors ---------------------------------------------------------
  //
  // The fixture RECORDS these floors so a test can assert them, rather than a
  // source comment claiming them. `text` is WCAG 1.4.3 AA for
  // information-bearing text; `non-text` is 1.4.11 for the boundaries that
  // identify a control.

  group('contrast floors', () {
    /// Every surface a token can be drawn on. Ink and control boundaries must
    /// clear their floor against ALL of them — the worst case is what a reader
    /// actually hits, and it is not always the darkest ground.
    final surfaces = <String, Color>{
      'ground': tokens.bg,
      'chrome': tokens.bgRaise,
      'card': tokens.bgCard,
      'card-nested': tokens.bgCard2,
      'input-well': tokens.bgInput,
      'bubble-remote': tokens.bubbleRemoteBg,
    };

    test('textMute clears the text floor on every surface', () {
      // i18n-exempt: a JSON key in assets/design-tokens.json, not copy.
      final floor = (contrast['text'] as num).toDouble();
      for (final surface in surfaces.entries) {
        final ratio = _contrastRatio(tokens.textMute, surface.value);
        expect(
          ratio,
          greaterThanOrEqualTo(floor),
          reason: 'textMute on ${surface.key} is '
              '${ratio.toStringAsFixed(2)}:1, below the $floor:1 floor. '
              'textMute colours information-bearing small text (timestamps, '
              'syslines, placeholders), so it must clear NORMAL-text contrast. '
              'Recede through the GROUND, never by fading the ink.',
        );
      }
    });

    test('borderInteractive clears the non-text floor on every surface', () {
      final floor = (contrast['non-text'] as num).toDouble();
      for (final surface in surfaces.entries) {
        final ratio = _contrastRatio(tokens.borderInteractive, surface.value);
        expect(
          ratio,
          greaterThanOrEqualTo(floor),
          reason: 'borderInteractive on ${surface.key} is '
              '${ratio.toStringAsFixed(2)}:1, below the $floor:1 floor. '
              'This is the boundary that IDENTIFIES a control (WCAG 1.4.11).',
        );
      }
    });

    test('the formula agrees with the WCAG reference pairs', () {
      // Anchors, so a broken luminance implementation cannot silently pass the
      // assertions above: black-on-white is exactly 21:1, and any colour
      // against itself is exactly 1:1.
      expect(
        _contrastRatio(const Color(0xFF000000), const Color(0xFFFFFFFF)),
        closeTo(21.0, 0.001),
      );
      expect(_contrastRatio(tokens.bg, tokens.bg), closeTo(1.0, 0.001));
    });
  });
}

// -- helpers -------------------------------------------------------------------

/// `#rrggbb` or `#rrggbbaa` -> a 32-bit ARGB int, the encoding
/// [Color.toARGB32] returns. Opaque when the fixture omits alpha.
int _parseHex(String hex) {
  final digits = hex.replaceFirst('#', '');
  if (digits.length == 6) return 0xFF000000 | int.parse(digits, radix: 16);
  if (digits.length == 8) {
    final rgba = int.parse(digits, radix: 16);
    // CSS orders the byte as #RRGGBBAA; Dart's Color wants AARRGGBB.
    return ((rgba & 0xFF) << 24) | (rgba >> 8);
  }
  throw FormatException('unsupported colour literal in the fixture', hex);
}

String _hex(int argb) => '#${argb.toRadixString(16).padLeft(8, '0')}';

/// WCAG 2.x relative luminance (definition in the Understanding docs for
/// SC 1.4.3). Implemented here rather than pulled in as a dependency: the whole
/// point of the gate is that it has no way to disagree with the spec.
double _relativeLuminance(Color color) {
  final argb = color.toARGB32();
  double channel(int value) {
    final s = value / 255.0;
    return s <= 0.04045
        ? s / 12.92
        : math.pow((s + 0.055) / 1.055, 2.4).toDouble();
  }

  return 0.2126 * channel((argb >> 16) & 0xFF) +
      0.7152 * channel((argb >> 8) & 0xFF) +
      0.0722 * channel(argb & 0xFF);
}

/// WCAG contrast ratio, `(lighter + 0.05) / (darker + 0.05)`, in 1:1..21:1.
///
/// Both arguments must be OPAQUE. A translucent ink composites against
/// whatever is behind it, so its real ratio depends on the stack — which is
/// exactly the bug the own-bubble gradient hid, where a translucent fill
/// composited over the ground instead of the card and dropped muted ink to
/// 4.06:1.
double _contrastRatio(Color a, Color b) {
  assert(a.a == 1.0 && b.a == 1.0, 'contrast is only meaningful on opaque colours');
  final la = _relativeLuminance(a);
  final lb = _relativeLuminance(b);
  final lighter = math.max(la, lb);
  final darker = math.min(la, lb);
  return (lighter + 0.05) / (darker + 0.05);
}
