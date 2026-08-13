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
// The WHOLE crate `src` surface — a component may live in any module (`app.rs`,
// `components/`, `compose.rs`, a future `settings.rs`), so scanning a fixed subset lets
// hardcoded copy in a new module bypass the gate. The catalog locale files are checked
// by the catalog rules instead and are EXCLUDED below (their method literals are copy
// but not RSX; scanning them here would be redundant).
const LITERAL_SCAN_ROOTS = Object.freeze(['crates/jeliya-ui/src']);

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
  return raw.replace(
    /\\(?:\n[ \t]*|(u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|[\s\S]))/g,
    (_, escape) => {
      // Rust LINE CONTINUATION: a backslash before a newline elides the newline AND
      // the next line's leading whitespace, so `"Bonjour\u{202f}\<NL>  !"` renders
      // `Bonjour !` (U+202F immediately before `!`). Decoding it as a literal newline
      // + indentation instead makes the typography gate see whitespace before `!` and
      // wrongly flag it. `escape` is undefined for this branch.
      if (escape === undefined) return '';
      if (escape.startsWith('u{')) return String.fromCodePoint(parseInt(escape.slice(2, -1), 16));
      if (/^x[0-9a-fA-F]{2}$/.test(escape)) return String.fromCodePoint(parseInt(escape.slice(1), 16));
      return { n: '\n', t: '\t', r: '\r', '0': '\0', '\\': '\\', '"': '"', "'": "'" }[escape] ?? escape;
    },
  );
}

/** Blank comments (newlines preserved so line numbers survive) and collect every
 *  string literal with its decoded value and source position. Also returns a
 *  `skeleton` (code with string CONTENTS blanked) so structural look-behind
 *  cannot trip over a brace or colon inside copy.
 *
 *  Handles `"…"` and raw strings `r#"…"#`. Rust `'` (lifetimes and char
 *  literals) is not string-significant in any scanned file and is ignored. */
export function scanRustSource(source) {
  // Split by UTF-16 code UNIT (`split('')`), not code POINT (`Array.from`): every
  // scanner offset — `source.length`, `indexOf`, regex `.index` — is a code-unit
  // position, so a code-point array drifts once any non-BMP char (an emoji, some
  // CJK) precedes scanned markup, and a later RSX literal would map to the wrong
  // skeleton range and be missed. Code-unit indexing keeps literals and RSX ranges
  // in one coordinate system.
  const masked = source.split('');
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
      // Rust block comments NEST: `/* a /* b */ c */` closes at the OUTER `*/`, not the
      // first one. Track depth — an `indexOf('*/')` stops at the inner terminator and
      // leaks the remainder (including a stray `}`) into the skeleton, which could close
      // a detected RSX range early and let later hardcoded copy pass the gate.
      let depth = 1;
      let j = i + 2;
      while (j < source.length && depth > 0) {
        if (source[j] === '/' && source[j + 1] === '*') {
          depth += 1;
          j += 2;
        } else if (source[j] === '*' && source[j + 1] === '/') {
          depth -= 1;
          j += 2;
        } else {
          j += 1;
        }
      }
      const stop = j; // past the matching outer `*/`, or source end if unterminated
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
        // A Rust ordinary string may contain an UNESCAPED newline, so scan to the
        // CLOSING quote, not the end of line: breaking at `\n` truncates valid
        // multiline copy (missing the literal in RSX) and corrupts the catalog skeleton
        // (a spurious `catalog-unparsed`). An unterminated string simply scans to EOF.
        if (source[j] === '"') break;
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
    // CHAR literal (`'x'`, `'\n'`, `'\''`, `'}'`) — distinct from a LIFETIME
    // (`'a`, `'static`, which is NOT closed by a `'`). Blank its content so a
    // delimiter inside it (`'}'`, `'{'`, `'"'`) is not counted as macro/RSX
    // structure or mistaken for a string start.
    if (ch === "'") {
      // A BYTE literal `b'A'` renders a `u8` number, not text — mask it for delimiter
      // balancing but do NOT record it as copy. `b` must be a standalone prefix, not
      // the tail of an identifier (`numb`), so the char before it is a non-word char.
      const isByte = source[i - 1] === 'b' && !(i - 2 >= 0 && /\w/.test(source[i - 2]));
      if (source[i + 1] === '\\') {
        let j = i + 2;
        while (j < source.length && source[j] !== "'" && source[j] !== '\n') j += 1;
        if (source[j] === "'") {
          // RECORD the char before masking: a `char` rendered as UI copy
          // (`let label = 'A'; "{label}"`) must be validated like a one-char string.
          if (!isByte) {
            literals.push({
              start: i,
              end: j + 1,
              contentStart: i + 1,
              contentEnd: j,
              value: unescapeRust(source.slice(i + 1, j)),
              raw: false,
            });
          }
          blank(i, j + 1);
          i = j + 1;
          continue;
        }
      } else if (i + 2 < source.length) {
        // A char literal `'X'` — X is ONE Unicode scalar, which is a surrogate PAIR (2
        // code units) for a non-BMP scalar like `'🦀'`/`'𠀀'`. Find the closing quote by
        // scalar width, not a fixed `i + 2`, or a non-BMP char is missed and its copy
        // slips past the gate.
        const scalar = source.codePointAt(i + 1);
        const width = scalar !== undefined && scalar > 0xffff ? 2 : 1;
        const close = i + 1 + width;
        if (source[close] === "'") {
          if (!isByte) {
            literals.push({
              start: i,
              end: close + 1,
              contentStart: i + 1,
              contentEnd: close,
              value: source.slice(i + 1, close),
              raw: false,
            });
          }
          blank(i, close + 1);
          i = close + 1;
          continue;
        }
      }
      // Otherwise a lifetime — leave it untouched.
    }
    i += 1;
  }

  // Copy the code-UNIT `masked` array (NOT `Array.from(join)`, which would re-split
  // into code POINTS and drift from the code-unit `contentStart`/`contentEnd` offsets
  // once any non-BMP char precedes a literal).
  const skeleton = masked.slice();
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

/** Collapse `{…}` format slots to the sentinel, PRESERVING Rust's escaped braces
 *  (`{{`/`}}` render the literal characters `{`/`}`, not a slot). `format!("{{Delete}}")`
 *  renders `{Delete}` — its letters must survive so the untranslated and typography
 *  checks see them, instead of collapsing to a slot and dropping every letter. Shield
 *  the escaped braces (with transient private-use sentinels), collapse real slots, then
 *  restore the shields as their RENDERED single braces. */
function collapseSlots(text) {
  return text
    .replace(/\{\{/g, "\u0001")
    .replace(/\}\}/g, "\u0002")
    .replace(/\{[^{}]*\}/g, SLOT)
    .replace(/\u0001/g, "{")
    .replace(/\u0002/g, "}");
}

/** The sorted multiset of format placeholders across a value set (`{n}` -> "n").
 *  Rust enforces only the method SIGNATURE and permits an unused argument, so a
 *  translation that drops, renames, or duplicates a `{…}` slot still compiles;
 *  comparing this set between EN and FR is what actually enforces placeholder
 *  parity. */
function slotSet(values) {
  const slots = [];
  for (const value of values) {
    // Drop Rust's ESCAPED braces first: `{{`/`}}` render the literal characters
    // `{`/`}`, not an interpolation slot. Without this, FR `format!("Compte {{n}}")`
    // (which displays the literal text `{n}`) yields the same slot set `["n"]` as EN
    // `format!("Count {n}")`, so the parity gate passes though the French argument is
    // unused and the text is wrong.
    const unescaped = value.replace(/\{\{|\}\}/g, '');
    // Compare the ARGUMENT identity, not the static format spec: `{n}` and `{n:>5}`
    // bind the same parameter `n`. But a `$`-referenced param in the spec (`{a:w1$}`
    // takes its width from `w1`) is a REAL placeholder whose binding must match, so
    // fold each `<ref>$` into the slot token — EN `{a:w1$}` and FR `{a:w2$}` then
    // differ. Static alignment/precision (`>5`, `.2`) carries no `$` and is ignored.
    for (const match of unescaped.matchAll(/\{([^{}]*)\}/g)) {
      const content = match[1];
      const colon = content.indexOf(':');
      const argName = (colon === -1 ? content : content.slice(0, colon)).trim();
      const spec = colon === -1 ? '' : content.slice(colon + 1);
      const refs = [...spec.matchAll(/(\w+)\$/g)].map((r) => r[1]).sort();
      slots.push(refs.length ? `${argName}:${refs.join(',')}` : argName);
    }
  }
  return slots.sort();
}

function words(text) {
  // Include ONE-letter words: dropping them lets `"A daemon"` tokenize to just
  // `daemon` and, if that is a never-translate token, be auto-exempted from the
  // untranslated check even though `A` is translatable copy. A value with no
  // letters at all still yields `[]` (a language-neutral value stays exempt).
  return (
    text
      .normalize('NFD')
      .replace(/[̀-ͯ]/g, '')
      .toLowerCase()
      // Unicode letters, not ASCII only: an identical EN/FR value in another script
      // (`"Удалить"`, `"删除"`) is translatable copy, so it must yield words and NOT be
      // auto-exempted as language-neutral. The automatic exemption stays only for
      // values that truly contain no letters.
      .match(/\p{L}+/gu) ?? []
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
  // Match up to the parameter `(` only, then BALANCE-match the param list — a
  // parameter type may contain parens (a tuple `(u32, u32)`), which a `[^)]*`
  // capture would truncate, silently omitting the method from the parsed maps.
  const methodRe = /\bfn\s+([a-z_][A-Za-z0-9_]*)\s*\(/g;
  let match;
  while ((match = methodRe.exec(skeleton)) !== null) {
    const name = match[1];
    const parenOpen = match.index + match[0].length - 1;
    let pdepth = 0;
    let parenClose = -1;
    for (let at = parenOpen; at < skeleton.length; at += 1) {
      if (skeleton[at] === '(') pdepth += 1;
      else if (skeleton[at] === ')') {
        pdepth -= 1;
        if (pdepth === 0) {
          parenClose = at;
          break;
        }
      }
    }
    if (parenClose === -1) break;
    const params = skeleton.slice(parenOpen + 1, parenClose);
    // A catalog method is `fn <name>(&self …) -> … {`; skip anything else.
    const sig = /^\s*&self\b/.test(params)
      ? /^\s*->\s*[^{;]*\{/.exec(skeleton.slice(parenClose + 1))
      : null;
    if (!sig) {
      methodRe.lastIndex = parenClose + 1;
      continue;
    }
    const open = parenClose + 1 + sig[0].length - 1;
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
    // Only the RETURNED expression's literals are rendered copy. A catalog method's
    // returned value is its tail expression — everything AFTER the last TOP-LEVEL
    // `;` in the body (a `let _note = "Aucun salon";` binding or a
    // `debug_assert!(cond, "Aucun salon");` statement is a non-rendered statement,
    // not the return). Exclude any literal at or before that last top-level `;` so a
    // statement literal cannot pollute the arm values and mask the English text
    // actually rendered. (A method with no top-level `;` — a bare literal or a
    // `match` — keeps every arm literal.)
    let lastTopLevelSemi = open;
    let bodyDepth = 0;
    for (let at = open + 1; at < close; at += 1) {
      const c = skeleton[at];
      if (c === '{' || c === '(' || c === '[') bodyDepth += 1;
      else if (c === '}' || c === ')' || c === ']') bodyDepth -= 1;
      else if (c === ';' && bodyDepth === 0) lastTopLevelSemi = at;
    }
    const parts = literals.filter(
      (l) => l.start > lastTopLevelSemi && l.end <= close,
    );
    // A returned branch whose ENTIRE value is an empty-string constructor
    // (`=> String::new()`, `{ String::default() }`, or the inferred `Default::default()`
    // where the return type is `String`) contributes NO string literal, so the
    // literal-based `parts` miss it — yet it renders BLANK copy for that input. Count
    // each such branch and record a synthetic empty value, so `value-empty` flags it
    // INDEPENDENTLY of sibling branches that carry text. Deliberately NOT matched:
    // `_ => None` (returns `Option::None`, not an empty string) and a ctor used as a
    // call ARGUMENT (`format!("x", String::new())` renders "x", so it is preceded by
    // `,`/`(`, not `=>`/`{`).
    let emptyBranchCount = 0;
    {
      const returned = skeleton.slice(lastTopLevelSemi, close);
      const emptyCtorRe =
        /(?:=>|\{)\s*(?:\w+\s*::\s*)*(?:String\s*::\s*(?:new|default)|Default\s*::\s*default)\s*\(\s*\)/g;
      while (emptyCtorRe.exec(returned) !== null) emptyBranchCount += 1;
      // A statically-`None` Option defaulted to its value also renders the empty
      // string (`Option::<String>::None.unwrap_or_default()` → `String::default()`
      // → ""), so count it as a blank branch too. Scoped to a literal `None`
      // receiver — an arbitrary `.unwrap_or_default()` on a value is not statically
      // empty and must not be swept in.
      const emptyDefaultRe =
        /(?:=>|\{)\s*(?:\w+\s*::\s*)*(?:<[^>]*>\s*::\s*)?None\s*\.\s*unwrap_or_default\s*\(\s*\)/g;
      while (emptyDefaultRe.exec(returned) !== null) emptyBranchCount += 1;
      // `String::with_capacity(N)` allocates capacity but renders the EMPTY string —
      // guaranteed blank regardless of the argument — so count it as a blank branch
      // too (its non-empty parens keep it out of the `\(\s*\)` ctor regex above).
      const emptyCapacityRe =
        /(?:=>|\{)\s*(?:\w+\s*::\s*)*String\s*::\s*with_capacity\s*\([^)]*\)/g;
      while (emptyCapacityRe.exec(returned) !== null) emptyBranchCount += 1;
    }
    const emptyBranchValues = Array.from({ length: emptyBranchCount }, () => '');
    const isPlural = /\bPluralCategory\b/.test(params);
    if (entries.has(name)) {
      errors.push(finding(file, lineOf(source, open), 'catalog-duplicate-key', `duplicate method: ${name}`, 'catalog'));
    }
    // For a plural method, group each literal by its PluralCategory arm (One /
    // Other) rather than by SOURCE POSITION, so a locale that lists the arms in a
    // different order is still compared category-against-category (parity would
    // otherwise misalign fr `Other` against en `One`).
    let slotsByCategory = null;
    let valuesByCategory = null;
    if (isPlural) {
      const bodyText = skeleton.slice(open, close);
      // Recognize BOTH an explicit `PluralCategory::One`/`Other` arm AND a wildcard
      // `_ =>` arm. Without the wildcard, literals in a `_ => …` (a common way to
      // write the `Other` arm) inherit the LAST explicit marker's category (One),
      // so EN could place `{n}` in the explicit One arm while FR places it in the
      // wildcard arm and still pass per-category parity. A `_` covers the category
      // NOT explicitly present (Other when One is explicit, else One).
      const raw = [];
      const explicitCats = new Set();
      // A `PluralCategory::X` reference marks its category; `_ =>` is the wildcard
      // arm. Two forms: a `match category { … }` uses `=>` arms, while an
      // `if category == PluralCategory::X { … } else { … }` DISPATCH puts the enum in
      // the CONDITION before both blocks — so for the if/else form `else` FLIPS the
      // category to the other block (without it both blocks inherit X and the other
      // category reads as missing). `else` is honored ONLY in the if/else form, or a
      // nested `else` inside a match arm would misclassify.
      const usesMatch = /\bmatch\b/.test(bodyText);
      const catRe = usesMatch
        ? /PluralCategory::(One|Other)\b|\b_\s*=>/g
        : /PluralCategory::(One|Other)\b|\belse\b/g;
      for (let m = catRe.exec(bodyText); m; m = catRe.exec(bodyText)) {
        const cat = m[1] ?? (m[0].startsWith('else') ? 'FLIP' : null);
        raw.push({ at: open + m.index, cat });
        if (m[1]) explicitCats.add(m[1]);
      }
      const wildcardCat = explicitCats.has('One') ? 'Other' : 'One';
      const flip = (c) => (c === 'One' ? 'Other' : c === 'Other' ? 'One' : wildcardCat);
      const markers = [];
      let lastExplicit = null;
      for (const r of raw) {
        let cat;
        if (r.cat === 'FLIP') cat = flip(lastExplicit);
        else if (r.cat === null) cat = wildcardCat;
        else {
          cat = r.cat;
          lastExplicit = r.cat;
        }
        markers.push({ at: r.at, cat });
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
      // Retain the VALUES per category too (collapsed like `values`), so the
      // untranslated check can compare fr↔en by category rather than source order.
      valuesByCategory = {
        One: valuesByCat.One.map(collapseSlots),
        Other: valuesByCat.Other.map(collapseSlots),
      };
    }
    // For a NONPLURAL method with `match` arms, key each literal by its arm PATTERN
    // (`1 =>`, `_ =>`, …) too, so a locale that REORDERS arms is compared
    // key-against-key — a positional compare would misalign a reordered untranslated
    // branch (e.g. an English `_ => "August"`) and miss it.
    let valuesByBranch = null;
    // Placeholder set PER match arm (keyed by arm pattern), so a NONPLURAL `match`
    // method's parity is compared branch-against-branch — a French translation that
    // swaps `{n}`/`{x}` BETWEEN arms (identical pooled multiset) is caught, which the
    // pooled `slots` fallback misses.
    let slotsByBranch = null;
    // For a NON-match branching return (`if c { "a" } else { "b" }`), each alternative
    // block is its own branch — see the computation below.
    let valuesByBlock = null;
    // Parallel per-block placeholder multiset, so if/else placeholder parity is compared
    // branch-by-branch (a `{n}`/`{x}` swap between branches has an identical pooled set).
    let slotsByBlock = null;
    if (!isPlural) {
      const bodyText = skeleton.slice(open, close);
      const markers = [];
      const arrowRe = /=>/g;
      for (let a = arrowRe.exec(bodyText); a; a = arrowRe.exec(bodyText)) {
        // Walk back from `=>` to the arm separator (`,`/`;`/block `{` at depth 0) for
        // the pattern.
        let s = a.index - 1;
        let d = 0;
        while (s >= 0) {
          const c = bodyText[s];
          if (c === '}' || c === ')' || c === ']') d += 1;
          else if (c === '{' || c === '(' || c === '[') {
            if (d === 0) break;
            d -= 1;
          } else if ((c === ',' || c === ';') && d === 0) break;
          s -= 1;
        }
        markers.push({ at: open + a.index + 2, key: bodyText.slice(s + 1, a.index).trim() });
      }
      if (markers.length > 0) {
        valuesByBranch = {};
        slotsByBranch = {};
        for (const p of parts) {
          let armKey = null;
          for (const mk of markers) {
            if (mk.at <= p.start) armKey = mk.key;
            else break;
          }
          if (armKey !== null) {
            (valuesByBranch[armKey] ??= []).push(collapseSlots(p.value));
            (slotsByBranch[armKey] ??= []).push(...slotSet([p.value]));
          }
        }
        // Sort each arm's pooled slot multiset so the per-arm comparison is
        // order-independent (a slot set is a sorted multiset, like `slots`).
        for (const armKey of Object.keys(slotsByBranch)) slotsByBranch[armKey].sort();
      }
      // NON-match branching (`if c { "a" } else { "b" }`): each top-level `{ … }` block
      // is an ALTERNATIVE branch (only one renders), so its literals compare
      // independently — joining them (concat semantics) would let an untranslated
      // `else` branch hide behind a translated `if`. Only when there are NO `=>` arms
      // (a `match` uses valuesByBranch; its body's outer `{}` would otherwise read as one
      // block). Group `parts` by enclosing depth-1 block in the returned region; a bare
      // return / `concat!` args (no block) leave this null → the base join path governs.
      if (markers.length === 0) {
        const blockRanges = [];
        let depth = 0;
        let start = -1;
        for (let at = lastTopLevelSemi + 1; at < close; at += 1) {
          const c = skeleton[at];
          if (c === '{') {
            if (depth === 0) start = at + 1;
            depth += 1;
          } else if (c === '}') {
            depth -= 1;
            if (depth === 0 && start !== -1) {
              blockRanges.push([start, at]);
              start = -1;
            }
          }
        }
        if (blockRanges.length > 0) {
          // Only a block's TAIL expression renders. A statement literal
          // (`if c { let _note = "…"; "tail" }`) sits before the block's last
          // top-level `;`, so it must NOT be concatenated with the returned value —
          // differing EN/FR statement literals would otherwise hide an untranslated
          // tail. Compute each block's tail start (after its last depth-0 `;`).
          const tailStart = blockRanges.map(([bStart, bEnd]) => {
            let last = bStart;
            let d = 0;
            for (let at = bStart; at < bEnd; at += 1) {
              const c = skeleton[at];
              if (c === '{' || c === '(' || c === '[') d += 1;
              else if (c === '}' || c === ')' || c === ']') d -= 1;
              else if (c === ';' && d === 0) last = at + 1;
            }
            return last;
          });
          const groups = [[]]; // [0] = base (literals in no block)
          const slotGroups = [[]]; // parallel: the slot multiset per block
          for (const p of parts) {
            let idx = 0;
            let isStatement = false;
            for (let b = 0; b < blockRanges.length; b += 1) {
              if (p.start >= blockRanges[b][0] && p.end <= blockRanges[b][1]) {
                idx = b + 1;
                if (p.start < tailStart[b]) {
                  // Before the block's tail — a statement literal, UNLESS it is an
                  // explicit `return <literal>;`, which IS a rendered branch value.
                  // Find this literal's statement start (nearest depth-0 `;` in the
                  // block) and keep it only when that statement begins with `return`.
                  let sstart = blockRanges[b][0];
                  let d = 0;
                  for (let at = p.start - 1; at >= blockRanges[b][0]; at -= 1) {
                    const c = skeleton[at];
                    if (c === '}' || c === ')' || c === ']') d += 1;
                    else if (c === '{' || c === '(' || c === '[') d -= 1;
                    else if (c === ';' && d === 0) {
                      sstart = at + 1;
                      break;
                    }
                  }
                  if (!/\breturn\b/.test(skeleton.slice(sstart, p.start))) isStatement = true;
                }
                break;
              }
            }
            if (isStatement) continue; // a statement literal, not the rendered tail
            (groups[idx] ??= []).push(collapseSlots(p.value));
            (slotGroups[idx] ??= []).push(...slotSet([p.value]));
          }
          for (const g of slotGroups) if (g) g.sort();
          valuesByBlock = groups;
          slotsByBlock = slotGroups;
        }
      }
    }
    entries.set(name, {
      key: name,
      line: lineOf(source, match.index),
      isPlural,
      // Append a synthetic empty value for each literal-free empty-string-constructor
      // branch (`=> String::new()`), so `value-empty` sees blank branches the literal
      // scan cannot.
      values: parts.map((p) => collapseSlots(p.value)).concat(emptyBranchValues),
      // How many literal-free empty-string-constructor branches this method has. Those
      // blank branches are NOT in `valuesByBranch`/`Block` (which hold only literal
      // fragments), so the complete-branch emptiness check reads this count directly.
      emptyBranchCount,
      slots: slotSet(parts.map((p) => p.value)),
      slotsPerArm: parts.map((p) => slotSet([p.value])),
      slotsByCategory,
      valuesByCategory,
      valuesByBranch,
      slotsByBranch,
      valuesByBlock,
      slotsByBlock,
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
  // Auto-exempt ONLY when the ENTIRE value is a single exact protocol token — not when
  // its words merely all appear in the lexicon. `"Hash mismatch"` is prose (both `hash`
  // and `mismatch` are tokens, so word-by-word `every` wrongly exempted it) whereas the
  // glossary exempts the exact wire code, and surrounding UI prose must be translated.
  // Multi-word protocol phrases need an explicit, reasoned allowlist entry.
  const core = text
    .normalize('NFD')
    .replace(/[̀-ͯ]/g, '')
    .toLowerCase()
    .replace(/^[^a-z0-9]+|[^a-z0-9]+$/g, '');
  if (NEVER_TRANSLATE.has(core)) {
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

/** The COMPLETE rendered value of each ALTERNATIVE branch of a catalog method
 *  (fragments within a branch concatenated), for checks that must see whole rendered
 *  copy rather than individual literals: plural categories, `match` arms, `if`/`else`
 *  blocks, else the single joined return. */
function renderedBranchValues(entry) {
  if (entry.valuesByCategory) {
    return ['One', 'Other'].map((cat) => (entry.valuesByCategory[cat] ?? []).join(''));
  }
  if (entry.valuesByBranch) {
    return Object.values(entry.valuesByBranch).map((arm) => arm.join(''));
  }
  if (entry.valuesByBlock) {
    return entry.valuesByBlock.map((group) => (group ?? []).join(''));
  }
  return [entry.values.join('')];
}

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
  // A value with a PLACEHOLDER renders content (`format!("{n}")` shows a count like
  // `42`), so it is NOT empty — only a value with no slot AND no non-whitespace
  // text is. Deleting the slot sentinel before the emptiness check (the old
  // behaviour) wrongly flagged a placeholder-only message as `value-empty`.
  const isEmptyValue = (v) => !v.includes(SLOT) && v.trim() === '';
  for (const locale of [en, fr]) {
    for (const entry of locale.entries.values()) {
      // Emptiness is a property of each COMPLETE rendered branch, not of individual
      // concatenated fragments: `concat!("Hello", " ", "world")` renders a non-empty
      // message, and its standalone `" "` fragment must not be read as an empty value.
      // A genuinely blank branch (a `String::new()` arm, an empty `if`/`else` block)
      // still renders `""` and is caught.
      const empty =
        entry.values.length === 0 ||
        entry.emptyBranchCount > 0 ||
        renderedBranchValues(entry).some(isEmptyValue);
      if (empty) {
        findings.push(finding(locale.file, entry.line, 'value-empty', `${entry.key}: value is empty or whitespace-only`, 'catalog'));
      }
    }
  }

  // Reject POSITIONAL format arguments (`{0}`, `{1}`, `{}`): comparing slot SETS
  // cannot tell that two locales bind the same positional slots to DIFFERENT
  // arguments — EN `format!("Room {0} for {1}", room, user)` and FR
  // `format!("Salon {0} pour {1}", user, room)` share the set `["0","1"]` yet swap
  // the values. Named/captured params (`{room}`) are checked by identity, so require
  // them. A slot is positional when its arg reference (before any `:` spec) is empty
  // or all digits.
  const isPositionalSlot = (slot) => /^\d*$/.test(slot.split(':')[0].trim());
  for (const locale of [en, fr]) {
    for (const entry of locale.entries.values()) {
      if (entry.slots.some(isPositionalSlot)) {
        findings.push(finding(locale.file, entry.line, 'positional-format', `${entry.key}: use a NAMED format parameter (\`{name}\`), not a positional one (\`{0}\`/\`{}\`) — positional slots let two locales bind the same slot to different arguments`, 'catalog'));
      }
    }
  }

  // Rule 5 — plural methods present in both, with both arms.
  for (const [key, frEntry] of fr.entries) {
    const enEntry = en.entries.get(key);
    if (!enEntry) continue;
    if (enEntry.isPlural !== frEntry.isPlural) {
      findings.push(finding(fr.file, frEntry.line, 'plural-parity', `${key}: plural-ness differs between en and fr`, 'catalog'));
    } else if (frEntry.isPlural) {
      // Each plural CATEGORY must render non-empty copy in BOTH locales — checked
      // PER CATEGORY, not by pooled fragment count. `One => concat!("One ", "room"),
      // Other => unreachable!()` yields two POOLED fragments (both in `One`) yet
      // renders nothing for `Other`; a `values.length >= 2` gate would pass it.
      const rendersCategory = (entry, cat) =>
        Array.isArray(entry.valuesByCategory?.[cat]) &&
        entry.valuesByCategory[cat].join('').length > 0;
      const missing = ['One', 'Other'].some(
        (cat) => !rendersCategory(frEntry, cat) || !rendersCategory(enEntry, cat),
      );
      if (missing) {
        findings.push(finding(fr.file, frEntry.line, 'plural-parity', `${key}: a plural message must render both categories (one/other) in both locales`, 'catalog'));
      }
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
    } else if (frEntry.slotsByBranch && enEntry.slotsByBranch) {
      // NONPLURAL `match`: compare placeholders PER ARM (keyed by pattern), so a
      // French translation that SWAPS `{n}`/`{x}` BETWEEN branches — an identical
      // POOLED multiset — is caught. The pooled `slots` fallback below would pass it.
      const armKeys = new Set([
        ...Object.keys(frEntry.slotsByBranch),
        ...Object.keys(enEntry.slotsByBranch),
      ]);
      if (
        [...armKeys].some(
          (armKey) =>
            !slotsEqual(frEntry.slotsByBranch[armKey] ?? [], enEntry.slotsByBranch[armKey] ?? []),
        )
      ) {
        findings.push(finding(fr.file, frEntry.line, 'placeholder-parity', `${key}: fr placeholders differ from en in a match arm — a translation moved, dropped, or duplicated a format slot between branches`, 'catalog'));
      }
    } else if (frEntry.slotsByBlock && enEntry.slotsByBlock) {
      // NON-match if/else: compare placeholders PER BLOCK (aligned by index), so a
      // French `if c { "Compte {x}" } else { "Nom {n}" }` against EN `{n}`/`{x}` — an
      // identical POOLED multiset — is caught. The pooled fallback below would pass it.
      const n = Math.max(frEntry.slotsByBlock.length, enEntry.slotsByBlock.length);
      for (let i = 0; i < n; i += 1) {
        if (!slotsEqual(frEntry.slotsByBlock[i] ?? [], enEntry.slotsByBlock[i] ?? [])) {
          findings.push(finding(fr.file, frEntry.line, 'placeholder-parity', `${key}: fr placeholders differ from en in a conditional branch — a translation moved, dropped, or duplicated a format slot between branches`, 'catalog'));
          break;
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
    // Byte-identical detection: for a PLURAL compare fr↔en by CATEGORY, not source
    // order — a harmless arm reordering must not hide that every French category is
    // still English. Otherwise compare the values directly.
    if (
      frEntry.isPlural &&
      enEntry.isPlural &&
      frEntry.valuesByCategory &&
      enEntry.valuesByCategory
    ) {
      // Check each PLURAL category independently: a category whose French value is
      // byte-identical to English AND carries translatable text (not a language-
      // neutral arm like `{n}`, which `identityExemption` clears) is untranslated —
      // even if OTHER arms were translated. A PARTIAL translation (one arm French,
      // another still English) must be caught, not only a wholly-English plural.
      // Concatenate the arm's literal fragments into the COMPLETE rendered value
      // (`join('')`) before comparing, so a locale that splits equivalent copy into a
      // different number of literals is still compared text-against-text.
      const untranslated = ['One', 'Other'].some(
        (cat) =>
          frEntry.valuesByCategory[cat].join('') === enEntry.valuesByCategory[cat].join('') &&
          !identityExemption(key, frEntry.valuesByCategory[cat].join(' '), allowlist),
      );
      if (!untranslated) continue;
    } else if (frEntry.valuesByBranch && enEntry.valuesByBranch) {
      // Non-plural `match`: compare each ARM by its pattern KEY (`1`, `_`, …), not
      // source order — a locale that reorders arms must still be compared
      // key-against-key so a reordered untranslated branch is caught. An arm whose
      // French value equals English AND carries translatable text is untranslated.
      const branchKeys = new Set([
        ...Object.keys(frEntry.valuesByBranch),
        ...Object.keys(enEntry.valuesByBranch),
      ]);
      const untranslated = [...branchKeys].some((armKey) => {
        const frArm = frEntry.valuesByBranch[armKey] ?? [];
        const enArm = enEntry.valuesByBranch[armKey] ?? [];
        return (
          frArm.length > 0 &&
          // Complete rendered value (fragments concatenated), so a split-literal
          // untranslated arm still compares equal.
          frArm.join('') === enArm.join('') &&
          !identityExemption(key, frArm.join(' '), allowlist)
        );
      });
      if (!untranslated) continue;
    } else if (frEntry.valuesByBlock && enEntry.valuesByBlock) {
      // Non-`match` BRANCHING (`if c { "a" } else { "b" }`): compare each alternative
      // block independently, so an untranslated `else` branch cannot hide behind a
      // translated `if` branch (which a whole-method join would flatten into one value).
      // Fragments WITHIN a block still join (concat semantics); blocks are aligned by
      // index (en/fr share the same branch structure).
      const n = Math.max(frEntry.valuesByBlock.length, enEntry.valuesByBlock.length);
      let untranslated = false;
      for (let i = 0; i < n; i += 1) {
        const frJoined = (frEntry.valuesByBlock[i] ?? []).join('');
        const enJoined = (enEntry.valuesByBlock[i] ?? []).join('');
        if (
          frJoined.length > 0 &&
          frJoined === enJoined &&
          !identityExemption(key, frJoined, allowlist)
        ) {
          untranslated = true;
          break;
        }
      }
      if (!untranslated) continue;
    } else {
      // Non-`match`, non-branching (a single returned value): compare the COMPLETE
      // rendered value (all literal fragments concatenated), not fragment-by-fragment.
      // EN `concat!("Delete ", "account")` and FR `"Delete account"` render
      // byte-identical text but split into a different NUMBER of literals, so a
      // positional compare finds no equal index and misses the untranslated French. A
      // language-neutral value is still cleared by `identityExemption`; an all-empty
      // value (length 0 after join) is a `value-empty` concern, not untranslated.
      const frJoined = frEntry.values.join('');
      const enJoined = enEntry.values.join('');
      const untranslated =
        frJoined.length > 0 && frJoined === enJoined && !identityExemption(key, frJoined, allowlist);
      if (!untranslated) continue;
    }
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
    // Compare COMPLETE rendered branches, not fragment boundaries: an intentionally
    // identical allowlisted value split differently between locales (EN `"Diagnostics"`
    // vs FR `concat!("Diag", "nostics")`) renders the same text, so it must not read as
    // stale. `\u0001` separates branches so a fragment refactor cannot forge a match.
    const rendered = (e) => renderedBranchValues(e).join('\u0001');
    if (rendered(frEntry) !== rendered(enEntry)) {
      findings.push(finding(fr.file, frEntry.line, 'allowlist-stale', `IDENTICAL_ALLOWLIST exempts ${key}, but the values now differ — drop the exemption`, 'catalog'));
    }
  }

  // Rule 4 — French typography over every fr value (localeTag excepted). Check the
  // COMPLETE rendered value of each BRANCH (fragments concatenated), not each literal
  // fragment: `concat!("Bonjour\u{202f}", "!")` renders the required narrow no-break
  // space immediately before `!`, but the `"!"` fragment alone has nothing before it,
  // which a per-fragment check would wrongly flag. Group per plural category / match arm
  // / if-else block (each an ALTERNATIVE rendered string), falling back to the whole
  // value joined for a plain return.
  for (const entry of fr.entries.values()) {
    if (entry.key === 'locale_tag') continue;
    for (const value of renderedBranchValues(entry)) {
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
  // Dioxus's `dangerous_inner_html` injects its string as rendered HTML — its
  // user-visible text (`"<b>Delete account</b>"`) is copy that must be localized,
  // not a structural attribute.
  'dangerous_inner_html',
  'hint',
  'label',
  'optional_label',
  'placeholder',
  // An `iframe` `srcdoc` is an embedded HTML document the browser renders as
  // visible content, so its text (`"<p>Delete account</p>"`) is copy, not a
  // structural attribute — same class as `dangerous_inner_html`.
  'srcdoc',
  'summary',
  'target',
  'title',
  // A control's `value` is its VISIBLE label for `type="submit"`/`"button"`, and
  // hardcoded default text for other inputs — a literal there is copy, not markup.
  'value',
]);

/** Dioxus renders the identifier form `aria_label` as the HTML `aria-label`, so
 *  BOTH spellings reach the DOM. Normalize the underscore ARIA aliases to their
 *  hyphen rendering before matching a copy/reserved attribute — otherwise
 *  `button { aria_label: "…" }` or `div { aria_live: … }` bypasses the gate. Only
 *  the `aria_` prefix is rewritten; a Rust prop like `close_label` keeps its
 *  underscore. */
function normalizeAttrName(attr) {
  return attr === null ? null : attr.replace(/^aria_/, 'aria-');
}

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
  // `role` is VALUE-scoped (`role=<value>`): a primitive owns only the exact role
  // it renders, so an ad-hoc `role: "dialog"` in `live_region.rs` (which owns
  // `role: "status"`) is still flagged. `aria-modal`/`aria-live`/`nav` are owned by
  // NAME (presence): the primitive owns the whole attribute/element it renders.
  ['components/dialog.rs', new Set(['role=dialog', 'aria-modal'])],
  ['components/live_region.rs', new Set(['role=status', 'aria-live'])],
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

/** Letters that survive `{…}` interpolation are real copy. Uses UNICODE letters
 *  (`\p{L}`), not ASCII only: short accented/non-Latin labels — `Été`, `Ça`,
 *  `Удалить`, `删除账户` — are valid rendered copy the scan must not skip (and the
 *  new French surface makes short accented labels common). ANY single letter counts:
 *  a one-character label (`"X"` on a close button, `"A"`, a single CJK glyph) is
 *  rendered to users too; the structural-position checks (attr `:`, call arg, method
 *  receiver) — not a length threshold — are what separate copy from identifiers. */
function bareLetters(text) {
  return /\p{L}/u.test(text.replace(/\{[^}]*\}/g, ' '));
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

/** The index of the ATTRIBUTE COLON governing the expression that contains `pos`, or
 *  `-1` if `pos` sits in a child/argument position instead. Handles a value computed
 *  inline as an `if`/`else`/`match` (`class: if c { "sel" } else { "def" }`): the
 *  literals inside those branch blocks no longer have `:` immediately before them, yet
 *  they belong to the attribute — so a caller can tell a STRUCTURAL attribute's branch
 *  from real markup text. Walks back over balanced groups; an expression block `{` (one
 *  preceded by `if`/`else`/`match`) is stepped through, while an element-body `{`, a
 *  sibling `,`, or a string boundary stops the walk (a child/arg → no attr). */
function enclosingAttrColonIndex(skeleton, pos) {
  let i = pos - 1;
  let guard = 0;
  while (i >= 0 && (guard += 1) < 100000) {
    const c = skeleton[i];
    if (c === '}' || c === ')' || c === ']') {
      const open = c === '}' ? '{' : c === ')' ? '(' : '[';
      let d = 1;
      i -= 1;
      while (i >= 0 && d > 0) {
        if (skeleton[i] === c) d += 1;
        else if (skeleton[i] === open) d -= 1;
        i -= 1;
      }
      continue;
    }
    if (c === ':') return i; // the attribute colon governing this expression
    if (c === '{') {
      // An expression block only if preceded by `if`/`else`/`match` (skip its
      // condition, bounded by the attr `:` / a block edge / a sibling comma). Otherwise
      // it is an element body or a child slot — a boundary.
      let condStart = i - 1;
      while (condStart >= 0 && !'{},;:'.includes(skeleton[condStart])) condStart -= 1;
      const chunk = skeleton.slice(condStart + 1, i);
      if (/\b(?:if|else|match)\b[^{]*$/.test(chunk) || /^\s*else\s*$/.test(chunk)) {
        i = condStart; // continue back past the whole `if <cond>` / `else` / `match <e>`
        continue;
      }
      return -1; // element body / child slot
    }
    if (c === ',' || c === '"') return -1; // sibling attr/child, or a text child
    i -= 1;
  }
  return -1;
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
  if (skeleton[j] === '}') return true;
  // A trailing method chain may CONVERT the call and still close the slot —
  // `{Some("Delete account").unwrap()}` / `{maybe("…").unwrap_or_default()}` is one
  // expression child, so its inner literal is copy, not a call argument to exempt.
  if (skeleton[j] === '.') return methodChainClosesSlot(skeleton, j);
  return false;
}

/** Like [`callIsExpressionChild`], but walks OUTWARD through enclosing constructor/
 *  wrapper calls: a literal nested any number of calls deep inside an
 *  expression-child slot — `div { {Some(String::from("Delete account")).unwrap()} }`
 *  — is still rendered copy. Starting at the literal's immediately-enclosing call
 *  `(`, it asks whether THAT call closes the slot; if not, and the call is itself an
 *  argument to an OUTER call (its callee is preceded by `(`), it repeats on the outer
 *  call. Stops (not copy) when the enclosing callee is preceded by anything else — a
 *  `:` (attr value like `class: foo(bar("x"))`), a `,`/`{` that is not a call, etc. */
function literalCallIsExpressionChild(skeleton, openParenIndex) {
  let paren = openParenIndex;
  for (let guard = 0; guard < 64 && paren >= 0; guard += 1) {
    if (callIsExpressionChild(skeleton, paren)) return true;
    // Walk back over this call's callee (ident / path / macro `!`).
    let i = paren - 1;
    while (i >= 0 && /[\w:!]/.test(skeleton[i])) i -= 1;
    while (i >= 0 && /\s/.test(skeleton[i])) i -= 1;
    // Nested inside an OUTER call — retry against the outer call's `(`.
    if (skeleton[i] === '(') {
      paren = i;
      continue;
    }
    return false;
  }
  return false;
}

/** The name bound at the `=` at `eqIndex`, if it is a `let [mut] <name> = …`,
 *  `let [mut] <name>: <TYPE> = …`, `const <NAME>: <TYPE> = …`, or
 *  `static <NAME>: <TYPE> = …` declaration; else null. All are common ways to hold
 *  copy later interpolated into RSX — including a typed `let`, whose annotation
 *  would otherwise hide the name from the walk-back. The name is an identifier, so
 *  it is not blanked in the skeleton. */
function letBindingName(skeleton, eqIndex) {
  // COMPOUND assignment `NAME += "…"` (`String += &str` appends copy): the char before
  // `=` is `+`, so the plain-word walk-back below yields an empty name. Read the
  // identifier before the operator and return it as the updated binding.
  {
    let k = eqIndex - 1;
    while (k >= 0 && /\s/.test(skeleton[k])) k -= 1;
    if (skeleton[k] === '+') {
      k -= 1;
      while (k >= 0 && /\s/.test(skeleton[k])) k -= 1;
      const cend = k + 1;
      while (k >= 0 && /\w/.test(skeleton[k])) k -= 1;
      const cname = skeleton.slice(k + 1, cend);
      if (cname) return cname;
    }
  }
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
  if (decl) return decl[1];
  // A plain ASSIGNMENT after declaration — `let mut label = String::new(); label =
  // "Delete account";` — still renders that binding as copy. It has a bare name
  // directly before a single `=` (no `let`/type). Comparisons (`==`/`!=`/`<=`/`>=`)
  // leave a non-word char before the `=`, so `name` is empty and they are excluded;
  // `eqIndex + 1 !== '='` also rejects a `==` whose first `=` was landed on.
  if (name && skeleton[eqIndex + 1] !== '=') return name;
  return null;
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

/** Balanced spans of every test-only item — a `#[cfg(test)]`-attributed `mod`/`fn`/
 *  `impl` (balance-matched braces) or a non-braced `use`/`const`/`static` (to its
 *  `;`). Masking the item itself, rather than cutting the file at the first
 *  `#[cfg(test)]`, lets production code that FOLLOWS an inline test module still be
 *  scanned (a later `rsx! { … }` must face the gate). */
function testItemSpans(skeleton) {
  const spans = [];
  const re = /#\[cfg\(test\)\]/g;
  for (let m = re.exec(skeleton); m; m = re.exec(skeleton)) {
    const after = m.index + m[0].length;
    const brace = skeleton.indexOf('{', after);
    const semi = skeleton.indexOf(';', after);
    let end = skeleton.length;
    if (brace !== -1 && (semi === -1 || brace < semi)) {
      // Braced item: balance-match its body so a nested `{}` cannot end it early.
      let depth = 0;
      for (let at = brace; at < skeleton.length; at += 1) {
        if (skeleton[at] === '{') depth += 1;
        else if (skeleton[at] === '}') {
          depth -= 1;
          if (depth === 0) {
            end = at + 1;
            break;
          }
        }
      }
    } else if (semi !== -1) {
      // Non-braced item (`#[cfg(test)] use …;`): ends at its statement `;`.
      end = semi + 1;
    }
    spans.push([m.index, end]);
    re.lastIndex = end;
  }
  return spans;
}

/** Report user-visible literals in one Rust component/app source. */
export function scanComponentLiterals(file, source) {
  const { skeleton, literals, comments } = scanRustSource(source);
  const lines = source.split('\n');
  // Test-only items are not shipped copy — but production code can FOLLOW an inline
  // `#[cfg(test)]` module, so mask each BALANCED test item rather than cutting the
  // file at the first test attribute (which would exempt every later production
  // `rsx!`). `inTest(pos)` is true only INSIDE such a masked item.
  const testSpans = testItemSpans(skeleton);
  const inTest = (pos) => testSpans.some(([s, e]) => pos >= s && pos < e);
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
    if (inTest(literal.start) || !inRsx(literal.start)) continue;
    let at = literal.start - 1;
    while (at >= 0 && /\s/.test(skeleton[at])) at -= 1;
    const prevChar = at >= 0 ? skeleton[at] : '';
    let after = literal.end;
    while (after < skeleton.length && /\s/.test(skeleton[after])) after += 1;
    let isCopy;
    if (prevChar === ':') {
      const attr = attrNameBefore(source, at);
      isCopy = attr !== null && COPY_ATTRS.has(normalizeAttrName(attr));
    } else {
      // A text child: not an attr value, not a call arg / binding, not a method
      // receiver (`"x".to_string()`).
      isCopy = prevChar !== '(' && prevChar !== '=' && prevChar !== '&' && skeleton[after] !== '.';
    }
    if (!isCopy) continue;
    // Capture the interpolated identifier even with a Rust FORMAT SPEC
    // (`{label:>20}`, `{n:.2}`): the spec after `:` does not change that `label` is
    // rendered, so a backing `let label = "…"` must still be caught. `[^{}]*`
    // consumes the spec without crossing a brace.
    for (const m of literal.value.matchAll(/\{(\w+)(?::[^{}]*)?\}/g)) copyInterpolations.add(m[1]);
  }

  // Dioxus SHORTHAND props: `SkipLink { label }` desugars to `label: label`, so a
  // copy-bearing prop written as a bare identifier (between `{`/`,` and `,`/`}`,
  // with no `:`) still flows that identifier into copy. Collect it so a backing
  // `let label = "Skip to rooms"` is caught like the explicit `label: label` form.
  const shorthandRe = /[{,]\s*([A-Za-z_]\w*)\s*(?=[,}])/g;
  for (let m = shorthandRe.exec(skeleton); m; m = shorthandRe.exec(skeleton)) {
    if (inTest(m.index) || !inRsx(m.index)) continue;
    if (COPY_ATTRS.has(normalizeAttrName(m[1]))) copyInterpolations.add(m[1]);
  }

  // EXPLICIT copy props with an IDENTIFIER value: `Banner { label: text }` — `label`
  // is a copy prop, `text` the bare binding that flows into copy. The shorthand pass
  // above collects the prop NAME; here collect the VALUE identifier so a backing
  // `let text = "…"` is caught too. A value that is a call (`label: helper()`), a
  // string, or a path is not a bare identifier before `,`/`}`, so it is not matched.
  const explicitPropRe = /([A-Za-z_]\w*)\s*:\s*([A-Za-z_]\w*)\s*(?=[,}])/g;
  for (let m = explicitPropRe.exec(skeleton); m; m = explicitPropRe.exec(skeleton)) {
    if (inTest(m.index) || !inRsx(m.index)) continue;
    if (COPY_ATTRS.has(normalizeAttrName(m[1]))) copyInterpolations.add(m[2]);
  }

  // Dioxus braced EXPRESSION CHILDREN: `div { {message} }` renders the binding as a
  // text node, so a `let message = "…"` behind it is copy REGARDLESS of the
  // variable's name (it need not be a copy-bearing prop like `label`). Collect the
  // identifier when the `{ident}` sits in CHILD position — its `{` follows the
  // element body `{`, a sibling child's `}`, or a sibling string (blanked to `"` in
  // the skeleton) — but NOT an attribute value (`attr: {x}`, preceded by `:`) or a
  // component's own prop brace (`Comp { ident }`, whose `{` follows a name — that
  // shorthand is handled above).
  const braceIdentRe = /\{\s*([A-Za-z_]\w*)\s*\}/g;
  for (let m = braceIdentRe.exec(skeleton); m; m = braceIdentRe.exec(skeleton)) {
    if (inTest(m.index) || !inRsx(m.index)) continue;
    let p = m.index - 1;
    while (p >= 0 && /\s/.test(skeleton[p])) p -= 1;
    const pc = p >= 0 ? skeleton[p] : '';
    if (pc === '{' || pc === '}' || pc === '"') copyInterpolations.add(m[1]);
  }

  // Propagate copy-flow through ALIAS bindings: `let label = base;` (RHS a bare
  // identifier). If the alias NAME is copy-bearing, so is its SOURCE binding — and
  // transitively along a chain — so a `let base = "…"` behind the alias is still
  // caught. Iterate to a fixpoint over `let NAME = IDENT;` (the RHS is a bare ident, so
  // a string/call/path RHS — including the literal itself — never matches).
  // The alias SOURCE: a bare identifier, optionally through an identity-preserving
  // conversion (`base`, `base.to_string()`, `base.to_owned()`, `base.clone()`,
  // `base.into()`, `base.as_str()`) — all still render the same copy. Captured as the
  // second group; the statement ends at `;`.
  const aliasRhs =
    '([A-Za-z_]\\w*)\\s*(?:\\.\\s*(?:to_string|to_owned|clone|into|as_str)\\s*\\(\\s*\\))?\\s*;';
  const aliasRes = [
    new RegExp(`\\blet\\s+(?:mut\\s+)?([A-Za-z_]\\w*)\\s*(?::\\s*[^=;{}]*)?=\\s*${aliasRhs}`, 'g'),
    // Plain REASSIGNMENT `label = base;`, statement-anchored after `;`/`{`/`}` so a
    // comparison/compound `=` (`==`, `+=`) is not matched.
    new RegExp(`[;{}]\\s*([A-Za-z_]\\w*)\\s*=\\s*${aliasRhs}`, 'g'),
  ];
  for (let grew = true; grew; ) {
    grew = false;
    for (const re of aliasRes) {
      re.lastIndex = 0;
      for (let m = re.exec(skeleton); m; m = re.exec(skeleton)) {
        if (inTest(m.index)) continue;
        if (copyInterpolations.has(m[1]) && !copyInterpolations.has(m[2])) {
          copyInterpolations.add(m[2]);
          grew = true;
        }
      }
    }
  }

  // Copy HELPER functions: a literal-returning fn INVOKED in an RSX copy position
  // (`div { {helper()} }`, `label: helper()`) renders its body's literal as copy,
  // even though that literal lives outside RSX and is never assigned. Collect the
  // invoked helper names, resolve each to its `fn` body, and treat bare-letter
  // literals inside as copy. A helper invoked only in a STRUCTURAL position
  // (`id: id_for()`) is not collected, so its body stays exempt.
  // Each invoked helper is a {name, receiver}: a QUALIFIED call `Recv::helper()` renders
  // the SAME literal, but must resolve ONLY Recv's method — resolving every `fn helper`
  // globally by terminal name would mark an unrelated `Other::helper` body as copy and
  // reject its structural literal. `receiver` is the segment before the terminal (null
  // for a bare call).
  const copyHelpers = new Map(); // key "receiver\0name" -> {name, receiver}
  // `Self`/`self`/`crate`/`super` are context-relative path qualifiers, not a
  // distinct named type/module, so they resolve globally (like a bare call) — a
  // `Self::helper()` on a free or inherent fn still traces. Only a CONCRETE
  // receiver (`A` vs `B`) scopes resolution to its own impl/mod.
  const PSEUDO_RECEIVERS = new Set(['Self', 'self', 'crate', 'super']);
  const parsePath = (path) => {
    const segs = path.split('::').map((s) => s.trim()).filter(Boolean);
    const name = segs[segs.length - 1];
    let receiver = segs.length >= 2 ? segs[segs.length - 2] : null;
    if (receiver && PSEUDO_RECEIVERS.has(receiver)) receiver = null;
    return { name, receiver };
  };
  const addHelper = (path) => {
    const { name, receiver } = parsePath(path);
    if (name) copyHelpers.set(`${receiver ?? ''}\0${name}`, { name, receiver });
  };
  // Expression-CHILD calls: `{ helper() }` / `{ Recv::helper() }` — the `{` follows the
  // element body `{`, a sibling child's `}`, or a sibling string (blanked to `"`).
  const childCallRe = /[{}"]\s*\{\s*((?:[A-Za-z_]\w*\s*::\s*)*[A-Za-z_]\w*)\s*\(/g;
  for (let m = childCallRe.exec(skeleton); m; m = childCallRe.exec(skeleton)) {
    const at = m.index + m[0].lastIndexOf('{');
    if (inTest(at) || !inRsx(at)) continue;
    addHelper(m[1]);
  }
  // Copy-ATTRIBUTE call values: `label: helper()` / `label: Recv::helper()`.
  const attrCallRe = /([A-Za-z_]\w*)\s*:\s*((?:[A-Za-z_]\w*\s*::\s*)*[A-Za-z_]\w*)\s*\(/g;
  for (let m = attrCallRe.exec(skeleton); m; m = attrCallRe.exec(skeleton)) {
    if (inTest(m.index) || !inRsx(m.index)) continue;
    if (COPY_ATTRS.has(normalizeAttrName(m[1]))) addHelper(m[2]);
  }
  // Whether the `fn` starting at `fnPos` lives inside an `impl`/`mod` block whose header
  // names `receiver` — so `Recv::name` resolves only Recv's method. A bare call
  // (`receiver === null`) matches any `fn name`.
  const scopeMatches = (fnPos, receiver) => {
    if (!receiver) return true;
    let depth = 0;
    for (let i = fnPos - 1; i >= 0; i -= 1) {
      const c = skeleton[i];
      if (c === '}') {
        depth += 1;
      } else if (c === '{') {
        if (depth === 0) {
          let s = i - 1;
          while (s >= 0 && !'{};'.includes(skeleton[s])) s -= 1;
          const header = skeleton.slice(s + 1, i);
          return new RegExp(`\\b(?:impl|mod)\\b[^{]*\\b${receiver}\\b`).test(header);
        }
        depth -= 1;
      }
    }
    return false;
  };
  // Resolve each helper to its `fn NAME(…) -> … { BODY }` body span (scoped by
  // receiver), TRANSITIVELY: a copy helper that calls another helper renders that
  // callee's literal too, so follow the calls each resolved body makes. `visited` (keyed
  // by receiver+name) bounds it and breaks cycles; unresolved names are harmless no-ops.
  const helperBodies = [];
  const visited = new Set();
  const pending = [...copyHelpers.values()];
  while (pending.length > 0) {
    const { name, receiver } = pending.pop();
    const key = `${receiver ?? ''}\0${name}`;
    if (visited.has(key)) continue;
    visited.add(key);
    const defRe = new RegExp(`\\bfn\\s+${name}\\s*\\(`, 'g');
    for (let m = defRe.exec(skeleton); m; m = defRe.exec(skeleton)) {
      if (!scopeMatches(m.index, receiver)) continue;
      let i = m.index + m[0].length - 1; // at the opening '('
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
      while (i < skeleton.length && skeleton[i] !== '{') i += 1;
      const close = matchingBrace(skeleton, i);
      if (close !== -1) {
        helperBodies.push([i, close]);
        const body = skeleton.slice(i, close);
        const bodyCallRe = /((?:[A-Za-z_]\w*\s*::\s*)*[A-Za-z_]\w*)\s*\(/g;
        for (let c = bodyCallRe.exec(body); c; c = bodyCallRe.exec(body)) {
          const parsed = parsePath(c[1]);
          if (parsed.name && !visited.has(`${parsed.receiver ?? ''}\0${parsed.name}`)) {
            pending.push(parsed);
          }
        }
      }
    }
  }
  const inCopyHelperBody = (pos) => helperBodies.some(([open, close]) => pos > open && pos < close);

  // A control's `value` is copy only when USER-VISIBLE (a submit/button label, a text
  // input's default). For `type="checkbox"/"radio"/"hidden"` it is SUBMITTED DATA that
  // must stay stable across locales — not copy. Returns true for those structural cases
  // by reading the enclosing element's `type` attribute from the source.
  const structuralValue = (attr, colonPos) => {
    if (normalizeAttrName(attr) !== 'value') return false;
    let e = colonPos;
    let depth = 0;
    while (e >= 0) {
      const c = skeleton[e];
      if (c === '}') depth += 1;
      else if (c === '{') {
        if (depth === 0) break;
        depth -= 1;
      }
      e -= 1;
    }
    if (e < 0) return false;
    const bclose = matchingBrace(skeleton, e);
    const body = source.slice(e, bclose === -1 ? source.length : bclose);
    const tm = /\btype\s*:\s*"([^"]*)"/.exec(body);
    return tm !== null && /^(?:checkbox|radio|hidden)$/i.test(tm[1]);
  };

  for (const literal of literals) {
    if (inTest(literal.start)) continue;
    // Only literals inside RSX markup are copy candidates; Rust logic literals
    // (class-name match arms, `let` bindings, `format!` args) are not — EXCEPT a
    // literal inside a copy-helper body, which is rendered as copy via its call.
    if (!inRsx(literal.start)) {
      if (
        inCopyHelperBody(literal.start) &&
        bareLetters(literal.value) &&
        !exempt(lineOf(source, literal.start))
      ) {
        findings.push(finding(file, lineOf(source, literal.start), 'rust-text', `copy returned by a helper is not in the catalog: ${literal.value.trim().slice(0, 60)}`, 'literals'));
      }
      continue;
    }
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
      if (attr && COPY_ATTRS.has(normalizeAttrName(attr)) && !structuralValue(attr, before)) {
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
      // Peel EVERY constructor layer: `hint: Some(String::from("…"))` nests the
      // literal in two calls, so keep walking back `ident(` wrappers until the prop
      // `:` (or a non-wrapper) is reached — stopping after one layer let a
      // double-wrapped copy value slip the gate. The callee token includes `!` so a
      // MACRO wrapper — `label: format!("Delete {name}")` — is peeled too (its `!`
      // would otherwise halt the walk before the prop colon and exempt the copy).
      let colon = before; // sits on the innermost '('
      while (skeleton[colon] === '(') {
        let ident = colon - 1;
        while (ident >= 0 && /[A-Za-z0-9_:!]/.test(skeleton[ident])) ident -= 1;
        while (ident >= 0 && /\s/.test(skeleton[ident])) ident -= 1;
        colon = ident; // char before the callee: another '(' peels again, ':' stops
      }
      if (skeleton[colon] === ':') {
        const attr = attrNameBefore(source, colon);
        if (attr && COPY_ATTRS.has(normalizeAttrName(attr))) {
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
    // A literal that is the argument of a call which IS (or is nested inside) the RSX
    // expression child — `div { {format!("Delete account")} }`, or a constructor deeper
    // in `div { {Some(String::from("Delete account")).unwrap()} }` — is visible copy (a
    // rendered text node), not Rust logic. Trace OUTWARD through the enclosing calls to
    // the whole `{ … }` slot before treating the argument as code; a `class:
    // format!("app-{}", p)` attr value or a nested arg under a non-child call stays exempt.
    if (prev === '(' && literalCallIsExpressionChild(skeleton, before)) {
      findings.push(finding(file, line, 'rust-text', `RSX text expression is not in the catalog: ${literal.value.trim().slice(0, 60)}`, 'literals'));
      continue;
    }
    // A literal that is a function/macro argument is likewise Rust code.
    if (prev === '(' || prev === '=' || prev === '&') continue;

    // A literal inside an attribute value computed inline as `if`/`else`/`match`
    // (`class: if selected { "selected-state" } else { "default-state" }`) belongs to
    // that attribute — its branch blocks just lost the immediate `:`. A STRUCTURAL
    // attribute's branch is not markup text (do not flag it); a COPY attribute's branch
    // is copy (flag it). Only a TRUE child position (no governing attr colon) falls to
    // the text finding below.
    const attrColon = enclosingAttrColonIndex(skeleton, literal.start);
    if (attrColon !== -1) {
      const attr = attrNameBefore(source, attrColon);
      if (attr && COPY_ATTRS.has(normalizeAttrName(attr))) {
        findings.push(finding(file, line, 'copy-attribute', `${attr} takes a literal, not a catalog message: ${literal.value.slice(0, 60)}`, 'literals'));
      }
      continue;
    }

    findings.push(finding(file, line, 'rust-text', `RSX text is not in the catalog: ${literal.value.trim().slice(0, 60)}`, 'literals'));
  }

  // A hardcoded literal ASSIGNED to a `let` binding that is then interpolated into
  // RSX copy (`let label = "Delete account"; div { "{label}" }`) lives OUTSIDE the
  // rsx! range, so the range-only scan above misses it. Flag such a binding when
  // its name is interpolated in a copy position. A catalog-derived binding
  // (`let x = strings.foo()`) has no string-literal RHS, so it is never matched.
  for (const literal of literals) {
    if (inTest(literal.start) || inRsx(literal.start)) continue; // in-RSX handled above
    if (!bareLetters(literal.value)) continue;
    let at = literal.start - 1;
    while (at >= 0 && /\s/.test(skeleton[at])) at -= 1;
    const prevChar = at >= 0 ? skeleton[at] : '';
    // The literal is a `let` binding's RHS either DIRECTLY (`let x = "…"`) or
    // WRAPPED in a constructor (`let x = String::from("…")` / `format!("…")`) —
    // both create the `String` later interpolated as copy. For the wrapped case,
    // walk back over the callee to the `=`.
    let eqIndex = -1;
    let mutatedBinding = null;
    if (prevChar === '=') {
      eqIndex = at;
    } else if (prevChar === '(') {
      // A constructor-wrapped RHS: `= String::from("…")` / `= format!("…")`.
      let i = at - 1;
      while (i >= 0 && /[\w:!]/.test(skeleton[i])) i -= 1; // callee ident / path / `!`
      while (i >= 0 && /\s/.test(skeleton[i])) i -= 1;
      if (skeleton[i] === '=') eqIndex = i;
    }
    // A MUTATING method appends copy to a binding (`label.push_str("…")`,
    // `label.insert_str(0, "…")`, `label.replace_range(.., "…")`). The literal is a
    // call ARG in ANY position, so walk back to the ENCLOSING `(` (over prior args at
    // depth 0) and check the callee: a `<receiver>.<mutator>` associates the literal
    // with the RECEIVER binding, traced like assigned copy.
    if (eqIndex < 0) {
      let i = literal.start - 1;
      let depth = 0;
      while (i >= 0) {
        const c = skeleton[i];
        if (c === ')' || c === ']' || c === '}') depth += 1;
        else if (c === '(' || c === '[' || c === '{') {
          if (depth === 0) break;
          depth -= 1;
        }
        i -= 1;
      }
      if (i >= 0 && skeleton[i] === '(') {
        let c = i - 1;
        while (c >= 0 && /\s/.test(skeleton[c])) c -= 1;
        const calleeEnd = c + 1;
        while (c >= 0 && /\w/.test(skeleton[c])) c -= 1;
        const callee = skeleton.slice(c + 1, calleeEnd);
        while (c >= 0 && /\s/.test(skeleton[c])) c -= 1;
        if (skeleton[c] === '.' && /^(?:push_str|push|insert_str|replace_range)$/.test(callee)) {
          let r = c - 1;
          while (r >= 0 && /\s/.test(skeleton[r])) r -= 1;
          const recvEnd = r + 1;
          while (r >= 0 && /\w/.test(skeleton[r])) r -= 1;
          mutatedBinding = skeleton.slice(r + 1, recvEnd);
        }
      }
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
    if (eqIndex < 0 && !mutatedBinding) continue;
    const boundName = mutatedBinding ?? letBindingName(skeleton, eqIndex);
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
    // PRESENCE ownership (`aria-live`/`aria-modal`): the whole attribute is exempt
    // in a file that owns it. `role` is not name-owned — it is checked per value.
    if (owned.has(attr)) continue;
    // Match BOTH spellings of an ARIA attribute: the HTML hyphen form
    // (`aria-live`, quoted in RSX) AND Dioxus's identifier alias (`aria_live:`),
    // which renders identically — otherwise the underscore form bypasses the
    // reserved-attribute gate. `\s*:\s*` reaches the value for the per-value check.
    const pat = attr.replace(/-/g, '[-_]');
    const re = new RegExp(`(?:\\b${pat}\\b|"${pat}")\\s*:\\s*`, 'g');
    for (let m = re.exec(source); m; m = re.exec(source)) {
      if (inTest(m.index) || !inRsx(m.index) || inComment(m.index)) continue;
      // VALUE-scoped ownership: a primitive owns only the exact `role` it renders
      // (`role=status`), so an ad-hoc `role: "dialog"` in that file is still flagged.
      const valMatch = /^"([^"]*)"/.exec(source.slice(m.index + m[0].length));
      if (valMatch && owned.has(`${attr}=${valMatch[1]}`)) continue;
      const line = lineOf(source, m.index);
      // NOTE: NO `exempt(line)` here. `i18n-exempt` clears only literal-COPY findings;
      // a reserved semantic (Decision-6) must NOT be silenceable by a copy exemption,
      // or a diagnostic comment would disable accessibility enforcement on the line.
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
      if (inTest(m.index) || !inRsx(m.index)) continue;
      const line = lineOf(source, m.index);
      // NO `exempt(line)`: a reserved semantic element is not copy — see the note above.
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
      // Skip test-only Fields: the control loop skips their `input` via `inTest`, so a
      // seeded count would stay 0 and falsely report `form-control-count` on valid
      // component-test markup.
      if (inTest(m.index)) continue;
      const open = m.index + m[0].length - 1;
      const close = matchingBrace(skeleton, open);
      if (close !== -1) fieldRanges.push([open, close]);
    }
  }
  // Extract the FIRST `id:` attribute value in `source[from..to)`, or null. The
  // value is a quoted literal (`"email"`) OR an EXPRESSION (`field_id.clone()`,
  // `make_id("email", suffix)`) — an expression-valued id must not slip the
  // mismatch check, so read the COMPLETE BALANCED expression: track `()`/`[]`/`{}`
  // depth and quotes (with backslash escapes) so a top-level `,`/`}` terminates but
  // a comma NESTED in a call does not. Compared as raw text (a literal keeps its
  // quotes, so a literal id and an expression id never spuriously match).
  // `source[from..to)` with every comment range blanked, so a commented `//
  // id: "email"` / `// hint: Some(…)` is not read as live markup (the id / hint
  // association must compare real attributes only — the exclusion the
  // reserved-attribute and nav paths already apply).
  const commentMaskedSlice = (from, to) => {
    let slice = source.slice(from, to);
    for (const { start, end } of comments) {
      if (end <= from || start >= to) continue;
      const a = Math.max(start, from) - from;
      const b = Math.min(end, to) - from;
      slice = slice.slice(0, a) + ' '.repeat(b - a) + slice.slice(b);
    }
    return slice;
  };
  const firstAttrValue = (from, to, headSrc) => {
    const slice = commentMaskedSlice(from, to);
    const head = new RegExp(headSrc).exec(slice);
    if (!head) return null;
    const startVal = head.index + head[0].length;
    let depth = 0;
    let quote = null;
    let i = startVal;
    for (; i < slice.length; i += 1) {
      const c = slice[i];
      if (quote) {
        if (c === '\\') i += 1; // skip the escaped char (incl. an escaped quote)
        else if (c === quote) quote = null;
        continue;
      }
      if (c === '"' || c === "'") quote = c;
      else if (c === '(' || c === '[' || c === '{') depth += 1;
      else if (c === ')' || c === ']') depth -= 1;
      else if (depth === 0 && (c === ',' || c === '}')) break;
    }
    return slice.slice(startVal, i).trim() || null;
  };
  // The FIRST `id:` value (quoted literal or a balanced expression like
  // `field_id.clone()` / `make_id("email", suffix)`), OR the field-init SHORTHAND
  // `id`. `Field { id, … }` is `id: id`, so the Field's id is the variable `id`;
  // without recognizing the shorthand, `firstAttrValue` returns null and the
  // label[for] / control-id mismatch check is skipped, leaving the control unnamed
  // while the gate passes. A bare `id` prop (followed by `,`/`}`, not `:`) resolves
  // to the identifier `id`.
  const firstIdAttr = (from, to) => {
    const explicit = firstAttrValue(from, to, '\\bid\\s*:\\s*');
    if (explicit !== null) return explicit;
    return /\bid\s*(?:,|\})/.test(commentMaskedSlice(from, to)) ? 'id' : null;
  };
  // The `aria-describedby` value (quoted hyphen name OR the `aria_describedby`
  // underscore alias Dioxus renders identically), read as a balanced expression.
  const describedbyValue = (from, to) =>
    firstAttrValue(from, to, '(?:"aria-describedby"|aria[-_]describedby)\\s*:\\s*');
  // How many controls sit inside each `Field` range (keyed by its open index): a
  // Field renders ONE `label[for]`, so ZERO controls leave that label naming nothing
  // and TWO produce duplicate ids and an ambiguous label — both flagged after the
  // loop. Seed EVERY detected Field at 0 so a control-less Field is not skipped for
  // want of an entry.
  const controlsPerField = new Map();
  for (const [open] of fieldRanges) controlsPerField.set(open, 0);
  for (const el of RESERVED_FORM_CONTROLS) {
    const re = new RegExp(`\\b${el}\\s*\\{`, 'g');
    for (let m = re.exec(skeleton); m; m = re.exec(skeleton)) {
      if (inTest(m.index) || !inRsx(m.index)) continue;
      const line = lineOf(source, m.index);
      // NO `exempt(line)`: a raw form control is a semantic gate, not copy (see note above).
      const field = fieldRanges.find(([open, close]) => m.index > open && m.index < close);
      if (!field) {
        // Not inside any `Field` → a raw, unlabelled control.
        findings.push(finding(file, line, 'raw-form-control', `raw \`${el}\` must be wrapped by the \`Field\` primitive (§5.6) for label association, not rendered ad-hoc (Decision-6)`, 'literals'));
        continue;
      }
      controlsPerField.set(field[0], (controlsPerField.get(field[0]) ?? 0) + 1);
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
      // When the Field supplies a HINT, the hint span is rendered OUTSIDE the
      // label with id `{id}-hint`, so the control must reference it via
      // `aria-describedby` or the hint is never exposed to assistive tech (§5.6).
      // A LITERAL Field id is checked exactly; an EXPRESSION id renders a dynamic
      // `{id}-hint`, so its association must be dynamic AND reference the Field id's
      // own base identifier — a literal or an expression naming a DIFFERENT id fails.
      // Comment-masked so a commented `// hint: Some(catalog_help)` does not read
      // as a real hint and falsely demand `aria-describedby` on a valid control.
      const fieldProps = commentMaskedSlice(field[0], controlOpen);
      const hasHint = /\bhint\s*:\s*(?!None\b)/.test(fieldProps);
      if (fieldId !== null && hasHint) {
        const describedby = describedbyValue(controlOpen, controlEnd);
        const literalId = /^"([^"]*)"$/.exec(fieldId);
        const describedbyLiteral = describedby === null ? null : /^"([^"]*)"$/.exec(describedby);
        if (literalId) {
          const expected = `${literalId[1]}-hint`;
          // `aria-describedby` is a space-separated ID REFERENCE LIST, so a control may
          // legitimately reference the hint AND another description
          // (`"email-hint email-error"`). Require MEMBERSHIP of the expected hint id,
          // not that it is the sole value.
          const describedbyIds =
            describedbyLiteral === null ? [] : describedbyLiteral[1].trim().split(/\s+/);
          if (!describedbyLiteral || !describedbyIds.includes(expected)) {
            findings.push(finding(file, line, 'form-control-hint-unassociated', `\`${el}\` inside a \`Field\` with a hint must set \`aria-describedby: "${expected}"\` so the hint is exposed as a description; found \`${describedby === null ? '(none)' : describedby}\``, 'literals'));
          }
        } else if (describedby === null || describedbyLiteral) {
          // An EXPRESSION Field id renders a DYNAMIC hint id (`{id}-hint`), so the
          // association must itself be dynamic: a LITERAL `aria-describedby` (or a
          // missing one) can never reference the runtime id.
          const found = describedbyLiteral ? `literal \`${describedby}\`` : 'no attribute';
          findings.push(finding(file, line, 'form-control-hint-unassociated', `\`${el}\` inside a \`Field\` with an expression \`id\` and a hint must set \`aria-describedby\` to a DYNAMIC value referencing the Field's \`{id}-hint\`; ${found} cannot name the runtime hint id`, 'literals'));
        } else {
          // A dynamic `aria-describedby` under an expression id must reference the
          // Field id's COMPLETE derivation, not merely its first identifier:
          // `id: ids.email.clone()` with `aria_describedby: format!("{}-hint", ids.other)`
          // share `ids` but name DIFFERENT hints. Compare the full access path with
          // any trailing conversion calls (`.clone()`/`.into()`/`.to_string()`…)
          // stripped, so `ids.email` must appear in the describedby verbatim.
          const idCore = fieldId.replace(/(\s*\.\s*\w+\s*\(\s*\))+\s*$/, '').trim();
          // The dynamic value must CONSTRUCT `{idCore}-hint`, not merely contain
          // idCore: require the `-hint` suffix AND idCore as a WHOLE reference — so
          // `aria_describedby: ids.email.clone()` (the id itself, no `-hint`) and a
          // prefix like `ids.email_backup` both fail.
          const escaped = idCore.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
          // The `-hint` suffix must apply to a PLACEHOLDER/interpolation close
          // (`}-hint`), so `format!("{}-hint", ids.email)` and
          // `format!("{ids.email}-hint")` pass, but `format!("wrong-hint {}", ids.email)`
          // — which contains `-hint` and `ids.email` INDEPENDENTLY — does not. And
          // idCore must appear as a whole reference (the interpolated value / arg).
          const namesHint =
            describedby.includes('}-hint') && new RegExp(`${escaped}(?![\\w.])`).test(describedby);
          if (idCore && !namesHint) {
            findings.push(finding(file, line, 'form-control-hint-unassociated', `\`${el}\`'s \`aria-describedby\` (\`${describedby}\`) must construct the Field id's own \`{id}-hint\` (from \`${idCore}\` + \`-hint\`); it does not, so the hint is unassociated`, 'literals'));
          }
        }
      }
    }
  }
  // A `Field` renders exactly one `label[for="{id}"]`; two controls in one Field
  // both set that id, so the DOM has DUPLICATE ids and the label resolves
  // ambiguously (the second control ends up unnamed). Enforce one control per Field.
  for (const [open, count] of controlsPerField) {
    if (count !== 1) {
      findings.push(finding(file, lineOf(source, open), 'form-control-count', `a \`Field\` must wrap exactly ONE control; found ${count} — ${count === 0 ? 'its `label[for]` names no control' : 'duplicate ids make its `label[for]` ambiguous and leave the extra control(s) unnamed'} (§5.6)`, 'literals'));
    }
  }

  // `nav` must be a NAMED landmark, and only the NavLandmark primitive (which
  // OWNS `nav`) may render a bare one. Elsewhere, flag a `nav { … }` whose body
  // carries no accessible name (`aria-label`/`aria-labelledby`) — the app shell's
  // named nav is legitimate, an unnamed one bypasses the named-navigation contract.
  if (!owned.has('nav')) {
    const navRe = /\bnav\s*\{/g;
    for (let m = navRe.exec(skeleton); m; m = navRe.exec(skeleton)) {
      if (inTest(m.index) || !inRsx(m.index)) continue;
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
      // Require a REAL accessible-name attribute, not a substring: `data-aria-label`
      // must not satisfy it. Anchor the leading edge (no `-`/word char before `aria`,
      // so `data-aria-label` is rejected) and require a trailing `:` (allowing the
      // `aria_label` alias and an optional closing quote of `"aria-label"`).
      // Require a NON-EMPTY accessible name: `aria_label: ""` sets the attribute
      // but names nothing (and its empty literal has no letters, so the copy scan is
      // silent too). The value must be a NON-EMPTY quoted string (`"[^"]+"`) OR a
      // non-string expression (`[^"\s]`); an empty `""` matches neither and is
      // treated as unnamed. (Matching the value explicitly, not a `(?!"")` lookahead
      // whose `\s*` would backtrack to pass at the space before the value.)
      const namesTheNav = /(?<![-\w])aria[-_]label(?:ledby)?"?\s*:\s*(?:"[^"]+"|[^"\s])/.test(attrs);
      // A statically-ABSENT optional value (`aria_label: None` / `None::<String>`)
      // renders NO attribute — Dioxus drops a `None`-valued attribute — so it does NOT
      // name the nav, even though it is a (non-string) expression.
      const absentName = /(?<![-\w])aria[-_]label(?:ledby)?"?\s*:\s*None\b/.test(attrs);
      if (namesTheNav && !absentName) continue;
      const line = lineOf(source, m.index);
      // NO `exempt(line)`: an unnamed nav is a semantic gate, not copy (see note above).
      findings.push(finding(file, line, 'raw-semantic-element', 'raw unnamed `nav` must carry an accessible name (aria-label/aria-labelledby) or come from the NavLandmark primitive (Decision-6)', 'literals'));
    }
  }
  return findings;
}

export function componentFiles(repoRoot) {
  const files = [];
  // The locale catalog files are checked by the catalog rules, not the literal scan.
  const excluded = new Set(Object.values(LOCALE_FILES).map((p) => resolve(repoRoot, p)));
  const walk = (absolute) => {
    for (const entry of readdirSync(absolute, { withFileTypes: true }).sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))) {
      const path = resolve(absolute, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (/\.rs$/.test(entry.name) && !excluded.has(path)) files.push(path);
    }
  };
  for (const root of LITERAL_SCAN_ROOTS) {
    const absolute = resolve(repoRoot, root);
    if (!existsSync(absolute)) continue;
    if (/\.rs$/.test(root)) {
      if (!excluded.has(absolute)) files.push(absolute);
    } else {
      walk(absolute);
    }
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
