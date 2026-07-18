/** Strings that are NOT copy, and must never reach a translator.
 *
 *  The Flutter client keeps these in `l10n/tokens.dart` for the same reason
 *  (`docs/i18n.md`, rule 1): a naive extractor sweeps glyphs, shell commands
 *  and wire-format examples into the catalog, where a translator is then asked
 *  to localize `jeliyad` or `▦`. Worse, `docs/glossary-fr.md` Tier 2 makes some
 *  of these a correctness matter — a connection path badge must render exactly
 *  what the daemon reported, so "translating" `direct` would be a lie about the
 *  network.
 *
 *  Everything here is deliberately outside `Catalog`.
 */

/** Decorative glyphs. Every one of these is `aria-hidden` at its call site —
 *  they carry no information the adjacent label does not already give. */
export const Glyph = {
  rooms: '▦',
  fleet: '✦',
  settings: '⚙',
  hex: '⬡',
  copy: '⧉',
  file: '⎘',
  pipe: '⤳',
  send: '➤',
  close: '✕',
  chevronDown: '⌄',
  disclosureOpen: '▾',
  disclosureClosed: '▸',
  back: '‹',
  more: '⋮',
  pinOn: '★',
  pinOff: '☆',
  archive: '⇩',
  restore: '⇧',
  create: '⊕',
  join: '⇥',
  add: '＋',
  externalLink: '↗',
  openRoom: '⇱',
  previous: '←',
  share: '↗',
  verified: '✓',
  failed: '✕',
} as const;

/** Punctuation and layout separators. Not sentences — a list of independent
 *  facts may be joined with these, but a SENTENCE may not (rule 2). */
export const Punct = {
  /** The decorative separator between independent facts on a meta line. */
  metaSep: ' · ',
  emDash: '—',
  /** Shown where a value is genuinely absent, never as a zero. */
  missingValue: '—',
  /** Count cap on a tab badge. */
  countCap: '99+',
} as const;

/** The brand. Never translated, never re-derived — DESIGN.md makes the wordmark
 *  a single source of truth. */
export const BRAND = 'Jeliya';

/** Language endonyms for the picker. A language is named in ITSELF, so these
 *  are the one set of language names that must never be translated — and a
 *  picker showing a bare ISO code is a bug the locale test catches. */
export const LANGUAGE_NAMES: Record<string, string> = {
  en: 'English',
  fr: 'Français',
};

/** Wire-format examples used as input placeholders. These show the user the
 *  SHAPE of a machine value; translating them would teach the wrong shape. */
export const Example = {
  ticket: 'roomtkt1… or roomtkt1…#<endpoint_id>@host:port',
  peerAddress: '<endpoint_id>@203.0.113.7:4242',
  pipeTarget: '127.0.0.1:3000',
  expirySeconds: '3600',
  identityId: '64-hex identity id',
  roomName: 'Build Iroh Rooms MVP',
  alias: 'e.g. Maya R.',
  selfLabel: 'e.g. Alex',
} as const;

/** Shell commands and paths. Copy-pasteable machine input — a translated
 *  command does not run. */
export const Command = {
  daemon: 'jeliyad',
  daemonPortParam: '?daemon=<port>',
  npmInstall: 'npm install',
  daemonPath: 'JELIYAD="$(command -v jeliyad)"',
  agentGuide: 'docs/agent-guide.md',
} as const;

/** The issue tracker. A URL is not copy. */
export const ISSUE_URL = 'https://github.com/kortiene/jeliya/issues/new';
