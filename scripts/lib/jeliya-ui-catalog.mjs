// Shared parser and rule set for the Dioxus (jeliya-ui) EN/FR catalog gates
// (#177, spec §4-D2/§6.1). One implementation, exposed as three independently
// required CI contexts through `scripts/check-jeliya-ui-i18n.mjs --only=<group>`.
//
// Why read the Rust catalogs as TEXT rather than importing the crate: a gate
// that evaluates the thing it gates can be argued out of its own findings, and
// `scripts/` has no Rust build step — the exact decision `check-ui-i18n.mjs`
// and `check-docs.mjs` already make for TypeScript and YAML. `rustc` already
// enforces key and placeholder parity (a missing `Catalog` method does not
// compile); these rules are defence in depth for what types cannot see:
//
//  - catalog group: key-set parity between en.rs and fr.rs, no empty value, no
//    French value byte-identical to English (with a stale-checked exemption
//    lexicon + allowlist), and plural methods present with both arms.
//  - typography group: the French typography contract over every fr value.
//  - literals group: user-visible string literals in app.rs / components/** that
//    never reach the catalog (the Rust analogue of check-ui-i18n rule 5).
//
// Values are decoded through Rust escapes (notably `\u{202f}`, the narrow
// no-break space the French percent rule requires) so the typography check sees
// the character a reviewer cannot.

import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPT_PATH = fileURLToPath(import.meta.url);
export const DEFAULT_REPO_ROOT = resolve(dirname(SCRIPT_PATH), '..', '..');

/** The catalogs, relative to the repository root. `en` is the source of truth. */
export const LOCALE_FILES = Object.freeze({
  en: 'crates/jeliya-ui/src/l10n/en.rs',
  fr: 'crates/jeliya-ui/src/l10n/fr.rs',
});

/** Where the literal scan looks: the app root plus every shared component. The
 *  l10n layer itself is excluded — it IS the catalog. */
const LITERAL_SCAN_ROOTS = Object.freeze([
  'crates/jeliya-ui/src/app.rs',
  'crates/jeliya-ui/src/components',
]);

/** Tier 2 / Tier 3 never-translate lexicon (docs/glossary-fr.md): words that are
 *  the SAME in French, so a value built only from them is identical on purpose.
 *  Lowercased and accent-stripped before lookup. */
const NEVER_TRANSLATE = new Set([
  'jeliya',
  'agent',
  'agents',
  'daemon',
  'jeliyad',
  'direct',
  'relay',
  'relais',
  'endpoint',
  'endpoints',
  'ticket',
  'tickets',
  'id',
  'ids',
  'hash',
  'mismatch',
  'loopback',
  'ok',
  'ui',
  'io',
  'p2p',
  'url',
]);

/** Keys whose French value is legitimately byte-identical to English, key ->
 *  reason. Checked BOTH ways: an entry whose key is gone, or whose values now
 *  differ, is itself reported — a stale exemption hides the next real one. */
export const IDENTICAL_ALLOWLIST = Object.freeze({
  diagnostics_open:
    '“Diagnostics” is the standard French plural for diagnostic information and ' +
    'is deliberately identical to the English label (mirrors the React ' +
    'settingsDiagnosticsTitle exemption).',
  diagnostics_title:
    '“Diagnostics” is correct French for the dialog heading; identical to ' +
    'English on purpose.',
  client_status:
    '“client” is the same word in French; the status line is the brand-neutral ' +
    '“client · {état}” framing with the state word supplied localized.',
  wire_path_direct:
    'Tier-2 protocol token (docs/glossary-fr.md): `direct` is rendered verbatim ' +
    'as the daemon reports it, identical in every language.',
  wire_path_relay:
    'Tier-2 protocol token (docs/glossary-fr.md): `relay` is rendered verbatim ' +
    'as the daemon reports it, identical in every language.',
});

/** Stands in for a `{…}` format slot when a message is compared or
 *  typography-checked as one sentence. Not a space, and it can never occur in
 *  copy, so an adjacent slot never looks like the space the French rule is
 *  about. */
const SLOT = '\u0000';

// ---------------------------------------------------------------------------
// A restricted Rust scanner
// ---------------------------------------------------------------------------

/** Decode the Rust escapes a catalog value can contain. `\u{202f}` is the one
 *  that matters most: the narrow no-break space the French percent rule needs,
 *  written as an escape in fr.rs so it is visible in review. */
function unescapeRust(raw) {
  return raw.replace(/\\(u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|[\s\S])/g, (_, escape) => {
    if (escape.startsWith('u{')) return String.fromCodePoint(parseInt(escape.slice(2, -1), 16));
    if (/^x[0-9a-fA-F]{2}$/.test(escape)) return String.fromCodePoint(parseInt(escape.slice(1), 16));
    return { n: '\n', t: '\t', r: '\r', '0': '\0', '\\': '\\', '"': '"', "'": "'" }[escape] ?? escape;
  });
}

/** Blank comments (newlines preserved so line numbers survive) and collect every
 *  string literal with its decoded value and source position. Also returns a
 *  `skeleton` (code with string CONTENTS blanked) so structural look-behind
 *  cannot trip over a brace or colon inside copy.
 *
 *  Handles `"…"` and raw strings `r#"…"#`. Rust `'` (lifetimes and char
 *  literals) is not string-significant in any scanned file and is ignored. */
export function scanRustSource(source) {
  const masked = Array.from(source);
  const literals = [];
  const comments = [];
  const blank = (from, to) => {
    for (let i = from; i < to; i += 1) if (masked[i] !== '\n') masked[i] = ' ';
  };

  let i = 0;
  while (i < source.length) {
    const ch = source[i];
    if (ch === '/' && source[i + 1] === '/') {
      const end = source.indexOf('\n', i);
      const stop = end === -1 ? source.length : end;
      comments.push({ start: i, end: stop });
      blank(i, stop);
      i = stop;
      continue;
    }
    if (ch === '/' && source[i + 1] === '*') {
      const end = source.indexOf('*/', i + 2);
      const stop = end === -1 ? source.length : end + 2;
      comments.push({ start: i, end: stop });
      blank(i, stop);
      i = stop;
      continue;
    }
    // Raw string: r, optional #…#, then " … " with the same # count.
    if (ch === 'r' && (source[i + 1] === '"' || source[i + 1] === '#')) {
      let j = i + 1;
      let hashes = 0;
      while (source[j] === '#') {
        hashes += 1;
        j += 1;
      }
      if (source[j] === '"') {
        const close = '"' + '#'.repeat(hashes);
        const end = source.indexOf(close, j + 1);
        const stop = end === -1 ? source.length : end + close.length;
        literals.push({
          start: i,
          end: stop,
          contentStart: j + 1,
          contentEnd: end === -1 ? source.length : end,
          value: source.slice(j + 1, end === -1 ? source.length : end),
          raw: true,
        });
        i = stop;
        continue;
      }
    }
    if (ch === '"') {
      let j = i + 1;
      while (j < source.length) {
        if (source[j] === '\\') {
          j += 2;
          continue;
        }
        if (source[j] === '"' || source[j] === '\n') break;
        j += 1;
      }
      const contentEnd = j;
      literals.push({
        start: i,
        end: Math.min(j + 1, source.length),
        contentStart: i + 1,
        contentEnd,
        value: unescapeRust(source.slice(i + 1, contentEnd)),
        raw: false,
      });
      i = Math.min(j + 1, source.length);
      continue;
    }
    i += 1;
  }

  const skeleton = Array.from(masked.join(''));
  for (const literal of literals) {
    for (let at = literal.contentStart; at < literal.contentEnd; at += 1) {
      if (skeleton[at] !== '\n') skeleton[at] = ' ';
    }
  }
  return { skeleton: skeleton.join(''), literals, comments };
}

function lineOf(source, index) {
  let line = 1;
  for (let at = 0; at < index && at < source.length; at += 1) {
    if (source.charCodeAt(at) === 10) line += 1;
  }
  return line;
}

function finding(file, line, code, message, group) {
  return { file, line, code, message, group };
}

/** Collapse `{…}` format slots to the sentinel (leaving `{{`/`}}` literal braces
 *  alone is unnecessary — no catalog value uses them). */
function collapseSlots(text) {
  return text.replace(/\{[^{}]*\}/g, SLOT);
}

/** The sorted multiset of format placeholders across a value set (`{n}` -> "n").
 *  Rust enforces only the method SIGNATURE and permits an unused argument, so a
 *  translation that drops, renames, or duplicates a `{…}` slot still compiles;
 *  comparing this set between EN and FR is what actually enforces placeholder
 *  parity. */
function slotSet(values) {
  const slots = [];
  for (const value of values) {
    for (const match of value.matchAll(/\{([^{}]*)\}/g)) slots.push(match[1]);
  }
  return slots.sort();
}

function words(text) {
  return (
    text
      .normalize('NFD')
      .replace(/[̀-ͯ]/g, '')
      .toLowerCase()
      .match(/[a-z]{2,}/g) ?? []
  );
}

// ---------------------------------------------------------------------------
// Catalog parsing
// ---------------------------------------------------------------------------

/** Parse a locale catalog (`en.rs` / `fr.rs`) into key -> entry.
 *
 *  A catalog method is `fn <name>(&self …) -> … { <body> }`. An entry records
 *  the decoded string literal(s) the body contributes (slots collapsed), whether
 *  it is a plural method (its signature names `PluralCategory`), and its line. */
export function parseCatalog(source, file) {
  const { skeleton, literals } = scanRustSource(source);
  const entries = new Map();
  const errors = [];
  const methodRe = /\bfn\s+([a-z_][A-Za-z0-9_]*)\s*\(\s*&self\b([^)]*)\)\s*->\s*[^{;]*\{/g;
  let match;
  while ((match = methodRe.exec(skeleton)) !== null) {
    const name = match[1];
    const params = match[2];
    const open = match.index + match[0].length - 1;
    // Match the body braces on the skeleton (string contents are blanked, so a
    // `}` inside copy cannot end the body early).
    let depth = 0;
    let close = -1;
    for (let at = open; at < skeleton.length; at += 1) {
      if (skeleton[at] === '{') depth += 1;
      else if (skeleton[at] === '}') {
        depth -= 1;
        if (depth === 0) {
          close = at;
          break;
        }
      }
    }
    if (close === -1) {
      errors.push(finding(file, lineOf(source, open), 'catalog-unparsed', `method ${name} body is unterminated`, 'catalog'));
      continue;
    }
    const parts = literals.filter((l) => l.start >= open && l.end <= close);
    const isPlural = /\bPluralCategory\b/.test(params);
    if (entries.has(name)) {
      errors.push(finding(file, lineOf(source, open), 'catalog-duplicate-key', `duplicate method: ${name}`, 'catalog'));
    }
    // For a plural method, group each literal by its PluralCategory arm (One /
    // Other) rather than by SOURCE POSITION, so a locale that lists the arms in a
    // different order is still compared category-against-category (parity would
    // otherwise misalign fr `Other` against en `One`).
    let slotsByCategory = null;
    if (isPlural) {
      const bodyText = skeleton.slice(open, close);
      const markers = [];
      const catRe = /PluralCategory::(One|Other)\b/g;
      for (let m = catRe.exec(bodyText); m; m = catRe.exec(bodyText)) {
        markers.push({ at: open + m.index, cat: m[1] });
      }
      const valuesByCat = { One: [], Other: [] };
      for (const p of parts) {
        let cat = null;
        for (const mk of markers) {
          if (mk.at <= p.start) cat = mk.cat;
          else break;
        }
        if (cat) valuesByCat[cat].push(p.value);
      }
      slotsByCategory = {
        One: slotSet(valuesByCat.One),
        Other: slotSet(valuesByCat.Other),
      };
    }
    entries.set(name, {
      key: name,
      line: lineOf(source, match.index),
      isPlural,
      values: parts.map((p) => collapseSlots(p.value)),
      slots: slotSet(parts.map((p) => p.value)),
      slotsPerArm: parts.map((p) => slotSet([p.value])),
      slotsByCategory,
    });
    methodRe.lastIndex = close;
  }
  if (entries.size === 0) {
    errors.push(finding(file, 1, 'catalog-unparsed', 'no `fn <name>(&self …) -> … { … }` catalog methods found', 'catalog'));
  }
  return { entries, errors };
}

/** Why an identical French value may be identical, or null if it may not. The
 *  allowlist is a parameter so the companion test can exercise the RULE without
 *  depending on today's exemptions. */
export function identityExemption(key, text, allowlist = IDENTICAL_ALLOWLIST) {
  if (Object.hasOwn(allowlist, key)) return { source: 'allowlist', reason: allowlist[key] };
  const found = words(text);
  if (found.length === 0) return { source: 'automatic', reason: 'no translatable word' };
  if (found.every((word) => NEVER_TRANSLATE.has(word))) {
    return { source: 'automatic', reason: 'never-translate lexicon' };
  }
  return null;
}

/** The French typography contract (docs/glossary-fr.md decision 7; §5.4), spelled
 *  with explicit escapes so a reviewer sees which invisible space each rule
 *  means. Run against the rendered value with slots collapsed to the sentinel. */
/** Human name for the character just before a typography match, so the
 *  message says whether it was a wrong space or no space at all. */
function describePreceding(m) {
  const i = m.index;
  if (i === 0) return 'nothing (start of value)';
  const c = m.input[i - 1];
  if (c === ' ') return 'a plain space';
  if (c === ' ') return 'U+00A0';
  if (c === ' ') return 'U+202F';
  return `'${c}' (U+${c.codePointAt(0).toString(16).padStart(4, '0')})`;
}

const TYPOGRAPHY = Object.freeze([
  {
    code: 'fr-narrow-space',
    // Any of ; ! ? % that is NOT immediately preceded by U+202F — this catches
    // both the WRONG space (plain / U+00A0) and NO space at all (`Bonjour!`),
    // since the contract requires exactly U+202F before these marks.
    pattern: /(?<! )([;!?%])/,
    message: (m) =>
      `"${m[1]}" must be preceded by U+202F (narrow no-break space); found ${describePreceding(m)}`,
  },
  {
    code: 'fr-no-break-space',
    // A colon NOT immediately preceded by U+00A0 — catches wrong space and no
    // space (`Bonjour:`) alike.
    pattern: /(?<! ):/,
    message: (m) => `":" must be preceded by U+00A0 (no-break space); found ${describePreceding(m)}`,
  },
  { code: 'fr-apostrophe', pattern: /'/, message: () => 'straight apostrophe — French copy uses U+2019' },
  { code: 'fr-quotes', pattern: /["“”]/, message: () => 'double quotes — French copy uses guillemets « »' },
  {
    code: 'fr-guillemet-space',
    pattern: /«(?! )|(?<! )»/,
    message: () => 'guillemets take a U+202F narrow no-break space on the inside',
  },
  { code: 'fr-ellipsis', pattern: /\.\.\./, message: () => 'three dots — use U+2026 (…)' },
]);

/** Catalog + typography rules over an already-parsed pair. */
export function checkCatalogs({ en, fr, allowlist = IDENTICAL_ALLOWLIST }) {
  const findings = [];

  // Rule 1 — key-set parity between the two catalogs, both directions.
  for (const key of en.entries.keys()) {
    if (!fr.entries.has(key)) {
      findings.push(finding(fr.file, 1, 'key-missing', `fr catalog is missing method: ${key}`, 'catalog'));
    }
  }
  for (const key of fr.entries.keys()) {
    if (!en.entries.has(key)) {
      findings.push(finding(fr.file, fr.entries.get(key).line, 'key-undeclared', `fr catalog has a method en does not: ${key}`, 'catalog'));
    }
  }

  // Rule 2 — no empty value, in either catalog. `some`, not `every`: a plural
  // whose ONE arm is empty (`One => ""` with a nonempty `Other`) renders a blank
  // message for that count, so ANY empty rendered branch fails — not only an
  // all-empty method.
  const isEmptyValue = (v) => v.replace(new RegExp(SLOT, 'g'), '').trim() === '';
  for (const locale of [en, fr]) {
    for (const entry of locale.entries.values()) {
      const empty = entry.values.length === 0 || entry.values.some(isEmptyValue);
      if (empty) {
        findings.push(finding(locale.file, entry.line, 'value-empty', `${entry.key}: value is empty or whitespace-only`, 'catalog'));
      }
    }
  }

  // Rule 5 — plural methods present in both, with both arms.
  for (const [key, frEntry] of fr.entries) {
    const enEntry = en.entries.get(key);
    if (!enEntry) continue;
    if (enEntry.isPlural !== frEntry.isPlural) {
      findings.push(finding(fr.file, frEntry.line, 'plural-parity', `${key}: plural-ness differs between en and fr`, 'catalog'));
    } else if (frEntry.isPlural && (frEntry.values.length < 2 || enEntry.values.length < 2)) {
      findings.push(finding(fr.file, frEntry.line, 'plural-parity', `${key}: a plural message must render both categories (one/other) in both locales`, 'catalog'));
    }
    // Placeholder parity: EN and FR must interpolate the SAME format slots — and
    // PER ARM, not pooled across the method. Pooling would let a French plural
    // One => "article" + Other => "{n}{n} articles" match English arms that each
    // use {n} (same [n, n] multiset) while one rendered arm actually drops or
    // doubles a slot. Compare arm-by-arm when the arm counts line up (a differing
    // count is already a plural-parity finding).
    if (frEntry.isPlural && enEntry.isPlural && frEntry.slotsByCategory && enEntry.slotsByCategory) {
      // Compare PER CATEGORY (One/Other), not by source order — a reordered arm
      // list must not misalign the comparison.
      for (const cat of ['One', 'Other']) {
        if (!slotsEqual(frEntry.slotsByCategory[cat], enEntry.slotsByCategory[cat])) {
          findings.push(finding(fr.file, frEntry.line, 'placeholder-parity', `${key}: fr ${cat} placeholders differ from en — a translation dropped, renamed, or duplicated a format slot`, 'catalog'));
        }
      }
    } else if (!slotsEqual(frEntry.slots, enEntry.slots)) {
      findings.push(finding(fr.file, frEntry.line, 'placeholder-parity', `${key}: fr placeholders differ from en — a translation dropped, renamed, or duplicated a format slot`, 'catalog'));
    }
  }

  // Rule 3 — a French value left in English.
  for (const [key, frEntry] of fr.entries) {
    const enEntry = en.entries.get(key);
    if (!enEntry) continue;
    if (frEntry.values.join(SLOT) !== enEntry.values.join(SLOT)) continue;
    if (identityExemption(key, frEntry.values.join(' '), allowlist)) continue;
    findings.push(finding(fr.file, frEntry.line, 'fr-untranslated', `${key}: French value is byte-identical to English — translate it, or add it to IDENTICAL_ALLOWLIST with the reason it is right`, 'catalog'));
  }
  // Rule 3, stale side — an exemption that no longer exempts anything.
  for (const key of Object.keys(allowlist)) {
    const frEntry = fr.entries.get(key);
    const enEntry = en.entries.get(key);
    if (!frEntry || !enEntry) {
      findings.push(finding(fr.file, 1, 'allowlist-stale', `IDENTICAL_ALLOWLIST names ${key}, which is not in both catalogs`, 'catalog'));
      continue;
    }
    if (frEntry.values.join(SLOT) !== enEntry.values.join(SLOT)) {
      findings.push(finding(fr.file, frEntry.line, 'allowlist-stale', `IDENTICAL_ALLOWLIST exempts ${key}, but the values now differ — drop the exemption`, 'catalog'));
    }
  }

  // Rule 4 — French typography over every fr value (localeTag excepted).
  for (const entry of fr.entries.values()) {
    if (entry.key === 'locale_tag') continue;
    for (const value of entry.values) {
      for (const rule of TYPOGRAPHY) {
        const m = rule.pattern.exec(value);
        if (m) findings.push(finding(fr.file, entry.line, rule.code, `${entry.key}: ${rule.message(m)}`, 'typography'));
      }
    }
  }

  return findings;
}

// ---------------------------------------------------------------------------
// Literal scan — user-visible copy outside the catalog
// ---------------------------------------------------------------------------

/** Copy-bearing RSX attributes / props. A literal assigned to one of these is
 *  user-visible copy that belongs in the catalog. Structural attributes
 *  (`class`, `id`, `href`, `role`, `tabindex`, `aria-live`, `aria-hidden`, …)
 *  are not listed and so are never flagged.
 *
 *  The list covers BOTH HTML attributes and this crate's semantic-primitive
 *  component PROPS: `label` (SkipLink/NavLandmark/StatusIndicator),
 *  `close_label` (Dialog), and `target` (BootScreen's already-localized status
 *  line). A hardcoded literal on any of these is the exact catalog bypass the
 *  gate exists to prevent — e.g. `SkipLink { label: "Skip to rooms" }` — so it
 *  must be caught, not just HTML `alt`/`aria-label`. */
const COPY_ATTRS = new Set([
  'alt',
  'aria-description',
  'aria-label',
  'aria-placeholder',
  'aria-roledescription',
  'aria-valuetext',
  'close_label',
  'hint',
  'label',
  'optional_label',
  'placeholder',
  'summary',
  'target',
  'title',
]);

/** Reserved semantic RSX attributes that must be provided by a shared primitive
 *  (Decision-6), never re-declared as raw markup — a raw `role: "dialog"` /
 *  `aria-live` region skips the primitive's focus/announce behaviour. Flagged
 *  everywhere EXCEPT the primitive files that legitimately define them. */
const RESERVED_SEMANTIC_ATTRS = new Set(['role', 'aria-modal', 'aria-live']);

/** Reserved semantic ELEMENT names that must come from a shared primitive
 *  (Decision-6). A bare `dialog { … }` outside a primitive skips the Dialog
 *  primitive's focus containment / Escape handling — there is no legitimate bare
 *  use, so it is always flagged. `nav` is handled separately below (flagged only
 *  when UNNAMED — a named landmark IS the contract, so the app shell's named nav
 *  is legitimate). Matched lowercase, so the `Dialog` primitive COMPONENT (capital
 *  D) is never caught. */
const RESERVED_SEMANTIC_ELEMENTS = new Set(['dialog']);

/** Raw form CONTROLS that must be wrapped by the `Field` primitive (§5.6), which
 *  supplies the `label[for]`/`id` association and `aria-describedby` hint linkage.
 *  A bare `input`/`textarea`/`select` skips that accessible-name path
 *  (Decision-6). `Field` takes the control as `children`, so a control is
 *  legitimate ONLY inside a `Field { … }` invocation; anywhere else it is flagged.
 *  The foundation ships no form yet, so this currently guards the first one. */
const RESERVED_FORM_CONTROLS = new Set(['input', 'textarea', 'select']);

/** Whether two placeholder-slot arrays are equal element-by-element. Compared
 *  positionally, NOT by a delimiter-free join: `['a','bc']` and `['ab','c']`
 *  both join to `'abc'`, so a join would call two distinct multisets equal and
 *  miss a dropped/renamed slot (Rust lets an impl's parameter names differ from
 *  the trait, so such a mismatch compiles). */
export function slotsEqual(a, b) {
  return a.length === b.length && a.every((slot, i) => slot === b[i]);
}

/** The reserved constructs each PRIMITIVE source file legitimately DEFINES, keyed
 *  by an exact repo path SUFFIX (not a basename). Scoping matters two ways
 *  (Decision-6): a future `components/legacy/dialog.rs` must NOT be exempted just
 *  for sharing the basename `dialog.rs`, and a primitive must be exempt ONLY for
 *  the constructs it owns — an ad-hoc `aria-live` added to `status.rs` must still
 *  be flagged. Values name the reserved attributes and/or the `nav` element token
 *  a file owns; the `dialog` ELEMENT is owned by NO file (the Dialog primitive
 *  renders `div role="dialog"`, not a `<dialog>`), so it is always flagged. */
const PRIMITIVE_OWNERSHIP = new Map([
  ['components/dialog.rs', new Set(['role', 'aria-modal'])],
  ['components/live_region.rs', new Set(['role', 'aria-live'])],
  ['components/nav.rs', new Set(['nav'])],
]);

const NO_OWNED_CONSTRUCTS = new Set();

/** The reserved constructs `file` may define, by exact path-suffix match (so
 *  `.../components/dialog.rs` matches but `.../components/legacy/dialog.rs` and a
 *  same-basename file elsewhere do not). Empty for non-primitive files. */
function ownedConstructs(file) {
  const normalized = file.replace(/\\/g, '/');
  for (const [suffix, owned] of PRIMITIVE_OWNERSHIP) {
    if (normalized === suffix || normalized.endsWith(`/${suffix}`)) return owned;
  }
  return NO_OWNED_CONSTRUCTS;
}

/** Letters that survive `{…}` interpolation are real copy. */
function bareLetters(text) {
  return /[A-Za-z]{2,}/.test(text.replace(/\{[^}]*\}/g, ' '));
}

/** The identifier or quoted attribute name immediately before the `:` at
 *  `colonIndex`, or null. Reads the NAME from the raw `source`: a quoted
 *  attribute name (`"aria-label":`) has its content blanked in the skeleton, so
 *  only the source carries the actual name. */
function attrNameBefore(source, colonIndex) {
  let at = colonIndex - 1;
  while (at >= 0 && /\s/.test(source[at])) at -= 1;
  if (at < 0) return null;
  if (source[at] === '"') {
    const close = at;
    let start = at - 1;
    while (start >= 0 && source[start] !== '"') start -= 1;
    return source.slice(start + 1, close);
  }
  const end = at + 1;
  while (at >= 0 && /[\w-]/.test(source[at])) at -= 1;
  return source.slice(at + 1, end);
}

/** The byte ranges covered by `rsx! { … }` blocks (nested rsx sits inside the
 *  outer range). Only literals inside these are RSX markup; a string literal in
 *  ordinary Rust code — a `match` arm returning a class name, a `let` binding, a
 *  `format!` argument — is not copy and must not be scanned. */
function rsxRanges(skeleton) {
  const ranges = [];
  const re = /\brsx\s*!\s*[({[]/g;
  let match;
  while ((match = re.exec(skeleton)) !== null) {
    const openChar = skeleton[match.index + match[0].length - 1];
    const closeChar = { '{': '}', '(': ')', '[': ']' }[openChar];
    let depth = 0;
    let end = -1;
    for (let at = match.index + match[0].length - 1; at < skeleton.length; at += 1) {
      if (skeleton[at] === openChar) depth += 1;
      else if (skeleton[at] === closeChar) {
        depth -= 1;
        if (depth === 0) {
          end = at;
          break;
        }
      }
    }
    if (end !== -1) {
      ranges.push([match.index, end + 1]);
      re.lastIndex = end;
    }
  }
  return ranges;
}

/** From a method-chain `.` at `dotStart`, walk `.ident(...)` / `.ident()` /
 *  `.ident` segments (with balanced call parens) and report whether the chain is
 *  immediately followed by the `}` that closes its RSX expression slot — i.e. the
 *  braces wrap ONLY a converted string literal, as an expression text child does.
 *  Distinguishes `{ "x".to_string() }` (a copy child → true) from a statement
 *  block like `{ "x".to_string(); … }` (→ false, the next token is `;`). */
function methodChainClosesSlot(skeleton, dotStart) {
  let i = dotStart;
  while (i < skeleton.length && skeleton[i] === '.') {
    i += 1; // past '.'
    while (i < skeleton.length && /\w/.test(skeleton[i])) i += 1; // method ident
    while (i < skeleton.length && /\s/.test(skeleton[i])) i += 1;
    if (skeleton[i] === '(') {
      let depth = 0;
      for (; i < skeleton.length; i += 1) {
        if (skeleton[i] === '(') depth += 1;
        else if (skeleton[i] === ')') {
          depth -= 1;
          if (depth === 0) {
            i += 1;
            break;
          }
        }
      }
    }
    while (i < skeleton.length && /\s/.test(skeleton[i])) i += 1;
  }
  return skeleton[i] === '}';
}

/** Whether the call whose `(` is at `openParenIndex` is itself an RSX EXPRESSION
 *  CHILD — `div { {format!("Delete account")} }` — rather than a prop/attr value
 *  or a nested call argument. Walks back over the callee (ident / path / macro
 *  `!`) to require an opening `{` immediately before it, and forward to require
 *  the call's matching `)` to be immediately followed by that slot's `}`. So
 *  `{ format!("…") }` (a visible text child, copy) is caught, while
 *  `class: format!("app-{}", p)` (an attr value, callee preceded by `:`) and
 *  `foo(bar("…"))` (a nested arg, callee preceded by `(`) are not. */
function callIsExpressionChild(skeleton, openParenIndex) {
  let i = openParenIndex - 1;
  while (i >= 0 && /[\w:!]/.test(skeleton[i])) i -= 1; // callee ident / path / `!`
  while (i >= 0 && /\s/.test(skeleton[i])) i -= 1;
  if (skeleton[i] !== '{') return false; // not the opener of an expression slot
  // The call must BE the whole slot: its matching `)` is followed only by `}`.
  let depth = 0;
  let j = openParenIndex;
  for (; j < skeleton.length; j += 1) {
    if (skeleton[j] === '(') depth += 1;
    else if (skeleton[j] === ')') {
      depth -= 1;
      if (depth === 0) {
        j += 1;
        break;
      }
    }
  }
  while (j < skeleton.length && /\s/.test(skeleton[j])) j += 1;
  return skeleton[j] === '}';
}

/** The name bound at the `=` at `eqIndex`, if it is a `let [mut] <name> = …`,
 *  `let [mut] <name>: <TYPE> = …`, `const <NAME>: <TYPE> = …`, or
 *  `static <NAME>: <TYPE> = …` declaration; else null. All are common ways to hold
 *  copy later interpolated into RSX — including a typed `let`, whose annotation
 *  would otherwise hide the name from the walk-back. The name is an identifier, so
 *  it is not blanked in the skeleton. */
function letBindingName(skeleton, eqIndex) {
  let i = eqIndex - 1;
  while (i >= 0 && /\s/.test(skeleton[i])) i -= 1;
  const nameEnd = i + 1;
  while (i >= 0 && /\w/.test(skeleton[i])) i -= 1;
  const name = skeleton.slice(i + 1, nameEnd);
  if (name) {
    let j = i;
    while (j >= 0 && /\s/.test(skeleton[j])) j -= 1;
    const keywordEndsAt = (kw, at) => {
      const start = at - kw.length + 1;
      return (
        start >= 0 &&
        skeleton.slice(start, at + 1) === kw &&
        (start === 0 || !/\w/.test(skeleton[start - 1]))
      );
    };
    if (keywordEndsAt('mut', j)) {
      j -= 3;
      while (j >= 0 && /\s/.test(skeleton[j])) j -= 1;
    }
    if (keywordEndsAt('let', j)) return name;
  }
  // TYPED bindings — `let [mut] NAME: TYPE =`, `const NAME: TYPE =`, or
  // `static NAME: TYPE =`. The type annotation sits between the name and `=`, so
  // the plain-word walk-back above lands on the TYPE, not the name (and, for a
  // typed `let`, never reaches the `let` keyword), missing the binding. Match the
  // keyword + name + `: TYPE` directly instead.
  const decl = /\b(?:let(?:\s+mut)?|const|static)\s+([A-Za-z_]\w*)\s*:\s*[^=;{}]*$/.exec(
    skeleton.slice(0, eqIndex),
  );
  return decl ? decl[1] : null;
}

/** The index of the `}` matching the `{` at `openIndex` in `skeleton`, or -1. */
function matchingBrace(skeleton, openIndex) {
  let depth = 0;
  for (let i = openIndex; i < skeleton.length; i += 1) {
    if (skeleton[i] === '{') depth += 1;
    else if (skeleton[i] === '}') {
      depth -= 1;
      if (depth === 0) return i;
    }
  }
  return -1;
}

/** Report user-visible literals in one Rust component/app source. */
export function scanComponentLiterals(file, source) {
  const { skeleton, literals, comments } = scanRustSource(source);
  const lines = source.split('\n');
  // Test modules are not shipped copy; skip everything from the first
  // `#[cfg(test)]` onward.
  const testAt = skeleton.indexOf('#[cfg(test)]');
  const limit = testAt === -1 ? source.length : testAt;
  const ranges = rsxRanges(skeleton);
  const inRsx = (pos) => ranges.some(([start, end]) => pos >= start && pos < end);
  // A position inside a `//`/`/* */` comment, per the Rust scanner. The reserved
  // attribute scan matches the RAW source (to catch a quoted `"aria-live"`, whose
  // content is blanked in the skeleton), so it must exclude comments itself — a
  // comment documenting `role:`/`aria-live:` renders no attribute.
  const inComment = (pos) => comments.some(({ start, end }) => pos >= start && pos < end);
  const exempt = (line) =>
    (lines[line - 1] ?? '').includes('i18n-exempt') || (lines[line - 2] ?? '').includes('i18n-exempt');
  const findings = [];

  // Identifiers interpolated into an RSX COPY position — a text child
  // (`div { "{label}" }`) or a copy-bearing attribute value
  // (`SkipLink { label: "{x}" }`). A `let`-bound literal that flows into one of
  // these is copy (checked after the main loop). Structural interpolations
  // (`id: "{x}"`, `class: "app-{x}"`) are deliberately NOT collected, so binding a
  // structural id/class stays exempt.
  const copyInterpolations = new Set();
  for (const literal of literals) {
    if (literal.start >= limit || !inRsx(literal.start)) continue;
    let at = literal.start - 1;
    while (at >= 0 && /\s/.test(skeleton[at])) at -= 1;
    const prevChar = at >= 0 ? skeleton[at] : '';
    let after = literal.end;
    while (after < skeleton.length && /\s/.test(skeleton[after])) after += 1;
    let isCopy;
    if (prevChar === ':') {
      const attr = attrNameBefore(source, at);
      isCopy = attr !== null && COPY_ATTRS.has(attr);
    } else {
      // A text child: not an attr value, not a call arg / binding, not a method
      // receiver (`"x".to_string()`).
      isCopy = prevChar !== '(' && prevChar !== '=' && prevChar !== '&' && skeleton[after] !== '.';
    }
    if (!isCopy) continue;
    for (const m of literal.value.matchAll(/\{(\w+)\}/g)) copyInterpolations.add(m[1]);
  }

  for (const literal of literals) {
    if (literal.start >= limit) continue;
    // Only literals inside RSX markup are copy candidates; Rust logic literals
    // (class-name match arms, `let` bindings, `format!` args) are not.
    if (!inRsx(literal.start)) continue;
    if (!bareLetters(literal.value)) continue;
    const line = lineOf(source, literal.start);
    if (exempt(line)) continue;

    // The first non-space char AFTER the literal: a `:` means the literal is an
    // attribute NAME (`"aria-label": …`), not a value — its value is checked
    // separately, so skip the name itself.
    let after = literal.end;
    while (after < skeleton.length && /\s/.test(skeleton[after])) after += 1;
    if (skeleton[after] === ':') continue;

    // What precedes the opening quote (skeleton, so string contents can't fool
    // the look-behind)?
    let before = literal.start - 1;
    while (before >= 0 && /\s/.test(skeleton[before])) before -= 1;
    const prev = before >= 0 ? skeleton[before] : '';

    // A copy-bearing prop/attr VALUE (`label: "…"`) is checked BEFORE the
    // method-receiver skip below: `label: "Skip to rooms".to_string()` is the
    // normal spelling for a `String` prop, and a trailing `.to_string()`/`.into()`
    // must NOT exempt it — that was the catalog bypass.
    if (prev === ':') {
      const attr = attrNameBefore(source, before);
      if (attr && COPY_ATTRS.has(attr)) {
        findings.push(finding(file, line, 'copy-attribute', `${attr} takes a literal, not a catalog message: ${literal.value.slice(0, 60)}`, 'literals'));
      }
      continue; // structural attribute or a non-copy prop
    }
    // A copy-prop value WRAPPED in a constructor — `hint: Some("…".to_string())`,
    // `label: String::from("…")`, `Cow::Borrowed("…")` — reaches here with
    // `prev === '('`. Walk back over the wrapper identifier and, if it is the
    // value of a copy-bearing prop, flag it: the wrapper is the normal spelling
    // for an `Option<String>`/`String` prop and must not exempt the copy.
    if (prev === '(') {
      let ident = before - 1;
      while (ident >= 0 && /[A-Za-z0-9_:]/.test(skeleton[ident])) ident -= 1;
      let colon = ident;
      while (colon >= 0 && /\s/.test(skeleton[colon])) colon -= 1;
      if (skeleton[colon] === ':') {
        const attr = attrNameBefore(source, colon);
        if (attr && COPY_ATTRS.has(attr)) {
          findings.push(finding(file, line, 'copy-attribute', `${attr} takes a wrapped literal, not a catalog message: ${literal.value.slice(0, 60)}`, 'literals'));
        }
        continue;
      }
    }
    // A method-receiver literal that is itself an RSX EXPRESSION CHILD — e.g.
    // `div { {"Delete account".to_string()} }` — is user-visible copy rendered as
    // a text node, NOT Rust logic. The opening `{` of the expression slot precedes
    // it (prev === '{') and the whole conversion chain closes that slot, so a
    // trailing `.to_string()`/`.into()` must not exempt it — that was the bypass a
    // bare `{ "…" }` child would otherwise be caught by. Classify it as copy.
    if (skeleton[after] === '.' && prev === '{' && methodChainClosesSlot(skeleton, after)) {
      findings.push(finding(file, line, 'rust-text', `RSX text expression is not in the catalog: ${literal.value.trim().slice(0, 60)}`, 'literals'));
      continue;
    }
    // A literal that is a method receiver (`"x".to_string()`) reached HERE — not
    // a copy-prop value (handled above) — is Rust logic embedded in RSX, not
    // markup text.
    if (skeleton[after] === '.') continue;
    // A literal that is the argument of a call which IS the RSX expression child —
    // `div { {format!("Delete account")} }` — is visible copy (a `format!` string
    // is a normal way to render dynamic text), not Rust logic. Detect that the
    // enclosing call is the whole `{ … }` slot before treating the argument as
    // code; a `class: format!("app-{}", p)` attr value or a nested `foo(bar("…"))`
    // arg is not, and stays exempt.
    if (prev === '(' && callIsExpressionChild(skeleton, before)) {
      findings.push(finding(file, line, 'rust-text', `RSX text expression is not in the catalog: ${literal.value.trim().slice(0, 60)}`, 'literals'));
      continue;
    }
    // A literal that is a function/macro argument is likewise Rust code.
    if (prev === '(' || prev === '=' || prev === '&') continue;

    findings.push(finding(file, line, 'rust-text', `RSX text is not in the catalog: ${literal.value.trim().slice(0, 60)}`, 'literals'));
  }

  // A hardcoded literal ASSIGNED to a `let` binding that is then interpolated into
  // RSX copy (`let label = "Delete account"; div { "{label}" }`) lives OUTSIDE the
  // rsx! range, so the range-only scan above misses it. Flag such a binding when
  // its name is interpolated in a copy position. A catalog-derived binding
  // (`let x = strings.foo()`) has no string-literal RHS, so it is never matched.
  for (const literal of literals) {
    if (literal.start >= limit || inRsx(literal.start)) continue; // in-RSX handled above
    if (!bareLetters(literal.value)) continue;
    let at = literal.start - 1;
    while (at >= 0 && /\s/.test(skeleton[at])) at -= 1;
    const prevChar = at >= 0 ? skeleton[at] : '';
    // The literal is a `let` binding's RHS either DIRECTLY (`let x = "…"`) or
    // WRAPPED in a constructor (`let x = String::from("…")` / `format!("…")`) —
    // both create the `String` later interpolated as copy. For the wrapped case,
    // walk back over the callee to the `=`.
    let eqIndex = -1;
    if (prevChar === '=') {
      eqIndex = at;
    } else if (prevChar === '(') {
      let i = at - 1;
      while (i >= 0 && /[\w:!]/.test(skeleton[i])) i -= 1; // callee ident / path / `!`
      while (i >= 0 && /\s/.test(skeleton[i])) i -= 1;
      if (skeleton[i] === '=') eqIndex = i;
    }
    // The literal may sit ANYWHERE in a `let NAME = <RHS>` — inside an `if/else`,
    // a `match` arm, or a block — where the char before it is `{`/`>`/etc., not
    // `=`/`(` (e.g. a conditional `let label = if c { "Delete" } else { "Remove" }`).
    // Walk back to the enclosing STATEMENT (nearest `;`) and, if the innermost
    // `let NAME =` opens before the literal within it, associate the literal with
    // NAME so conditionally-assigned copy cannot bypass the check.
    if (eqIndex < 0) {
      let stmtStart = literal.start - 1;
      while (stmtStart >= 0 && skeleton[stmtStart] !== ';') stmtStart -= 1;
      stmtStart += 1;
      const stmt = skeleton.slice(stmtStart, literal.start);
      const letRe = /\blet\s+(?:mut\s+)?[A-Za-z_]\w*\s*(?::\s*[^=;{}]*)?=/g;
      let lastLet = null;
      for (let mm = letRe.exec(stmt); mm; mm = letRe.exec(stmt)) lastLet = mm;
      if (lastLet) eqIndex = stmtStart + lastLet.index + lastLet[0].length - 1;
    }
    if (eqIndex < 0) continue;
    const boundName = letBindingName(skeleton, eqIndex);
    if (!boundName || !copyInterpolations.has(boundName)) continue;
    const line = lineOf(source, literal.start);
    if (exempt(line)) continue;
    findings.push(finding(file, line, 'rust-text', `a literal bound to \`${boundName}\` is rendered as RSX copy but is not in the catalog: ${literal.value.trim().slice(0, 60)}`, 'literals'));
  }

  // Reserved-semantic scan (Decision-6): raw semantics must come from a shared
  // primitive, and a primitive is exempt ONLY for the exact constructs it owns
  // (an ad-hoc `aria-live` in `status.rs` is still flagged; a same-basename file
  // in another directory is NOT exempt).
  const owned = ownedConstructs(file);

  // Reserved ATTRIBUTES (`role`/`aria-modal`/`aria-live`): flag unless THIS file
  // owns that specific attribute. Names matched from the raw source (a quoted
  // `"aria-live"` has its content blanked in the skeleton).
  for (const attr of RESERVED_SEMANTIC_ATTRS) {
    if (owned.has(attr)) continue;
    const re = new RegExp(`(?:\\b${attr}\\b|"${attr}")\\s*:`, 'g');
    for (let m = re.exec(source); m; m = re.exec(source)) {
      if (m.index >= limit || !inRsx(m.index) || inComment(m.index)) continue;
      const line = lineOf(source, m.index);
      if (exempt(line)) continue;
      findings.push(finding(file, line, 'raw-semantic', `raw \`${attr}\` must come from a shared primitive (Decision-6), not ad-hoc markup`, 'literals'));
    }
  }

  // Reserved semantic ELEMENTS: a bare `dialog { … }` sets dialog semantics
  // implicitly. NO primitive owns the `dialog` element (Dialog renders `div
  // role="dialog"`), so it is flagged EVERYWHERE. Match element openers in the
  // SKELETON so a `dialog {` inside a blanked string never matches.
  for (const el of RESERVED_SEMANTIC_ELEMENTS) {
    const re = new RegExp(`\\b${el}\\s*\\{`, 'g');
    for (let m = re.exec(skeleton); m; m = re.exec(skeleton)) {
      if (m.index >= limit || !inRsx(m.index)) continue;
      const line = lineOf(source, m.index);
      if (exempt(line)) continue;
      findings.push(finding(file, line, 'raw-semantic-element', `raw \`${el}\` element must come from a shared primitive (Decision-6), not ad-hoc markup`, 'literals'));
    }
  }

  // Reserved FORM CONTROLS (`input`/`textarea`/`select`): each must be wrapped by
  // the `Field` primitive, which owns the label association. `Field` renders the
  // control as `children`, so a control is legitimate ONLY inside a `Field { … }`
  // invocation; flag one anywhere else. Compute the `Field` invocation ranges from
  // the SKELETON (a `Field {` in a blanked string/comment never counts), then a
  // control opener outside every such range is raw.
  const fieldRanges = [];
  {
    const re = /\bField\s*\{/g;
    for (let m = re.exec(skeleton); m; m = re.exec(skeleton)) {
      const open = m.index + m[0].length - 1;
      const close = matchingBrace(skeleton, open);
      if (close !== -1) fieldRanges.push([open, close]);
    }
  }
  // Extract the FIRST `id:` attribute value in `source[from..to)`, or null. The
  // value is either a quoted literal (`"email"`) OR an EXPRESSION
  // (`field_id.clone()`, up to the next `,`/`}`) — an expression-valued id must not
  // slip the mismatch check, so capture both forms and compare them as raw text
  // (a literal keeps its quotes, so a literal id and an expression id never
  // spuriously match). Used for both the Field's own id and the control's id.
  const firstIdAttr = (from, to) => {
    const match = /\bid\s*:\s*("[^"]*"|[^,}]+)/.exec(source.slice(from, to));
    return match ? match[1].trim() : null;
  };
  for (const el of RESERVED_FORM_CONTROLS) {
    const re = new RegExp(`\\b${el}\\s*\\{`, 'g');
    for (let m = re.exec(skeleton); m; m = re.exec(skeleton)) {
      if (m.index >= limit || !inRsx(m.index)) continue;
      const line = lineOf(source, m.index);
      if (exempt(line)) continue;
      const field = fieldRanges.find(([open, close]) => m.index > open && m.index < close);
      if (!field) {
        // Not inside any `Field` → a raw, unlabelled control.
        findings.push(finding(file, line, 'raw-form-control', `raw \`${el}\` must be wrapped by the \`Field\` primitive (§5.6) for label association, not rendered ad-hoc (Decision-6)`, 'literals'));
        continue;
      }
      // Inside a `Field`, but nesting alone is not enough: the Field renders
      // `label[for="{id}"]`, so the CONTROL must set the SAME `id` or the label
      // names nothing. Compare the Field's own `id` (its props precede the child
      // control) with the control's `id`.
      const controlOpen = m.index + m[0].length - 1;
      const controlClose = matchingBrace(skeleton, controlOpen);
      const controlEnd = controlClose === -1 ? source.length : controlClose;
      const fieldId = firstIdAttr(field[0], controlOpen);
      const controlId = firstIdAttr(controlOpen, controlEnd);
      if (fieldId !== null && controlId !== fieldId) {
        findings.push(finding(file, line, 'form-control-id-mismatch', `\`${el}\` inside \`Field\` must set \`id\` to match the Field's \`id\` (\`${fieldId}\`) so its \`label[for]\` names it; found \`${controlId === null ? '(no id)' : controlId}\``, 'literals'));
      }
    }
  }

  // `nav` must be a NAMED landmark, and only the NavLandmark primitive (which
  // OWNS `nav`) may render a bare one. Elsewhere, flag a `nav { … }` whose body
  // carries no accessible name (`aria-label`/`aria-labelledby`) — the app shell's
  // named nav is legitimate, an unnamed one bypasses the named-navigation contract.
  if (!owned.has('nav')) {
    const navRe = /\bnav\s*\{/g;
    for (let m = navRe.exec(skeleton); m; m = navRe.exec(skeleton)) {
      if (m.index >= limit || !inRsx(m.index)) continue;
      const openBrace = m.index + m[0].length - 1;
      const close = matchingBrace(skeleton, openBrace);
      // Inspect ONLY the nav's OWN attribute list, not its subtree: a descendant's
      // `aria-label` (e.g. an icon button) does not name the nav landmark.
      // Attributes precede the first nested `{` (a child element / expression
      // body); interpolation braces inside string VALUES are blanked in the
      // skeleton, so the first skeleton `{` is a real child opener. Read the raw
      // SOURCE over that span (a quoted `"aria-label"` name is blanked in skeleton).
      let attrEnd = close === -1 ? skeleton.length : close;
      for (let j = openBrace + 1; j < attrEnd; j += 1) {
        if (skeleton[j] === '{') {
          attrEnd = j;
          break;
        }
      }
      // Read the raw SOURCE over the attr span, then BLANK any comment ranges
      // inside it: a comment like `// aria-label: supplied later` must NOT be read
      // as a real accessible name (the reserved-attribute path already excludes
      // comments; the nav path must too).
      // Read the raw SOURCE over the attr span, then BLANK any comment ranges
      // inside it: a comment like `// aria-label: supplied later` must NOT be read
      // as a real accessible name (the reserved-attribute path already excludes
      // comments; the nav path must too).
      const attrsStart = openBrace + 1;
      let attrs = source.slice(attrsStart, attrEnd);
      for (const { start, end } of comments) {
        if (end <= attrsStart || start >= attrEnd) continue;
        const from = Math.max(start, attrsStart) - attrsStart;
        const to = Math.min(end, attrEnd) - attrsStart;
        attrs = attrs.slice(0, from) + ' '.repeat(to - from) + attrs.slice(to);
      }
      if (/aria-label\b|aria-labelledby\b/.test(attrs)) continue;
      const line = lineOf(source, m.index);
      if (exempt(line)) continue;
      findings.push(finding(file, line, 'raw-semantic-element', 'raw unnamed `nav` must carry an accessible name (aria-label/aria-labelledby) or come from the NavLandmark primitive (Decision-6)', 'literals'));
    }
  }
  return findings;
}

function componentFiles(repoRoot) {
  const files = [];
  const walk = (absolute) => {
    for (const entry of readdirSync(absolute, { withFileTypes: true }).sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))) {
      const path = resolve(absolute, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (/\.rs$/.test(entry.name)) files.push(path);
    }
  };
  for (const root of LITERAL_SCAN_ROOTS) {
    const absolute = resolve(repoRoot, root);
    if (!existsSync(absolute)) continue;
    if (/\.rs$/.test(root)) files.push(absolute);
    else walk(absolute);
  }
  return files;
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

function compareFindings(a, b) {
  const text = (x, y) => (x < y ? -1 : x > y ? 1 : 0);
  return text(a.file, b.file) || a.line - b.line || text(a.code, b.code) || text(a.message, b.message);
}

function toRepoPath(repoRoot, path) {
  return relative(repoRoot, path).split(sep).join('/');
}

/** Run the selected rule groups against a repository tree and return sorted
 *  findings. `only` is a subset of {'catalog','typography','literals'}; default
 *  runs all three. `allowlist` is a parameter so the companion test can drive
 *  the rules with fixture exemptions. */
export function checkJeliyaUiI18n({
  repoRoot = DEFAULT_REPO_ROOT,
  only = ['catalog', 'typography', 'literals'],
  allowlist = IDENTICAL_ALLOWLIST,
} = {}) {
  const root = resolve(repoRoot);
  const groups = new Set(only);
  const findings = [];
  const read = (relativePath) => {
    const absolute = resolve(root, relativePath);
    return existsSync(absolute) ? readFileSync(absolute, 'utf8') : null;
  };

  if (groups.has('catalog') || groups.has('typography')) {
    const parsed = {};
    for (const [tag, path] of Object.entries(LOCALE_FILES)) {
      const source = read(path);
      if (source === null) {
        findings.push(finding(path, 1, 'catalog-missing', `the ${tag} catalog is missing — every locale ships complete`, 'catalog'));
        continue;
      }
      const { entries, errors } = parseCatalog(source, path);
      findings.push(...errors);
      parsed[tag] = { file: path, entries };
    }
    if (parsed.en && parsed.fr) {
      findings.push(...checkCatalogs({ en: parsed.en, fr: parsed.fr, allowlist }));
    }
  }

  if (groups.has('literals')) {
    for (const absolute of componentFiles(root)) {
      const file = toRepoPath(root, absolute);
      findings.push(...scanComponentLiterals(file, readFileSync(absolute, 'utf8')));
    }
  }

  return findings.filter((f) => groups.has(f.group)).sort(compareFindings);
}
