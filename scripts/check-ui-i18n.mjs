#!/usr/bin/env node
// React localization completeness gate (issue #74).
//
// `scripts/i18n-gate.mjs` guards the Flutter catalog. This is its React
// counterpart, and it exists for the failure modes TYPES CANNOT SEE. Both
// `ui/src/l10n/en.ts` and `ui/src/l10n/fr.ts` are typed `Catalog`, so `tsc`
// already rejects a missing key. It cannot reject a key whose value is an
// empty string, and it cannot reject a French value that is still the English
// sentence — which is exactly what a translator hand-off produces when a key
// is added late, or when a merge takes the wrong side.
//
// Rules:
//  1. Key parity. Every `Catalog` interface member appears in every locale
//     catalog, and no locale carries a key the interface does not declare.
//     (Defence in depth behind `tsc`, and it also catches a catalog file that
//     stopped parsing as a catalog at all.)
//  2. No empty or whitespace-only value in any locale.
//  3. No French value byte-identical to its English counterpart, unless it is
//     legitimately identical. Two exemption paths, both stale-checked:
//       - AUTOMATIC, for values that carry no translatable words: a value with
//         no word in it (punctuation, a bare slot) and a value built only from
//         the Tier 2/Tier 3 never-translate lexicon (`docs/glossary-fr.md`) —
//         brand, ticket, agent, pipe, daemon, path badges, error codes.
//       - EXPLICIT, `IDENTICAL_ALLOWLIST` below: key -> reason. An entry whose
//         key is gone, or whose values are no longer identical, is reported.
//         A stale exemption hides the next real one.
//  4. French typography (`docs/glossary-fr.md`, decision 7). U+202F before
//     `; ! ? %` and inside guillemets, U+00A0 before `:`, U+2019 for the
//     apostrophe, U+2026 for the ellipsis, guillemets rather than double
//     quotes. This is the rule most likely to rot: every one of these is
//     invisible in review, and every one of them is wrong in a way a French
//     reader notices immediately.
//  5. User-visible string literals in `ui/src/App.tsx` and
//     `ui/src/components/**` that never reach the catalog — the React
//     equivalent of i18n-gate rule 1, using the same heuristics (comments
//     stripped string-aware, `i18n-exempt: <reason>` honored on the line or
//     the line above). `EXEMPT_FILES` records the modules that stay English
//     BY DECISION, with the reason, and reports an entry that no longer
//     exists.
//
// Parsing. This reads the catalogs as TEXT with a restricted TypeScript
// scanner rather than importing them, for the reason `scripts/check-docs.mjs`
// parses its own YAML subset: a gate that evaluates the thing it is gating is
// a gate that can be talked out of its own findings, and `scripts/` has no
// build step and no access to `ui/node_modules`. The scanner understands
// exactly what a catalog module may contain — comments, string literals,
// template literals with `${}` slots, and arrow functions.
//
// Run: node scripts/check-ui-i18n.mjs   (exit 1 on findings, 0 when clean)

import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const DEFAULT_REPO_ROOT = resolve(dirname(SCRIPT_PATH), '..');

/** Where the catalogs live, relative to the repository root. */
export const CATALOG_INTERFACE = 'ui/src/l10n/catalog.ts';
export const LOCALE_FILES = Object.freeze({
  en: 'ui/src/l10n/en.ts',
  fr: 'ui/src/l10n/fr.ts',
});

/** Files rule 5 scans. Everything else in `ui/src` is either not copy or is
 *  the l10n layer itself. */
const LITERAL_SCAN_ROOTS = Object.freeze(['ui/src/App.tsx', 'ui/src/components']);

/** Modules that hold English strings BY DECISION, with the decision. Rule 5
 *  never reports these, and reports an entry whose file has been deleted or
 *  moved — an exemption pointing at nothing is a note, not a rule.
 *
 *  These are listed even where they sit outside `LITERAL_SCAN_ROOTS` today:
 *  the reason is the durable part, and widening the scan must not silently
 *  sweep them in. */
export const EXEMPT_FILES = Object.freeze({
  'ui/src/lib/mock.ts':
    'fixture DATA, not copy — a demo room name is content, and translating it ' +
    'would make the fixtures assert their own translation.',
  'ui/src/lib/diagnostics.ts':
    'the support artifact stays English in its entirety (docs/glossary-fr.md, ' +
    'decision 1) — it is pasted into an issue read by maintainers, so a ' +
    'localized diagnostic is a bug report nobody can act on.',
  'ui/src/l10n/tokens.ts':
    'the never-translate module (docs/i18n.md rule 1) — glyphs, shell ' +
    'commands, wire-format examples, the brand, language endonyms.',
  'ui/src/lib/format.ts':
    'labelTone/prettyLabel are an English-token contract: they map wire ' +
    'values to tone classes and back, and the tokens are protocol, not copy. ' +
    'Display text for those values goes through the catalog display map ' +
    '(docs/i18n.md rule 3).',
});

/** Tier 2 and Tier 3 of `docs/glossary-fr.md`: words that are the SAME word in
 *  French, so a value built only from them is identical on purpose. Lowercased
 *  and accent-stripped before lookup. */
const NEVER_TRANSLATE = new Set([
  // Tier 3 — the brand.
  'jeliya',
  // Tier 2 — the daemon's own vocabulary and machine identifiers.
  'daemon',
  'jeliyad',
  'direct',
  'relay',
  'endpoint',
  'endpoints',
  'pipe',
  'pipes',
  'ticket',
  'tickets',
  'agent',
  'agents',
  'id',
  'ids',
  // Tier 2 — error codes, quoted verbatim so a user can search for them.
  'unavailable',
  'unauthorized',
  'hash',
  'mismatch',
  'hash_mismatch',
  // Acronyms and units that are not words in either language.
  'p2p',
  'qr',
  'url',
  'http',
  'https',
  'ok',
  'ui',
  'io',
  'ms',
  'kb',
  'mb',
  'gb',
]);

/** Keys whose French value is legitimately byte-identical to English for a
 *  reason the automatic lexicon above cannot express. Key -> reason.
 *
 *  A reason says why the FRENCH is right, never "not translated yet" — that is
 *  the finding, not the exemption. And an entry here is checked BOTH ways: if
 *  the key leaves the catalogs, or the two values stop being identical, the
 *  gate reports the exemption itself. A stale exemption is a claim nobody has
 *  re-read, and it hides the next real one. */
export const IDENTICAL_ALLOWLIST = Object.freeze({
  commonOptionalFieldLabel:
    'The template contains only two reorderable slots separated by a space; ' +
    'French uses the same layout while each slot supplies localized text.',
  addAgentWorkerLabel:
    '“Worker” names the runner’s configured execution backend and is kept as ' +
    'the same technical product term in the French Flutter catalog.',
  roomInfoSession:
    '“Session” is the standard French networking noun as well as the English ' +
    'one, so the room-information label is correctly identical.',
  settingsDiagnosticsTitle:
    '“Diagnostics” is the standard French plural for diagnostic information ' +
    'and is deliberately identical to the English heading.',
  timelineFileMeta:
    'This value is only translator-controlled punctuation around two data ' +
    'slots; French uses the same middle-dot layout.',
  timelineFilterConversation:
    '“Conversation” is the same noun in French and English and names the ' +
    'message filter accurately in both languages.',
  identityEndpointShort:
    '"ep" abbreviates the endpoint id, which docs/glossary-fr.md Tier 2 keeps ' +
    'verbatim — the prefix labels a machine identifier a user may need to ' +
    'match against daemon output, so a French "pt" would break the match.',
  modalRenameAliasLabel:
    '"Alias" is the French word too (Larousse), and the room roster shows it ' +
    'beside the identity id where a synonym would read as a second concept.',
  wireModeLoopback:
    '"loopback" is the daemon mode as the daemon reports it — ' +
    'docs/glossary-fr.md Tier 2 keeps wire words verbatim, because the badge ' +
    'is claiming a network fact and a translated one would no longer match ' +
    'what the operator sees in daemon output.',
});

/** Rule 5 started as a staged report while the component migration was in
 *  progress. It is blocking now that every React surface resolves copy through
 *  the catalog: a new hard-coded user-visible literal fails CI immediately. */
const LITERAL_SCAN_ENABLED = true;

/** `--report` runs the staged rule anyway and exits 0 regardless. It is how the
 *  migration's remaining surface stays countable while the rule is off — a
 *  number nobody can print is a number nobody tracks. */
const REPORT_ONLY = process.argv.includes('--report');

// ---------------------------------------------------------------------------
// A restricted TypeScript scanner
// ---------------------------------------------------------------------------

function unescapeJs(raw) {
  return raw.replace(
    /\\(u\{[0-9a-fA-F]+\}|u[0-9a-fA-F]{4}|x[0-9a-fA-F]{2}|[\s\S])/g,
    (_, escape) => {
      if (escape.startsWith('u{')) {
        return String.fromCodePoint(parseInt(escape.slice(2, -1), 16));
      }
      if (/^u[0-9a-fA-F]{4}$/.test(escape) || /^x[0-9a-fA-F]{2}$/.test(escape)) {
        return String.fromCodePoint(parseInt(escape.slice(1), 16));
      }
      return { n: '\n', t: '\t', r: '\r', b: '\b', f: '\f', v: '\v', '0': '\0' }[escape] ?? escape;
    },
  );
}

function scanQuoted(source, start, quote) {
  let index = start + 1;
  while (index < source.length) {
    const char = source[index];
    if (char === '\\') {
      index += 2;
      continue;
    }
    if (char === quote || char === '\n') break;
    index += 1;
  }
  return {
    start,
    end: Math.min(index + 1, source.length),
    contentStart: start + 1,
    contentEnd: index,
    value: unescapeJs(source.slice(start + 1, index)),
  };
}

function scanTemplate(source, start, literals) {
  let index = start + 1;
  let chunkStart = index;
  const pushChunk = (end) => {
    literals.push({
      start: chunkStart,
      end,
      contentStart: chunkStart,
      contentEnd: end,
      value: unescapeJs(source.slice(chunkStart, end)),
      template: true,
    });
  };
  while (index < source.length) {
    const char = source[index];
    if (char === '\\') {
      index += 2;
      continue;
    }
    if (char === '`') {
      pushChunk(index);
      return index + 1;
    }
    if (char === '$' && source[index + 1] === '{') {
      pushChunk(index);
      index = scanExpression(source, index + 2, literals);
      chunkStart = index;
      continue;
    }
    index += 1;
  }
  pushChunk(source.length);
  return source.length;
}

function scanExpression(source, start, literals) {
  let index = start;
  let depth = 1;
  while (index < source.length) {
    const char = source[index];
    if (char === "'" || char === '"') {
      const literal = scanQuoted(source, index, char);
      literals.push(literal);
      index = literal.end;
      continue;
    }
    if (char === '`') {
      index = scanTemplate(source, index, literals);
      continue;
    }
    if (char === '{') depth += 1;
    else if (char === '}') {
      depth -= 1;
      if (depth === 0) return index + 1;
    }
    index += 1;
  }
  return index;
}

/** Blank comments and literal CONTENTS (newlines preserved, so line numbers
 *  survive and so structural scanning cannot trip over a brace inside copy),
 *  and return every literal with its source position.
 *
 *  A template literal yields one entry per static chunk; the `${}` expressions
 *  between them are scanned as code, so a literal nested inside a slot is
 *  reported on its own rather than swallowed.
 *
 *  Known limit, and it fails SAFE: a straight apostrophe in JSX text
 *  (`<p>Don't</p>`) opens what looks like a string literal. The scan ends at
 *  the newline, so the blast radius is that one line, and the effect is a
 *  missed finding rather than a false one. The codebase writes `’` in copy
 *  anyway — which is the house style the French rules below also enforce. */
export function scanSource(source) {
  const masked = Array.from(source);
  const literals = [];
  const blank = (from, to) => {
    for (let index = from; index < to; index += 1) {
      if (masked[index] !== '\n') masked[index] = ' ';
    }
  };

  let index = 0;
  while (index < source.length) {
    const char = source[index];
    if (char === '/' && source[index + 1] === '/') {
      const end = source.indexOf('\n', index);
      const stop = end === -1 ? source.length : end;
      blank(index, stop);
      index = stop;
      continue;
    }
    if (char === '/' && source[index + 1] === '*') {
      const end = source.indexOf('*/', index + 2);
      const stop = end === -1 ? source.length : end + 2;
      blank(index, stop);
      index = stop;
      continue;
    }
    if (char === "'" || char === '"') {
      const literal = scanQuoted(source, index, char);
      literals.push(literal);
      index = literal.end;
      continue;
    }
    if (char === '`') {
      index = scanTemplate(source, index, literals);
      continue;
    }
    index += 1;
  }

  const code = masked.join('');
  const skeleton = Array.from(code);
  for (const literal of literals) {
    for (let at = literal.contentStart; at < literal.contentEnd; at += 1) {
      if (skeleton[at] !== '\n') skeleton[at] = ' ';
    }
  }
  return { code, skeleton: skeleton.join(''), literals };
}

/** Stands in for a `${}` slot when a message is compared or typography-checked
 *  as one sentence. Not a space: an adjacent slot must not look like the space
 *  the French contract is about, and it can never occur in copy. */
const SLOT = '\u0000';

function lineOf(source, index) {
  let line = 1;
  for (let at = 0; at < index && at < source.length; at += 1) {
    if (source.charCodeAt(at) === 10) line += 1;
  }
  return line;
}

function finding(file, line, code, message) {
  return { file, line, code, message };
}

// ---------------------------------------------------------------------------
// Catalog parsing
// ---------------------------------------------------------------------------

function matchingBrace(skeleton, open) {
  let depth = 0;
  for (let index = open; index < skeleton.length; index += 1) {
    const char = skeleton[index];
    if (char === '{') depth += 1;
    else if (char === '}') {
      depth -= 1;
      if (depth === 0) return index;
    }
  }
  return -1;
}

/** The member names of `export interface Catalog { ... }`, in source order. */
export function parseCatalogInterface(source, file = CATALOG_INTERFACE) {
  const { skeleton } = scanSource(source);
  const header = /export\s+interface\s+Catalog\s*\{/.exec(skeleton);
  if (!header) {
    return {
      keys: [],
      errors: [
        finding(file, 1, 'catalog-unparsed', 'no `export interface Catalog {` declaration found'),
      ],
    };
  }
  const open = header.index + header[0].length - 1;
  const close = matchingBrace(skeleton, open);
  if (close === -1) {
    return {
      keys: [],
      errors: [finding(file, lineOf(source, open), 'catalog-unparsed', 'interface body is unterminated')],
    };
  }
  const body = skeleton.slice(open + 1, close);
  const keys = [];
  // Members are `;`-terminated at depth 0. A `name:` inside a type argument
  // (`MessageFn<[room: string]>`) sits at depth > 0 and never terminates, so
  // the parameter labels a translator would otherwise see as keys are skipped.
  let depth = 0;
  let memberStart = 0;
  for (let index = 0; index < body.length; index += 1) {
    const char = body[index];
    if (char === '<' || char === '[' || char === '(' || char === '{') depth += 1;
    else if (char === '>' || char === ']' || char === ')' || char === '}') {
      depth = Math.max(0, depth - 1);
    } else if (char === ';' && depth === 0) {
      const member = body.slice(memberStart, index);
      const name = /^(?:readonly\s+)?([A-Za-z_$][\w$]*)\s*\??\s*:/.exec(member.trimStart());
      if (name) keys.push({ key: name[1], line: lineOf(source, open + 1 + memberStart) });
      memberStart = index + 1;
    }
  }
  return { keys, errors: [] };
}

/** Parse `export const <name>: Catalog = { ... }` into key -> entry.
 *
 *  An entry records every static string the value contributes, joined with a
 *  U+0000 slot marker so a `MessageFn` is compared and typography-checked as
 *  the one sentence it renders, not as loose fragments. */
export function parseLocaleCatalog(source, file) {
  const { skeleton, literals } = scanSource(source);
  const header =
    /export\s+const\s+[A-Za-z_$][\w$]*\s*:\s*(?:Catalog|LocaleCatalog)\s*=\s*\{/.exec(skeleton) ??
    (/\}\s*satisfies\s+(?:Catalog|LocaleCatalog)/.test(skeleton)
      ? /export\s+const\s+[A-Za-z_$][\w$]*\s*=\s*\{/.exec(skeleton)
      : null);
  if (!header) {
    return {
      entries: new Map(),
      errors: [
        finding(
          file,
          1,
          'catalog-unparsed',
          'no `export const <name>: Catalog = {` declaration found — the gate ' +
            'reads the catalog as text and cannot follow another shape',
        ),
      ],
    };
  }
  const open = header.index + header[0].length - 1;
  const close = matchingBrace(skeleton, open);
  if (close === -1) {
    return {
      entries: new Map(),
      errors: [finding(file, lineOf(source, open), 'catalog-unparsed', 'catalog literal is unterminated')],
    };
  }

  const entries = new Map();
  const errors = [];
  let index = open + 1;
  let depth = 0;
  while (index < close) {
    const rest = skeleton.slice(index, close);
    const key = /^[\s,]*([A-Za-z_$][\w$]*)\s*:/.exec(rest);
    if (!key) {
      // Whitespace to the closing brace is the normal end. Anything else is a
      // shape the scanner does not model (a spread, a computed key) — say so
      // rather than silently treating the rest of the catalog as absent.
      if (rest.trim() !== '') {
        errors.push(
          finding(
            file,
            lineOf(source, index + (rest.length - rest.trimStart().length)),
            'catalog-unparsed',
            'expected `key: value` — a catalog is a flat object literal of ' +
              'messages, with no spread and no computed keys',
          ),
        );
      }
      break;
    }
    const valueStart = index + key[0].length;
    let cursor = valueStart;
    depth = 0;
    while (cursor < close) {
      const char = skeleton[cursor];
      if ('{[('.includes(char)) depth += 1;
      else if ('}])'.includes(char)) depth -= 1;
      else if (char === ',' && depth === 0) break;
      cursor += 1;
    }
    const valueEnd = cursor;
    const parts = literals.filter((literal) => literal.start >= valueStart && literal.end <= valueEnd);
    const raw = skeleton.slice(valueStart, valueEnd).trim();
    const name = key[1];
    if (entries.has(name)) {
      errors.push(
        finding(file, lineOf(source, valueStart), 'catalog-duplicate-key', `duplicate key: ${name}`),
      );
    }
    entries.set(name, {
      key: name,
      line: lineOf(source, index + key[0].indexOf(name)),
      // A value that is exactly one quoted literal is a plain `Message`.
      plain: /^['"`]/.test(raw) && parts.length === 1 && !raw.includes('${'),
      parts,
      text: parts.map((part) => part.value).join(SLOT),
    });
    index = valueEnd + 1;
  }
  return { entries, errors };
}

// ---------------------------------------------------------------------------
// Rules 1-4
// ---------------------------------------------------------------------------

function words(text) {
  return (
    text
      .normalize('NFD')
      .replace(/[\u0300-\u036f]/g, '')
      .toLowerCase()
      .match(/[a-z]{2,}/g) ?? []
  );
}

/** Why an identical French value may be identical, or null if it may not.
 *
 *  `allowlist` is a parameter rather than a direct read of the module constant
 *  so the companion test can exercise the RULE without depending on today's
 *  exemptions — a test that has to be edited whenever a translator adds a word
 *  stops being run. */
export function identityExemption(key, text, allowlist = IDENTICAL_ALLOWLIST) {
  const explicit = Object.hasOwn(allowlist, key) ? allowlist[key] : null;
  if (explicit) return { source: 'allowlist', reason: explicit };
  const found = words(text);
  if (found.length === 0) {
    return { source: 'automatic', reason: 'no translatable word — punctuation, digits or slots only' };
  }
  if (found.every((word) => NEVER_TRANSLATE.has(word))) {
    return { source: 'automatic', reason: 'never-translate lexicon (docs/glossary-fr.md Tier 2/3)' };
  }
  return null;
}

/** The French typography contract, written with explicit escapes.
 *
 *  Every character this enforces is INVISIBLE in a diff, which is the whole
 *  reason the rule exists — and it is also the reason the patterns below spell
 *  their spaces \u202f and \u00a0 rather than pasting them: a reviewer can see
 *  which space a rule means. The patterns run against the rendered sentence
 *  with slots collapsed to U+0000, so a break that straddles a `${}` boundary
 *  is still caught and an adjacent slot never looks like a space.
 *
 *  `docs/glossary-fr.md`, decision 7. */
const TYPOGRAPHY = Object.freeze([
  {
    code: 'fr-narrow-space',
    pattern: /([ \u00a0])([;!?%\u00bb])/,
    message: (match) =>
      `space before "${match[2]}" must be U+202F (narrow no-break space), not ` +
      (match[1] === ' ' ? 'a plain space' : 'U+00A0'),
  },
  {
    code: 'fr-no-break-space',
    pattern: /[ \u202f]:/,
    message: () => 'space before ":" must be U+00A0 (no-break space), not a plain or narrow space',
  },
  {
    code: 'fr-apostrophe',
    pattern: /'/,
    message: () => 'straight apostrophe \u2014 French copy uses U+2019 (l\u2019identit\u00e9)',
  },
  {
    code: 'fr-quotes',
    pattern: /["\u201c\u201d]/,
    message: () => 'double quotes \u2014 French copy uses guillemets \u00ab \u00bb with U+202F inside',
  },
  {
    code: 'fr-guillemet-space',
    pattern: /\u00ab(?!\u202f)|(?<!\u202f)\u00bb/,
    message: () => 'guillemets take a U+202F narrow no-break space on the inside',
  },
  {
    code: 'fr-ellipsis',
    pattern: /\.\.\./,
    message: () => 'three dots \u2014 use U+2026 (\u2026)',
  },
]);

/** Rules 1-4 over an already-parsed pair of catalogs. */
export function checkCatalogs({
  interfaceKeys,
  locales,
  exempt = () => false,
  allowlist = IDENTICAL_ALLOWLIST,
}) {
  const findings = [];
  const declared = new Set(interfaceKeys.map((member) => member.key));

  for (const [tag, { file, entries }] of Object.entries(locales)) {
    // Rule 1 — key parity against the interface, in both directions.
    for (const member of interfaceKeys) {
      if (!entries.has(member.key)) {
        findings.push(
          finding(file, 1, 'key-missing', `missing key declared by Catalog: ${member.key}`),
        );
      }
    }
    for (const entry of entries.values()) {
      if (!declared.has(entry.key)) {
        findings.push(
          finding(file, entry.line, 'key-undeclared', `key is not declared by Catalog: ${entry.key}`),
        );
      }
      // Rule 2 — an empty value is a blank on screen that types accept.
      const empty = entry.parts.length === 0 || entry.parts.every((part) => part.value.trim() === '');
      if (empty && entry.plain && !exempt(file, entry.line)) {
        findings.push(
          finding(file, entry.line, 'value-empty', `${tag}: value is empty or whitespace-only`),
        );
      } else if (empty && !entry.plain && !exempt(file, entry.line)) {
        findings.push(
          finding(
            file,
            entry.line,
            'value-empty',
            `${tag}: message contributes no text of its own — if it renders a bare ` +
              'value that is correct, mark the line `i18n-exempt: <reason>`',
          ),
        );
      }
    }
  }

  const en = locales.en;
  const fr = locales.fr;
  if (!en || !fr) return findings.sort(compareFindings);

  // Rule 3 — a French value left in English.
  for (const [key, frEntry] of fr.entries) {
    const enEntry = en.entries.get(key);
    if (!enEntry) continue;
    if (enEntry.text !== frEntry.text) continue;
    const exemption = identityExemption(key, frEntry.text, allowlist);
    if (exemption) continue;
    findings.push(
      finding(
        fr.file,
        frEntry.line,
        'fr-untranslated',
        `${key}: French value is byte-identical to English — translate it, or ` +
          'add it to IDENTICAL_ALLOWLIST with the reason it is right',
      ),
    );
  }
  // Rule 3, stale side — an exemption that no longer exempts anything.
  for (const key of Object.keys(allowlist)) {
    const frEntry = fr.entries.get(key);
    const enEntry = en.entries.get(key);
    if (!frEntry || !enEntry) {
      findings.push(
        finding(
          fr.file,
          1,
          'allowlist-stale',
          `IDENTICAL_ALLOWLIST names ${key}, which is not in both catalogs`,
        ),
      );
      continue;
    }
    if (enEntry.text !== frEntry.text) {
      findings.push(
        finding(
          fr.file,
          frEntry.line,
          'allowlist-stale',
          `IDENTICAL_ALLOWLIST exempts ${key}, but the values now differ — drop the exemption`,
        ),
      );
    }
  }

  // Rule 4 — French typography, checked on the rendered sentence (slots stand
  // in as U+0000) so a break across a `${}` boundary is still seen.
  for (const entry of fr.entries.values()) {
    if (entry.key === 'localeTag') continue;
    if (exempt(fr.file, entry.line)) continue;
    for (const rule of TYPOGRAPHY) {
      const match = rule.pattern.exec(entry.text);
      if (!match) continue;
      findings.push(finding(fr.file, entry.line, rule.code, `${entry.key}: ${rule.message(match)}`));
    }
  }

  return findings.sort(compareFindings);
}

// ---------------------------------------------------------------------------
// Rule 5 — literals outside the catalog
// ---------------------------------------------------------------------------

/** Letters that survive `${}` interpolation are real copy — the same test
 *  `scripts/i18n-gate.mjs` applies to Dart, so the two clients agree on what
 *  counts as a user-visible literal. Where that is not enough on its own, the
 *  callers below also constrain WHERE the literal is allowed to sit. */
function bareLetters(text) {
  return /[A-Za-z]{2,}/.test(text.replace(/\$\{[^}]*\}/g, ' '));
}

const ATTRIBUTE_NAMES =
  'title|aria-label|aria-description|aria-placeholder|aria-roledescription|aria-valuetext|' +
  'placeholder|alt|label|summary|message|text';

/** Report user-visible literals in one component source. */
export function scanComponentLiterals(file, source) {
  const { code, skeleton } = scanSource(source);
  const lines = source.split('\n');
  const exempt = (line) =>
    (lines[line - 1] ?? '').includes('i18n-exempt') || (lines[line - 2] ?? '').includes('i18n-exempt');
  const findings = [];
  const report = (index, code_, message) => {
    const line = lineOf(source, index);
    if (exempt(line)) return;
    findings.push(finding(file, line, code_, message));
  };

  // JSX text between two tags. Excluding (){}<>=` keeps arrow-function bodies
  // and expression containers out: real copy rarely carries them, and a case
  // that does is caught by the attribute or property rules instead.
  for (const match of skeleton.matchAll(/>([^<>{}()=`]*[A-Za-z]{2}[^<>{}()=`]*)</g)) {
    const text = code.slice(match.index + 1, match.index + match[0].length - 1);
    if (!bareLetters(text) || text.trim() === '') continue;
    // A self-closing JSX node followed by another node inside an expression
    // object is code, not text: `slot: <Name />, next: <Name />`. The `/` alone
    // cannot exempt it because `<Icon />Delete account` is real visible copy.
    // Object/type member syntax across the newline distinguishes the code case.
    if (text.includes('\n') && /[,;]\s*[A-Za-z_$][\w$]*\??\s*:/.test(text)) continue;
    report(match.index + 1, 'jsx-text', `JSX text is not in the catalog: ${text.trim().slice(0, 60)}`);
  }
  // Copy-bearing JSX attributes and object properties, either quote style and
  // either `=` or `:` — `{ key: 'rooms', label: 'Rooms' }` is a table of copy.
  const attribute = new RegExp(
    String.raw`\b(${ATTRIBUTE_NAMES})\s*[:=]\s*\{?\s*(['"])((?:\\.|(?!\2).)*)\2`,
    'g',
  );
  for (const match of code.matchAll(attribute)) {
    if (!bareLetters(match[3])) continue;
    report(match.index, 'copy-attribute', `${match[1]} takes a literal, not a catalog message: ${match[3].slice(0, 60)}`);
  }
  return findings;
}

function componentFiles(repoRoot) {
  const files = [];
  const walk = (absolute) => {
    for (const entry of readdirSync(absolute, { withFileTypes: true }).sort((a, b) =>
      a.name < b.name ? -1 : a.name > b.name ? 1 : 0,
    )) {
      const path = resolve(absolute, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (/\.tsx?$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name)) files.push(path);
    }
  };
  for (const root of LITERAL_SCAN_ROOTS) {
    const absolute = resolve(repoRoot, root);
    if (!existsSync(absolute)) continue;
    if (/\.tsx?$/.test(root)) files.push(absolute);
    else walk(absolute);
  }
  return files;
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

function compareFindings(a, b) {
  const text = (x, y) => (x < y ? -1 : x > y ? 1 : 0);
  return (
    text(a.file, b.file) || a.line - b.line || text(a.code, b.code) || text(a.message, b.message)
  );
}

function toRepoPath(repoRoot, path) {
  return relative(repoRoot, path).split(sep).join('/');
}

/** Run every rule against a repository tree and return sorted findings.
 *
 *  The two exemption maps are parameters, defaulting to this module's, so the
 *  companion test can build a fixture tree and exercise a rule without the
 *  fixture having to reproduce the real repository's exemptions. */
export function checkUiI18n({
  repoRoot = DEFAULT_REPO_ROOT,
  scanLiterals = LITERAL_SCAN_ENABLED,
  allowlist = IDENTICAL_ALLOWLIST,
  exemptFiles = EXEMPT_FILES,
} = {}) {
  const root = resolve(repoRoot);
  const findings = [];
  const read = (relativePath) => {
    const absolute = resolve(root, relativePath);
    return existsSync(absolute) ? readFileSync(absolute, 'utf8') : null;
  };

  const interfaceSource = read(CATALOG_INTERFACE);
  let interfaceKeys = [];
  if (interfaceSource === null) {
    findings.push(finding(CATALOG_INTERFACE, 1, 'catalog-missing', 'the Catalog interface is missing'));
  } else {
    const parsed = parseCatalogInterface(interfaceSource, CATALOG_INTERFACE);
    interfaceKeys = parsed.keys;
    findings.push(...parsed.errors);
  }

  const locales = {};
  const sources = {};
  for (const [tag, path] of Object.entries(LOCALE_FILES)) {
    const source = read(path);
    if (source === null) {
      findings.push(
        finding(path, 1, 'catalog-missing', `the ${tag} catalog is missing — every locale ships complete`),
      );
      continue;
    }
    sources[path] = source.split('\n');
    const parsed = parseLocaleCatalog(source, path);
    findings.push(...parsed.errors);
    locales[tag] = { file: path, entries: parsed.entries };
  }

  const exempt = (file, line) => {
    const lines = sources[file] ?? [];
    return (lines[line - 1] ?? '').includes('i18n-exempt') || (lines[line - 2] ?? '').includes('i18n-exempt');
  };
  findings.push(...checkCatalogs({ interfaceKeys, locales, exempt, allowlist }));

  // Rule 5, stale side — an exemption whose file is gone.
  for (const [path, reason] of Object.entries(exemptFiles)) {
    if (existsSync(resolve(root, path))) continue;
    findings.push(
      finding(path, 1, 'exempt-stale', `EXEMPT_FILES names a file that does not exist (${reason.slice(0, 40)}…)`),
    );
  }

  if (scanLiterals) {
    for (const absolute of componentFiles(root)) {
      const file = toRepoPath(root, absolute);
      if (Object.hasOwn(exemptFiles, file)) continue;
      findings.push(...scanComponentLiterals(file, readFileSync(absolute, 'utf8')));
    }
  }

  return findings.sort(compareFindings);
}

function main() {
  const findings = checkUiI18n(REPORT_ONLY ? { scanLiterals: true } : {});
  if (REPORT_ONLY) {
    // Reporting mode never fails: it exists so the staged rule's remaining
    // surface stays countable, not so it can block a merge by the back door.
    const staged = findings.filter((f) => f.code === 'jsx-text' || f.code === 'copy-attribute');
    console.log(`check-ui-i18n --report: ${staged.length} literal(s) still outside the catalog`);
    const byFile = new Map();
    for (const entry of staged) byFile.set(entry.file, (byFile.get(entry.file) ?? 0) + 1);
    for (const [file, count] of [...byFile].sort((a, b) => b[1] - a[1])) {
      console.log(`  ${String(count).padStart(4)}  ${file}`);
    }
    process.exit(0);
  }
  if (findings.length > 0) {
    console.error(`check-ui-i18n: ${findings.length} finding(s)\n`);
    for (const entry of findings) {
      console.error(`  ${entry.file}:${entry.line}  [${entry.code}] ${entry.message}`);
    }
    console.error('\nCopy belongs in ui/src/l10n/catalog.ts (the shape) and in EVERY');
    console.error('locale catalog beside it — en.ts is the source of truth, fr.ts ships');
    console.error('complete or not at all. Components read it at render time through');
    console.error('useStrings(); a sentence with styled or interactive segments is ONE');
    console.error('message rendered through <Template>, never fragments joined in JSX.');
    console.error('Non-copy glyphs, commands, wire examples and the brand go in');
    console.error('ui/src/l10n/tokens.ts. French typography is docs/glossary-fr.md');
    console.error('decision 7 — U+202F before ; ! ? %, U+00A0 before :, U+2019, U+2026,');
    console.error('guillemets. A French value that is right in English anyway goes in');
    console.error('IDENTICAL_ALLOWLIST with the reason. Deliberate one-off exceptions:');
    console.error('`// i18n-exempt: <reason>` on the line or the line above.');
    process.exitCode = 1;
    return;
  }
  console.log('check-ui-i18n: OK — catalogs are complete, translated, and typographically French.');
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH) main();
