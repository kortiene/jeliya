/// Jeliya visual tokens, ported 1:1 from the reference web client
/// (`ui/src/styles.css`) via the Phase 3 design contract
/// (`phase3-design.json`). Dark-only this phase — the web client is dark-only,
/// and the teal-black + emerald identity is a settled product decision.
///
/// Everything color-shaped lives on [JeliyaTokens] (a [ThemeExtension], read
/// with `JeliyaTokens.of(context)`); spacing/radii/typography scales are
/// plain consts ([JeliyaSpacing], [JeliyaRadii], [JeliyaText]).
library;

import 'package:flutter/material.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart' show LabelTone;

// -- palette constants (styles.css custom properties) --------------------------

const Color _bg = Color(0xFF070D10); // --bg
const Color _bgRaise = Color(0xFF0A1116); // --bg-raise
const Color _bgCard = Color(0xFF0E161B); // --bg-card
const Color _bgCard2 = Color(0xFF111B21); // --bg-card-2
const Color _bgInput = Color(0xFF0C1419); // --bg-input
const Color _bubbleRemoteBg = Color(0xFF0C1519); // remote bubble (one-off)
const Color _border = Color(0xFF16232A); // --border
const Color _borderStrong = Color(0xFF21343C); // --border-strong
// The 3:1 non-text-contrast boundary (issues #73, #75). See
// `borderInteractive`; pinned in assets/design-tokens.json.
const Color _borderInteractive = Color(0xFF41707E);
const Color _accent = Color(0xFF2FD6A4); // --accent (emerald)
const Color _accent2 = Color(0xFF1FB4A8); // --accent-2 (teal, gradients only)
const Color _text = Color(0xFFDCEBE6); // --text (primary ink)
const Color _textDim = Color(0xFF8AA39D); // --text-dim
const Color _textMute = Color(0xFF7A938C); // --text-mute (AA-tuned)
const Color _amber = Color(0xFFF5B453); // --amber
const Color _red = Color(0xFFF26D6D); // --red
const Color _blue = Color(0xFF6AA8F7); // --blue
const Color _violet = Color(0xFFA78BFA); // avatar/file tint (no CSS token)

Color _alpha(Color base, double opacity) =>
    base.withValues(alpha: opacity);

/// The app-wide design tokens. One canonical instance ([JeliyaTokens.dark])
/// — the app is dark-only this phase, so [lerp]/[copyWith] are intentionally
/// trivial.
@immutable
class JeliyaTokens extends ThemeExtension<JeliyaTokens> {
  const JeliyaTokens._();

  /// The single dark theme instance.
  static const JeliyaTokens dark = JeliyaTokens._();

  static JeliyaTokens of(BuildContext context) =>
      Theme.of(context).extension<JeliyaTokens>() ?? dark;

  // -- surfaces ----------------------------------------------------------------

  /// App ground / body, identity footer, fleet+settings page ground.
  Color get bg => _bg;

  /// Raised panes (sidebar, right panel, room header, composer strip, modal,
  /// onboarding card, day-divider pill) and the "receded" ground for
  /// departed/offline/closed/removed rows.
  Color get bgRaise => _bgRaise;

  /// Default card + button surface.
  Color get bgCard => _bgCard;

  /// Deeper inset surface (pressed buttons, count badges, tiles, progress
  /// track, skeleton bars, new-messages pill, pipe address chip).
  Color get bgCard2 => _bgCard2;

  /// Text inputs, composer bar, ticket/addr code boxes.
  Color get bgInput => _bgInput;

  /// Remote message bubble background (between [bgInput] and [bgCard]).
  Color get bubbleRemoteBg => _bubbleRemoteBg;

  // -- borders -----------------------------------------------------------------

  /// Passive hairlines (pane dividers, card borders, tile borders).
  ///
  /// Decorative only. A divider carries no information a user must perceive to
  /// operate the app, so WCAG 1.4.11 does not apply and this stays at the
  /// designed 1.1-1.2:1 whisper.
  Color get border => _border;

  /// Interactive borders (buttons, inputs, chips, modal, dashed affordances).
  Color get borderStrong => _borderStrong;

  /// The boundary that IDENTIFIES a control — the only visual edge a default
  /// button, a text input or a chip has against its surface.
  ///
  /// [borderStrong] measures 1.35:1 to 1.51:1 against the five app surfaces,
  /// so the affordance that says "this is a control" was invisible to anyone
  /// who needs contrast. WCAG 1.4.11 puts the floor at 3:1 for exactly this
  /// case. #41707E is the same teal family one step brighter, and measures
  /// 3.20:1 on [bgCard2] (the lightest surface, so the worst case), 3.34:1 on
  /// [bgCard] and 3.58:1 on [bg].
  ///
  /// Kept SEPARATE from [borderStrong] rather than replacing it, because
  /// several controls encode selected/active state in their border colour;
  /// widening the token would have made those states unreadable.
  ///
  /// Mirrors `--border-interactive` in `ui/src/styles.css`; both answer to
  /// `assets/design-tokens.json` (issue #75).
  Color get borderInteractive => _borderInteractive;

  /// The keyboard focus ring — DESIGN.md's "global 2px emerald ring, offset 2",
  /// the same contract `ui/src/styles.css` implements for the web client.
  ///
  /// Solid accent measures 9.38:1 to 10.50:1 against every app surface, so it
  /// clears the 3:1 non-text floor with room to spare. It is drawn ADDITIVELY
  /// (outside the control's own box) so it composes with, rather than
  /// overwrites, a border already carrying state.
  Color get focusRing => _accent;


  // -- accent (emerald — earned, never a fallback) -------------------------------

  Color get accent => _accent;

  /// Gradient partner only — the progress fill start, and nothing else. The
  /// progress fill is the ONLY sanctioned accent gradient (DESIGN.md, Identity
  /// Marks); this token previously also fed an own-bubble gradient, which is
  /// why the drift was invisible in review.
  Color get accent2 => _accent2;

  /// Accent tint fills (primary button bg, selected nav/room, self-chip).
  Color get accentDim => _alpha(_accent, 0.12);

  /// Accent borders (primary button, hover borders, active states).
  Color get accentLine => _alpha(_accent, 0.4);

  // -- ink tiers ------------------------------------------------------------------

  /// Primary ink.
  Color get text => _text;

  /// Secondary ink (muted copy, meta, ghost buttons, panel tab labels).
  Color get textDim => _textDim;

  /// Tertiary ink — WCAG AA >= 4.5:1 on every surface; colors
  /// information-bearing small text (timestamps, syslines, placeholders,
  /// AGENT chip, uppercase section labels).
  Color get textMute => _textMute;

  // -- status hues ------------------------------------------------------------------

  /// Warning hue — reconnecting, relay path, pending invites, "no provider".
  Color get amber => _amber;
  Color get amberLine => _alpha(_amber, 0.4);
  Color get amberDim => _alpha(_amber, 0.12);

  /// Error/danger — disconnected, failed fetch, danger buttons, pdf tint.
  Color get red => _red;
  Color get redLine => _alpha(_red, 0.4);
  Color get redDim => _alpha(_red, 0.1);

  /// Informational/agent hue — connecting peers, agent-work card, agent role
  /// chip, invited status, doc-file tint.
  Color get blue => _blue;
  Color get blueLine => _alpha(_blue, 0.4);
  Color get blueDim => _alpha(_blue, 0.1);

  // -- message bubbles ------------------------------------------------------------------

  // Four bubble tokens were removed here in issue #75, because naming drift as
  // a first-class token is what let both clients stay confidently wrong in the
  // same direction — the reviewer sees a token, not a rule being broken:
  //
  //   bubbleOwnGradientStart (accent @ 0.20) + bubbleOwnGradientEnd
  //     (accent2 @ 0.13) — an own-message accent gradient. The progress fill
  //     is the only sanctioned accent gradient (DESIGN.md, Identity Marks).
  //   bubbleRemoteEdge (blue @ 0.34) — the 2px LEFT accent edge, which is
  //     exactly the side-stripe DESIGN.md forbids.
  //   bubbleOwnBorder (accent @ 0.42) — a fifth alpha where the system
  //     specifies two (assets/design-tokens.json `alpha`); the own bubble now
  //     takes the sanctioned [accentLine] (0.4) like every other accent edge.
  //
  // Ownership still reads four ways, which is plenty: right alignment, the
  // suppressed avatar, the flipped tail radius, and the emerald border.
  // Authorship reads from [bubbleRemoteBg] — the surface, not a stripe.

  /// Own event tile tint border (accent @ 0.34).
  Color get ownTileBorder => _alpha(_accent, 0.34);

  /// Agent work card (structured activity, not chat).
  Color get agentCardBg => _alpha(_blue, 0.08);
  Color get agentCardBorder => _alpha(_blue, 0.22);

  // -- banners / modal ------------------------------------------------------------------

  Color get bannerReconnectBg => _alpha(_amber, 0.14);
  Color get bannerReconnectBorder => _alpha(_amber, 0.35);
  Color get bannerDisconnectBg => _alpha(_red, 0.14);
  Color get errorNoteBg => _alpha(_red, 0.09);
  Color get errorNoteBorder => _alpha(_red, 0.35);

  /// Modal backdrop rgba(3,7,9,0.72).
  Color get modalBarrier => const Color(0xB8030709);

  // -- deterministic identity colors -----------------------------------------------------

  /// Avatar palette (deterministic per id; see [colorForId]).
  List<Color> get avatarPalette => const [
        _accent,
        _blue,
        _violet,
        Color(0xFFFB923C), // orange
        Color(0xFFF472B6), // pink
        Color(0xFF22D3EE), // cyan
      ];

  /// `colorForId` (format.ts): h = (h*31 + charCode) >>> 0 over the id,
  /// mod the 6-color palette.
  Color colorForId(String id) {
    var h = 0;
    for (var i = 0; i < id.length; i++) {
      h = (h * 31 + id.codeUnitAt(i)) & 0xFFFFFFFF;
    }
    return avatarPalette[h % avatarPalette.length];
  }

  /// Avatar fill: the id color at 15% alpha (hex suffix `26`).
  Color avatarBg(String id) => colorForId(id).withAlpha(0x26);

  /// Room/fleet hex-tile fill: the id color at 12% alpha (hex suffix `1f`).
  Color tileBg(String id) => colorForId(id).withAlpha(0x1F);

  /// File-type tint by extension (format.ts `fileTint`).
  Color fileTint(String name) {
    final dot = name.lastIndexOf('.');
    final ext = dot >= 0 ? name.substring(dot + 1).toLowerCase() : '';
    switch (ext) {
      case 'pdf':
        return red;
      case 'md':
      case 'txt':
      case 'doc':
      case 'docx':
        return blue;
      case 'json':
      case 'js':
      case 'ts':
        return accent;
      case 'png':
      case 'jpg':
      case 'jpeg':
      case 'gif':
      case 'svg':
      case 'webp':
        return _violet;
      default:
        return textDim;
    }
  }

  /// The text/dot color for an agent-status [LabelTone] — green is EARNED
  /// (honesty rule 4); neutral renders [textDim] with no glow.
  Color toneColor(LabelTone tone) => switch (tone) {
        LabelTone.red => red,
        LabelTone.blue => blue,
        LabelTone.green => accent,
        LabelTone.neutral => textDim,
      };

  /// The tinted chip background for a [LabelTone] (neutral gets none).
  Color? toneBg(LabelTone tone) => switch (tone) {
        LabelTone.red => redDim,
        LabelTone.blue => blueDim,
        LabelTone.green => accentDim,
        LabelTone.neutral => null,
      };

  /// The chip border for a [LabelTone] (neutral falls back to [borderStrong]).
  Color toneBorder(LabelTone tone) => switch (tone) {
        LabelTone.red => redLine,
        LabelTone.blue => blueLine,
        LabelTone.green => accentLine,
        LabelTone.neutral => borderStrong,
      };

  // Dark-only this phase: a single canonical instance, so theme transitions
  // have nothing to interpolate.
  @override
  JeliyaTokens copyWith() => this;

  @override
  JeliyaTokens lerp(ThemeExtension<JeliyaTokens>? other, double t) =>
      other is JeliyaTokens && t >= 0.5 ? other : this;
}

// -- spacing / radii scales (phase3-design.json layout) --------------------------

/// Spacing scale in logical px: 2,4,6,8,10,12,14,16,18,24.
abstract final class JeliyaSpacing {
  static const double x2 = 2;
  static const double x4 = 4;
  static const double x6 = 6;
  static const double x8 = 8;
  static const double x10 = 10;
  static const double x12 = 12;
  static const double x14 = 14;
  static const double x16 = 16;
  static const double x18 = 18;
  static const double x24 = 24;

  /// 14px panel padding.
  static const double panel = x14;

  /// 24px page-level padding.
  static const double page = x24;

  /// 18px section padding.
  static const double section = x18;
}

/// Radii scale (phase3-design.json): every value is deliberate.
abstract final class JeliyaRadii {
  /// Bubble sharp corner.
  static const double bubbleSharp = 4;

  /// icon-btn / skeleton / pipe-addr chip.
  static const double iconBtn = 7;

  /// btn-sm.
  static const double btnSm = 8;

  /// btn / inputs / 34px square tiles.
  static const double btn = 9;

  /// nav items / stat inner cells.
  static const double nav = 10;

  /// room rows / file+pipe tiles / agent-work card.
  static const double row = 11;

  /// profile + settings cards.
  static const double card = 12;

  /// composer bar / agent+pipe cards / panel forms.
  static const double composer = 13;

  /// member/file rows / bubble round corners / stat tiles.
  static const double bubble = 14;

  /// heroes / fleet cards.
  static const double hero = 15;

  /// modal / onboarding card.
  static const double modal = 16;

  /// All pills/chips/badges.
  static const double pill = 999;
}

// -- typography (phase3-design.json typography) -----------------------------------

/// Type scale + families. The UI family is the platform system stack (San
/// Francisco on macOS — Flutter's default); mono is Menlo-first.
abstract final class JeliyaText {
  /// Mono stack for peer ids, hashes, pipe addresses, stat values, paths.
  static const List<String> monoFamily = [
    'Menlo',
    'SF Mono',
    'Consolas',
    'monospace',
  ];

  /// Base body: 14px / 1.5.
  static const TextStyle body = TextStyle(
    fontSize: 14,
    height: 1.5,
    color: _text,
    fontWeight: FontWeight.w400,
  );

  /// `.mono` renders at 0.92em of context; this is the 14px-context size.
  static TextStyle mono({
    double fontSize = 12.9,
    Color color = _textDim,
    FontWeight fontWeight = FontWeight.w400,
  }) =>
      TextStyle(
        fontFamily: monoFamily.first,
        fontFamilyFallback: monoFamily.sublist(1),
        fontSize: fontSize,
        color: color,
        fontWeight: fontWeight,
      );

  /// Timestamps / room meta / hints: 11.5px, always [JeliyaTokens.textMute].
  static const TextStyle meta = TextStyle(fontSize: 11.5, color: _textMute);

  /// Syslines / day divider / fetch detail: 12px.
  static const TextStyle sysline = TextStyle(fontSize: 12, color: _textMute);

  /// Secondary copy / field labels / conn banner: 12.5px.
  static const TextStyle secondary = TextStyle(fontSize: 12.5, color: _textDim);

  /// Names / tabs: 13.5px weight 600.
  static const TextStyle name = TextStyle(
    fontSize: 13.5,
    fontWeight: FontWeight.w600,
    color: _text,
  );

  /// Card h2 (heroes): 15px / 1.25.
  static const TextStyle cardTitle = TextStyle(
    fontSize: 15,
    height: 1.25,
    fontWeight: FontWeight.w700,
    color: _text,
  );

  /// Modal title: 16px / 700.
  static const TextStyle modalTitle = TextStyle(
    fontSize: 16,
    fontWeight: FontWeight.w700,
    color: _text,
  );

  /// Onboarding card h2: 17px.
  static const TextStyle onboardingCardTitle = TextStyle(
    fontSize: 17,
    fontWeight: FontWeight.w700,
    color: _text,
  );

  /// Room title h1 / sidebar brand: 19px / 700.
  static const TextStyle roomTitle = TextStyle(
    fontSize: 19,
    fontWeight: FontWeight.w700,
    color: _text,
  );

  /// Boot h1 / fleet stat value: 26px.
  static const TextStyle bootTitle = TextStyle(
    fontSize: 26,
    fontWeight: FontWeight.w700,
    color: _text,
  );

  /// Onboarding h1: 30px.
  static const TextStyle onboardingTitle = TextStyle(
    fontSize: 30,
    fontWeight: FontWeight.w700,
    color: _text,
  );

  /// The wordmark: display stack, 700, +0.01em tracking, ink — NEVER accent
  /// (the TreeMark carries the accent).
  static TextStyle wordmark(double fontSize) => TextStyle(
        fontSize: fontSize,
        fontWeight: FontWeight.w700,
        letterSpacing: fontSize * 0.01,
        color: _text,
      );

  /// Micro uppercase labels (identity/settings labels, 11px +0.06em).
  static const TextStyle microLabel = TextStyle(
    fontSize: 11,
    letterSpacing: 0.66,
    color: _textMute,
  );
}

/// The app [ThemeData]: dark, Material 3, token-driven component defaults.
ThemeData buildJeliyaTheme() {
  const tokens = JeliyaTokens.dark;
  final base = ThemeData(
    useMaterial3: true,
    brightness: Brightness.dark,
    colorScheme: ColorScheme.fromSeed(
      seedColor: tokens.accent,
      brightness: Brightness.dark,
      surface: tokens.bg,
      error: tokens.red,
    ).copyWith(
      primary: tokens.accent,
      secondary: tokens.accent2,
      outline: tokens.borderStrong,
      outlineVariant: tokens.border,
      onSurface: tokens.text,
    ),
  );

  final textTheme = base.textTheme
      .apply(bodyColor: tokens.text, displayColor: tokens.text)
      .copyWith(
        bodyMedium: JeliyaText.body,
        bodySmall: JeliyaText.secondary,
        labelSmall: JeliyaText.meta,
        titleMedium: JeliyaText.cardTitle,
        titleLarge: JeliyaText.roomTitle,
      );

  return base.copyWith(
    scaffoldBackgroundColor: tokens.bg,
    canvasColor: tokens.bg,
    dividerColor: tokens.border,
    splashFactory: NoSplash.splashFactory,
    hoverColor: tokens.bgCard,
    focusColor: tokens.accentDim,
    highlightColor: tokens.bgCard2,
    textTheme: textTheme,
    extensions: const [tokens],
    dialogTheme: DialogThemeData(
      backgroundColor: tokens.bgRaise,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(JeliyaRadii.modal),
        side: BorderSide(color: tokens.borderStrong),
      ),
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: tokens.bgInput,
      isDense: true,
      contentPadding: const EdgeInsets.symmetric(horizontal: 11, vertical: 8),
      hintStyle: const TextStyle(color: _textMute, fontSize: 14),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(JeliyaRadii.btn),
        borderSide: BorderSide(color: tokens.borderStrong),
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(JeliyaRadii.btn),
        borderSide: BorderSide(color: tokens.accentLine),
      ),
      disabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(JeliyaRadii.btn),
        borderSide: BorderSide(color: tokens.border),
      ),
    ),
    scrollbarTheme: ScrollbarThemeData(
      thumbColor: WidgetStatePropertyAll(tokens.borderStrong),
      radius: const Radius.circular(JeliyaRadii.pill),
    ),
    tooltipTheme: TooltipThemeData(
      decoration: BoxDecoration(
        color: tokens.bgCard2,
        borderRadius: BorderRadius.circular(JeliyaRadii.iconBtn),
        border: Border.all(color: tokens.borderStrong),
      ),
      textStyle: JeliyaText.secondary,
      waitDuration: const Duration(milliseconds: 400),
    ),
  );
}
