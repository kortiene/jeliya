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
  const blank = (from, to) => {
    for (let i = from; i < to; i += 1) if (masked[i] !== '\n') masked[i] = ' ';
  };

  let i = 0;
  while (i < source.length) {
    const ch = source[i];
    if (ch === '/' && source[i + 1] === '/') {
      const end = source.indexOf('\n', i);
      const stop = end === -1 ? source.length : end;
      blank(i, stop);
      i = stop;
      continue;
    }
    if (ch === '/' && source[i + 1] === '*') {
      const end = source.indexOf('*/', i + 2);
      const stop = end === -1 ? source.length : end + 2;
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
  return { skeleton: skeleton.join(''), literals };
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
    entries.set(name, {
      key: name,
      line: lineOf(source, match.index),
      isPlural,
      values: parts.map((p) => collapseSlots(p.value)),
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

  // Rule 2 — no empty value, in either catalog.
  for (const locale of [en, fr]) {
    for (const entry of locale.entries.values()) {
      const empty = entry.values.length === 0 || entry.values.every((v) => v.replace(new RegExp(SLOT, 'g'), '').trim() === '');
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
  'label',
  'placeholder',
  'summary',
  'target',
  'title',
]);

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

/** Report user-visible literals in one Rust component/app source. */
export function scanComponentLiterals(file, source) {
  const { skeleton, literals } = scanRustSource(source);
  const lines = source.split('\n');
  // Test modules are not shipped copy; skip everything from the first
  // `#[cfg(test)]` onward.
  const testAt = skeleton.indexOf('#[cfg(test)]');
  const limit = testAt === -1 ? source.length : testAt;
  const ranges = rsxRanges(skeleton);
  const inRsx = (pos) => ranges.some(([start, end]) => pos >= start && pos < end);
  const exempt = (line) =>
    (lines[line - 1] ?? '').includes('i18n-exempt') || (lines[line - 2] ?? '').includes('i18n-exempt');
  const findings = [];

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
    if (skeleton[after] === '.') continue; // `"x".to_string()` etc.

    // What precedes the opening quote (skeleton, so string contents can't fool
    // the look-behind)?
    let before = literal.start - 1;
    while (before >= 0 && /\s/.test(skeleton[before])) before -= 1;
    const prev = before >= 0 ? skeleton[before] : '';

    if (prev === ':') {
      const attr = attrNameBefore(source, before);
      if (attr && COPY_ATTRS.has(attr)) {
        findings.push(finding(file, line, 'copy-attribute', `${attr} takes a literal, not a catalog message: ${literal.value.slice(0, 60)}`, 'literals'));
      }
      continue; // structural attribute or a non-copy prop
    }
    // A literal that is a function/macro argument or a method receiver is Rust
    // code embedded in RSX (e.g. an `if`-let guard), not markup text.
    if (prev === '(' || prev === '=' || prev === '&') continue;

    findings.push(finding(file, line, 'rust-text', `RSX text is not in the catalog: ${literal.value.trim().slice(0, 60)}`, 'literals'));
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
