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
  // compose.rs is production UI code too — it carries the web and native `rsx!`
  // roots that mount `AppRoot`, so hardcoded copy there (an adapter-specific error
  // banner, say) ships to users and must face the same gate.
  'crates/jeliya-ui/src/compose.rs',
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
    // CHAR literal (`'x'`, `'\n'`, `'\''`, `'}'`) — distinct from a LIFETIME
    // (`'a`, `'static`, which is NOT closed by a `'`). Blank its content so a
    // delimiter inside it (`'}'`, `'{'`, `'"'`) is not counted as macro/RSX
    // structure or mistaken for a string start.
    if (ch === "'") {
      if (source[i + 1] === '\\') {
        let j = i + 2;
        while (j < source.length && source[j] !== "'" && source[j] !== '\n') j += 1;
        if (source[j] === "'") {
          blank(i, j + 1);
          i = j + 1;
          continue;
        }
      } else if (i + 2 < source.length && source[i + 2] === "'") {
        blank(i, i + 3);
        i += 3;
        continue;
      }
      // Otherwise a lifetime — leave it untouched.
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
  // Include ONE-letter words: dropping them lets `"A daemon"` tokenize to just
  // `daemon` and, if that is a never-translate token, be auto-exempted from the
  // untranslated check even though `A` is translatable copy. A value with no
  // letters at all still yields `[]` (a language-neutral value stays exempt).
  return (
    text
      .normalize('NFD')
      .replace(/[̀-ͯ]/g, '')
      .toLowerCase()
      .match(/[a-z]+/g) ?? []
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
        /(?:=>|\{)\s*(?:String::(?:new|default)|Default::default)\s*\(\s*\)/g;
      while (emptyCtorRe.exec(returned) !== null) emptyBranchCount += 1;
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
      const catRe = /PluralCategory::(One|Other)\b|\b_\s*=>/g;
      for (let m = catRe.exec(bodyText); m; m = catRe.exec(bodyText)) {
        raw.push({ at: open + m.index, cat: m[1] ?? null });
        if (m[1]) explicitCats.add(m[1]);
      }
      const wildcardCat = explicitCats.has('One') ? 'Other' : 'One';
      const markers = raw.map((r) => ({ at: r.at, cat: r.cat ?? wildcardCat }));
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
          const groups = [[]]; // [0] = base (literals in no block)
          const slotGroups = [[]]; // parallel: the slot multiset per block
          for (const p of parts) {
            let idx = 0;
            for (let b = 0; b < blockRanges.length; b += 1) {
              if (p.start >= blockRanges[b][0] && p.end <= blockRanges[b][1]) {
                idx = b + 1;
                break;
              }
            }
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
  // A value with a PLACEHOLDER renders content (`format!("{n}")` shows a count like
  // `42`), so it is NOT empty — only a value with no slot AND no non-whitespace
  // text is. Deleting the slot sentinel before the emptiness check (the old
  // behaviour) wrongly flagged a placeholder-only message as `value-empty`.
  const isEmptyValue = (v) => !v.includes(SLOT) && v.trim() === '';
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
  // Dioxus's `dangerous_inner_html` injects its string as rendered HTML — its
  // user-visible text (`"<b>Delete account</b>"`) is copy that must be localized,
  // not a structural attribute.
  'dangerous_inner_html',
  'hint',
  'label',
  'optional_label',
  'placeholder',
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
 *  new French surface makes short accented labels common). */
function bareLetters(text) {
  return /\p{L}{2,}/u.test(text.replace(/\{[^}]*\}/g, ' '));
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
    for (const m of literal.value.matchAll(/\{(\w+)\}/g)) copyInterpolations.add(m[1]);
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

  // Copy HELPER functions: a literal-returning fn INVOKED in an RSX copy position
  // (`div { {helper()} }`, `label: helper()`) renders its body's literal as copy,
  // even though that literal lives outside RSX and is never assigned. Collect the
  // invoked helper names, resolve each to its `fn` body, and treat bare-letter
  // literals inside as copy. A helper invoked only in a STRUCTURAL position
  // (`id: id_for()`) is not collected, so its body stays exempt.
  const copyHelpers = new Set();
  // Expression-CHILD calls: `{ helper() }` — its `{` follows the element body `{`,
  // a sibling child's `}`, or a sibling string (blanked to `"`). A QUALIFIED path
  // (`{Self::helper()}`, `{copy::helper()}`) renders its terminal function's literal
  // just the same, so match an optional `Foo::` path prefix and capture the TERMINAL
  // name to resolve (a bare-identifier-only match let qualified helpers ship copy).
  const childCallRe = /[{}"]\s*\{\s*(?:[A-Za-z_]\w*\s*::\s*)*([A-Za-z_]\w*)\s*\(/g;
  for (let m = childCallRe.exec(skeleton); m; m = childCallRe.exec(skeleton)) {
    const at = m.index + m[0].lastIndexOf('{');
    if (inTest(at) || !inRsx(at)) continue;
    copyHelpers.add(m[1]);
  }
  // Copy-ATTRIBUTE call values: `label: helper()` / `label: Self::helper()` (a
  // copy-bearing prop) — likewise resolve the terminal function of a qualified path.
  const attrCallRe = /([A-Za-z_]\w*)\s*:\s*(?:[A-Za-z_]\w*\s*::\s*)*([A-Za-z_]\w*)\s*\(/g;
  for (let m = attrCallRe.exec(skeleton); m; m = attrCallRe.exec(skeleton)) {
    if (inTest(m.index) || !inRsx(m.index)) continue;
    if (COPY_ATTRS.has(normalizeAttrName(m[1]))) copyHelpers.add(m[2]);
  }
  // Resolve each helper name to its `fn NAME(…) -> … { BODY }` body span — skip the
  // balanced parameter list, then take the first `{` (the body) and its match — and do
  // so TRANSITIVELY: a copy helper that calls another helper (`fn outer() { inner() }`)
  // renders that callee's literal too, so trace the calls each resolved body makes
  // (bare or qualified terminal name) and follow them. A `visited` set bounds it and
  // breaks cycles; names with no matching `fn` (std ctors, macros, keywords) resolve to
  // nothing and are harmless no-ops.
  const helperBodies = [];
  const visited = new Set();
  const pending = [...copyHelpers];
  while (pending.length > 0) {
    const name = pending.pop();
    if (visited.has(name)) continue;
    visited.add(name);
    const defRe = new RegExp(`\\bfn\\s+${name}\\s*\\(`, 'g');
    for (let m = defRe.exec(skeleton); m; m = defRe.exec(skeleton)) {
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
        // Follow calls this body makes so a helper that delegates to another helper's
        // literal is still traced (terminal name of a possibly-qualified path).
        const body = skeleton.slice(i, close);
        const bodyCallRe = /(?:[A-Za-z_]\w*\s*::\s*)*([A-Za-z_]\w*)\s*\(/g;
        for (let c = bodyCallRe.exec(body); c; c = bodyCallRe.exec(body)) {
          if (!visited.has(c[1])) pending.push(c[1]);
        }
      }
    }
  }
  const inCopyHelperBody = (pos) => helperBodies.some(([open, close]) => pos > open && pos < close);

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
      if (attr && COPY_ATTRS.has(normalizeAttrName(attr))) {
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
      if (inTest(m.index) || !inRsx(m.index)) continue;
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
  // `field_id.clone()` / `make_id("email", suffix)`).
  const firstIdAttr = (from, to) => firstAttrValue(from, to, '\\bid\\s*:\\s*');
  // The `aria-describedby` value (quoted hyphen name OR the `aria_describedby`
  // underscore alias Dioxus renders identically), read as a balanced expression.
  const describedbyValue = (from, to) =>
    firstAttrValue(from, to, '(?:"aria-describedby"|aria[-_]describedby)\\s*:\\s*');
  // How many controls sit inside each `Field` range (keyed by its open index): a
  // Field renders ONE `label[for]`, so two controls sharing the Field's id produce
  // duplicate ids and an ambiguous label — flagged after the loop.
  const controlsPerField = new Map();
  for (const el of RESERVED_FORM_CONTROLS) {
    const re = new RegExp(`\\b${el}\\s*\\{`, 'g');
    for (let m = re.exec(skeleton); m; m = re.exec(skeleton)) {
      if (inTest(m.index) || !inRsx(m.index)) continue;
      const line = lineOf(source, m.index);
      if (exempt(line)) continue;
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
          if (!describedbyLiteral || describedbyLiteral[1] !== expected) {
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
    if (count > 1) {
      findings.push(finding(file, lineOf(source, open), 'form-control-duplicate', `a \`Field\` must wrap exactly ONE control; found ${count} — duplicate ids make its \`label[for]\` ambiguous and leave the extra control(s) unnamed (§5.6)`, 'literals'));
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
      if (/(?<![-\w])aria[-_]label(?:ledby)?"?\s*:\s*(?:"[^"]+"|[^"\s])/.test(attrs)) continue;
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
