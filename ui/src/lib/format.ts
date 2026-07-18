// Small display helpers — no protocol knowledge here.

export function shortId(id: string): string {
  const raw = id.includes(':') ? id.slice(id.indexOf(':') + 1) : id;
  if (raw.length <= 10) return raw;
  return `${raw.slice(0, 4)}…${raw.slice(-4)}`;
}

export function prettyLabel(label: string): string {
  const s = label.replace(/[_-]+/g, ' ').trim();
  return s ? s.charAt(0).toUpperCase() + s.slice(1) : label;
}

/** Tone for an agent-status label. Shared so the timeline chip and the Agents
 *  panel dot agree.
 *
 *  Green is earned, never the fallback: only labels carrying a known
 *  healthy/active token render the accent. Labels are free-form wire data —
 *  a label this English-token contract can't read (including any non-English
 *  label, e.g. `échec`) must render neutral, because guessing green for it
 *  would be exactly the reassuring-but-untrue state the honesty rules forbid.
 *  The token contract is documented in docs/agent-guide.md. */
const GREEN_TOKENS =
  /\b(done|working|online|ready|pass|passed|success|successful|ok|complete|completed|connected|healthy|active|running|verified|live)\b/;
// Word-boundary tokens, not substrings: `review` must not match inside
// `preview` (preview_ready is a success label, not a waiting one).
const BLUE_TOKENS = /\b(await(ing)?|review(ing|ed)?|pend(ing)?)\b/;

export function labelTone(label: string): 'red' | 'blue' | 'green' | 'neutral' {
  const l = label.toLowerCase().replace(/[_-]+/g, ' ');
  // Substrings on purpose for red: a false alarm is the honest direction to
  // err in — never the reverse.
  if (l.includes('fail') || l.includes('error') || l.includes('block')) return 'red';
  if (BLUE_TOKENS.test(l)) return 'blue';
  if (GREEN_TOKENS.test(l)) return 'green';
  return 'neutral';
}

// Must stay in sync with the corresponding tokens in styles.css.
const ACCENT = '#2fd6a4'; // --accent
const BLUE = '#6aa8f7'; // --blue
const RED = '#f26d6d'; // --red
const TEXT_DIM = '#8aa39d'; // --text-dim
const VIOLET = '#a78bfa'; // no CSS token — shared here by AVATAR_PALETTE and fileTint

const AVATAR_PALETTE = [ACCENT, BLUE, VIOLET, '#fb923c', '#f472b6', '#22d3ee'];

export function colorForId(id: string): string {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (Math.imul(h, 31) + id.charCodeAt(i)) >>> 0;
  return AVATAR_PALETTE[h % AVATAR_PALETTE.length];
}

export function initials(name: string): string {
  const parts = name.split(/\s+/).filter(Boolean);
  if (parts.length === 0) return '?';
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0].charAt(0) + parts[1].charAt(0)).toUpperCase();
}

export function extOf(name: string): string {
  const i = name.lastIndexOf('.');
  return i >= 0 ? name.slice(i + 1).toLowerCase() : '';
}

export function fileTint(name: string): string {
  switch (extOf(name)) {
    case 'pdf':
      return RED;
    case 'md':
    case 'txt':
    case 'doc':
    case 'docx':
      return BLUE;
    case 'json':
    case 'js':
    case 'ts':
      return ACCENT;
    case 'png':
    case 'jpg':
    case 'jpeg':
    case 'gif':
    case 'svg':
    case 'webp':
      return VIOLET;
    default:
      return TEXT_DIM;
  }
}
