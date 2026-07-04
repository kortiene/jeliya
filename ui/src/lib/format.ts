// Small display helpers — no protocol knowledge here.

export function shortId(id: string): string {
  const raw = id.includes(':') ? id.slice(id.indexOf(':') + 1) : id;
  if (raw.length <= 10) return raw;
  return `${raw.slice(0, 4)}…${raw.slice(-4)}`;
}

export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return '?';
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(1)} GB`;
}

export function formatTime(ts: number): string {
  return new Date(ts).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
}

export function dayLabel(ts: number): string {
  const date = new Date(ts);
  const today = new Date();
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  if (date.toDateString() === today.toDateString()) return 'Today';
  if (date.toDateString() === yesterday.toDateString()) return 'Yesterday';
  return date.toLocaleDateString([], { month: 'short', day: 'numeric', year: 'numeric' });
}

/** Relative time from a real event timestamp — display only, never a
 *  liveness claim. */
export function relTime(ts: number): string {
  const delta = Date.now() - ts;
  if (delta < 45_000) return 'just now';
  const mins = Math.round(delta / 60_000);
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  return `${days}d ago`;
}

export function prettyLabel(label: string): string {
  const s = label.replace(/[_-]+/g, ' ').trim();
  return s ? s.charAt(0).toUpperCase() + s.slice(1) : label;
}

/** Tone for an agent-status label: red for failures, blue for waiting, else
 *  green. Shared so the timeline chip and the Agents panel dot agree. */
export function labelTone(label: string): 'red' | 'blue' | 'green' {
  const l = label.toLowerCase();
  if (l.includes('fail') || l.includes('error') || l.includes('block')) return 'red';
  if (l.includes('await') || l.includes('review') || l.includes('pend')) return 'blue';
  return 'green';
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
