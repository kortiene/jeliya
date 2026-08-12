// Unit tests for the Dioxus catalog gate (#177). Mirrors
// scripts/check-ui-i18n.test.mjs: fixture catalogs exercise each RULE without
// depending on today's real copy, plus a smoke test that the actual tree is
// clean so a real regression fails this suite too.
//
// Run: node --test scripts/check-jeliya-ui-i18n.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';

import {
  checkCatalogs,
  checkJeliyaUiI18n,
  identityExemption,
  parseCatalog,
  scanComponentLiterals,
} from './lib/jeliya-ui-catalog.mjs';

const EN = `
impl Catalog for En {
    fn locale_tag(&self) -> &'static str { "en" }
    fn hello(&self) -> &'static str { "Hello" }
    fn greet(&self, name: &str) -> String { format!("Hi {name}") }
    fn pct(&self, n: &str) -> String { format!("{n}%") }
    fn items(&self, n: &str, category: PluralCategory) -> String {
        match category {
            PluralCategory::One => format!("{n} item"),
            PluralCategory::Other => format!("{n} items"),
        }
    }
}
`;

const FR_GOOD = `
impl Catalog for Fr {
    fn locale_tag(&self) -> &'static str { "fr" }
    fn hello(&self) -> &'static str { "Bonjour" }
    fn greet(&self, name: &str) -> String { format!("Salut {name}") }
    fn pct(&self, n: &str) -> String { format!("{n}\\u{202f}%") }
    fn items(&self, n: &str, category: PluralCategory) -> String {
        match category {
            PluralCategory::One => format!("{n} article"),
            PluralCategory::Other => format!("{n} articles"),
        }
    }
}
`;

function catalogs(enSrc, frSrc) {
  return {
    en: { file: 'en.rs', entries: parseCatalog(enSrc, 'en.rs').entries },
    fr: { file: 'fr.rs', entries: parseCatalog(frSrc, 'fr.rs').entries },
  };
}

test('parseCatalog extracts keys, plural-ness, and decoded values', () => {
  const { entries } = parseCatalog(EN, 'en.rs');
  assert.deepEqual([...entries.keys()], ['locale_tag', 'hello', 'greet', 'pct', 'items']);
  assert.equal(entries.get('items').isPlural, true);
  assert.equal(entries.get('hello').isPlural, false);
  assert.equal(entries.get('hello').values[0], 'Hello');
});

test('a well-formed EN/FR pair produces no findings', () => {
  const { en, fr } = catalogs(EN, FR_GOOD);
  assert.deepEqual(checkCatalogs({ en, fr, allowlist: {} }), []);
});

test('rule 1: a missing key in fr is reported', () => {
  const frMissing = FR_GOOD.replace(/fn hello\(&self\) -> &'static str \{ "Bonjour" \}\n/, '');
  const { en, fr } = catalogs(EN, frMissing);
  const codes = checkCatalogs({ en, fr, allowlist: {} }).map((f) => f.code);
  assert.ok(codes.includes('key-missing'));
});

test('rule 2: an empty value is reported', () => {
  const frEmpty = FR_GOOD.replace('"Bonjour"', '""');
  const { en, fr } = catalogs(EN, frEmpty);
  const codes = checkCatalogs({ en, fr, allowlist: {} }).map((f) => f.code);
  assert.ok(codes.includes('value-empty'));
});

test('rule 3: a French value left in English is reported, and the allowlist exempts it', () => {
  const frUntranslated = FR_GOOD.replace('"Bonjour"', '"Hello"');
  const { en, fr } = catalogs(EN, frUntranslated);
  assert.ok(checkCatalogs({ en, fr, allowlist: {} }).some((f) => f.code === 'fr-untranslated'));
  // An explicit allowlist entry silences it…
  assert.ok(!checkCatalogs({ en, fr, allowlist: { hello: 'kept identical on purpose' } }).some((f) => f.code === 'fr-untranslated'));
});

test('rule 3 stale side: an allowlist entry that no longer exempts anything is reported', () => {
  const { en, fr } = catalogs(EN, FR_GOOD); // hello differs, so the exemption is stale
  const findings = checkCatalogs({ en, fr, allowlist: { hello: 'stale' } });
  assert.ok(findings.some((f) => f.code === 'allowlist-stale'));
});

test('rule 3: the never-translate lexicon exempts a legitimately identical value', () => {
  assert.equal(identityExemption('x', 'Hello'), null);
  assert.ok(identityExemption('x', 'Jeliya'));
  assert.ok(identityExemption('x', 'Agent'));
});

test('rule 4: a plain space before % is flagged; U+202F passes', () => {
  const frBadPercent = FR_GOOD.replace('format!("{n}\\u{202f}%")', 'format!("{n} %")');
  const { en, fr } = catalogs(EN, frBadPercent);
  const codes = checkCatalogs({ en, fr, allowlist: {} }).map((f) => f.code);
  assert.ok(codes.includes('fr-narrow-space'));
  // The unmodified good catalog (U+202F) does not trip it.
  const clean = catalogs(EN, FR_GOOD);
  assert.ok(!checkCatalogs(clean).some((f) => f.code === 'fr-narrow-space'));
});

test('rule 4: MISSING French spacing is flagged (not only wrong spacing)', () => {
  // `Bonjour!` with no space before the mark, and `Bonjour:` with no space
  // before the colon, must both be caught — the gate hole a wrong-space-only
  // rule leaves open.
  const frBang = FR_GOOD.replace('"Bonjour"', '"Bonjour!"');
  assert.ok(
    checkCatalogs({ ...catalogs(EN, frBang), allowlist: {} }).some((f) => f.code === 'fr-narrow-space'),
    'no space before ! must trip fr-narrow-space',
  );
  const frColon = FR_GOOD.replace('"Bonjour"', '"Bonjour:"');
  assert.ok(
    checkCatalogs({ ...catalogs(EN, frColon), allowlist: {} }).some((f) => f.code === 'fr-no-break-space'),
    'no space before : must trip fr-no-break-space',
  );
});

test('rule 4: a straight apostrophe and three-dot ellipsis are flagged', () => {
  const frBad = FR_GOOD.replace('"Bonjour"', `"l'ami..."`);
  const { en, fr } = catalogs(EN, frBad);
  const codes = new Set(checkCatalogs({ en, fr, allowlist: {} }).map((f) => f.code));
  assert.ok(codes.has('fr-apostrophe'));
  assert.ok(codes.has('fr-ellipsis'));
});

test('rule: placeholder parity — a dropped/renamed format slot is flagged', () => {
  // EN's `pct` interpolates {n}; an FR that drops it still compiles in Rust
  // (unused arg) but must be caught.
  const frNoSlot = FR_GOOD.replace('format!("{n}\\u{202f}%")', 'format!("pourcent")');
  assert.ok(
    checkCatalogs({ ...catalogs(EN, frNoSlot), allowlist: {} }).some((f) => f.code === 'placeholder-parity'),
    'an fr value dropping the {n} slot must trip placeholder-parity',
  );
  // The unmodified good catalog (matching slots) does not trip it.
  assert.ok(!checkCatalogs(catalogs(EN, FR_GOOD)).some((f) => f.code === 'placeholder-parity'));
});

test('rule 5: a plural method reduced to one arm is reported', () => {
  const frOneArm = FR_GOOD.replace(
    /fn items[\s\S]*?\n    \}\n/,
    'fn items(&self, n: &str, category: PluralCategory) -> String { format!("{n} article") }\n',
  );
  const { en, fr } = catalogs(EN, frOneArm);
  assert.ok(checkCatalogs({ en, fr, allowlist: {} }).some((f) => f.code === 'plural-parity'));
});

test('literal scan: an RSX text node and a copy attribute are flagged', () => {
  const source = `
fn view() -> Element {
    rsx! {
        div { class: "x", role: "dialog", "Hello world" }
        img { "aria-label": "Delete account" }
    }
}
`;
  const codes = scanComponentLiterals('x.rs', source).map((f) => f.code);
  assert.ok(codes.includes('rust-text'));
  assert.ok(codes.includes('copy-attribute'));
});

test('literal scan: a hardcoded component copy PROP is flagged, structural props are not', () => {
  // The bypass the wrong-set gate left open: `label`/`close_label`/`target` are
  // component props carrying user-visible copy, so a literal on them belongs in
  // the catalog; `anchor`/`id`/`class` are structural and must stay clean.
  const source = `
fn view() -> Element {
    rsx! {
        SkipLink { anchor: "rooms-nav", label: "Skip to rooms" }
        Dialog { close_label: "Close" }
        BootScreen { target: "Connecting" }
    }
}
`;
  const codes = scanComponentLiterals('x.rs', source).map((f) => f.code);
  assert.equal(codes.filter((c) => c === 'copy-attribute').length, 3, 'label + close_label + target');
  // The structural `anchor`/`id`/`class` literals must NOT be flagged.
  assert.ok(
    !scanComponentLiterals('x.rs', source).some((f) => /anchor|rooms-nav/.test(f.text ?? '')),
    'structural props stay clean',
  );
});

test('literal scan: a copy-prop literal with a trailing .to_string() is still flagged', () => {
  // `.to_string()`/`.into()` is the normal spelling for a String prop, so it
  // must NOT exempt a hardcoded copy prop — while a STRUCTURAL prop stays clean.
  const source = `
fn view() -> Element {
    rsx! {
        SkipLink { anchor: "rooms-nav".to_string(), label: "Skip to rooms".to_string() }
    }
}
`;
  const codes = scanComponentLiterals('x.rs', source).map((f) => f.code);
  assert.equal(codes.filter((c) => c === 'copy-attribute').length, 1, 'label flagged, anchor not');
});

test('literal scan: interpolation, structural attrs, and format! args are clean', () => {
  const source = `
fn view(greeting: String) -> Element {
    let _ = format!("room.list: {}", greeting);
    rsx! {
        div { class: "sidebar", id: "main", role: "dialog", tabindex: "-1", "{greeting}" }
        a { class: "skip-link", href: "#main", "aria-label": "{greeting}" }
    }
}
`;
  assert.deepEqual(scanComponentLiterals('x.rs', source), []);
});

test('literal scan: an i18n-exempt line is honored', () => {
  const source = `
fn view() -> Element {
    rsx! {
        // i18n-exempt: developer-only diagnostic string
        div { "Raw developer text" }
    }
}
`;
  assert.deepEqual(scanComponentLiterals('x.rs', source), []);
});

test('the real jeliya-ui tree is clean across all groups', () => {
  const findings = checkJeliyaUiI18n({});
  assert.deepEqual(
    findings,
    [],
    `the actual catalogs/components must be clean:\n${findings.map((f) => `  ${f.file}:${f.line} [${f.code}] ${f.message}`).join('\n')}`,
  );
});
