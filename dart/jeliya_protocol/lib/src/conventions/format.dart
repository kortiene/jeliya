/// Display-classification helpers ported verbatim from ui/src/lib/format.ts —
/// only the pieces the protocol conventions themselves depend on ([labelTone]
/// per docs/PROTOCOL.md "Agent-status label tone", and [shortId] which the
/// diagnostics report uses to truncate identifiers). Purely visual formatting
/// (bytes, times, avatars) stays in the app layer.
library;

/// Tone classes for an agent-status label. Derived, never wire data.
enum LabelTone { red, blue, green, neutral }

// Green is earned, never the fallback: only labels carrying a known
// healthy/active token render the accent (honesty rule 4). The token contract
// is documented in docs/agent-guide.md.
final RegExp _greenTokens = RegExp(
    r'\b(done|working|online|ready|pass|passed|success|successful|ok|complete|completed'
    r'|connected|healthy|active|running|verified|live)\b');
// Word-boundary tokens, not substrings: `review` must not match inside
// `preview` (preview_ready is a success label, not a waiting one).
final RegExp _blueTokens = RegExp(r'\b(await(ing)?|review(ing|ed)?|pend(ing)?)\b');
final RegExp _separators = RegExp('[_-]+');

/// Tone for an agent-status label (format.ts `labelTone`) — shared so the
/// timeline chip and the Agents panel dot agree, and mirrored exactly across
/// clients (docs/PROTOCOL.md "Agent-status label tone").
///
/// Applied to the label lowercased with `_`/`-` collapsed to spaces, in this
/// precedence: red substrings (`fail`/`error`/`block` — substrings on purpose,
/// a false alarm is the honest direction to err in), then blue word-boundary
/// tokens, then green word-boundary tokens, else neutral. Labels are free-form
/// wire data — a label this English-token contract can't read (including any
/// non-English label, e.g. `échec`) must render neutral, because guessing
/// green for it would be exactly the reassuring-but-untrue state the honesty
/// rules forbid.
LabelTone labelTone(String label) {
  final l = label.toLowerCase().replaceAll(_separators, ' ');
  if (l.contains('fail') || l.contains('error') || l.contains('block')) return LabelTone.red;
  if (_blueTokens.hasMatch(l)) return LabelTone.blue;
  if (_greenTokens.hasMatch(l)) return LabelTone.green;
  return LabelTone.neutral;
}

/// Truncate an identifier for display (format.ts `shortId`): drop any
/// `<scheme>:` prefix, then keep 4+4 chars around an ellipsis once the raw id
/// exceeds 10 chars.
String shortId(String id) {
  final raw = id.contains(':') ? id.substring(id.indexOf(':') + 1) : id;
  if (raw.length <= 10) return raw;
  return '${raw.substring(0, 4)}…${raw.substring(raw.length - 4)}';
}
