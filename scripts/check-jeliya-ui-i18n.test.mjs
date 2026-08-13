// Unit tests for the Dioxus catalog gate (#177). Mirrors
// scripts/check-ui-i18n.test.mjs: fixture catalogs exercise each RULE without
// depending on today's real copy, plus a smoke test that the actual tree is
// clean so a real regression fails this suite too.
//
// Run: node --test scripts/check-jeliya-ui-i18n.test.mjs

import { readFileSync } from 'node:fs';

import test from 'node:test';
import assert from 'node:assert/strict';

import {
  checkCatalogs,
  checkJeliyaUiI18n,
  identityExemption,
  parseCatalog,
  scanComponentLiterals,
  slotsEqual,
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

test('rule 2: a plural with ONE empty arm is reported (not only all-empty)', () => {
  // `One => ""` with a nonempty Other renders blank for that count.
  const frOneEmpty = FR_GOOD.replace('format!("{n} article")', '""');
  assert.ok(
    checkCatalogs({ ...catalogs(EN, frOneEmpty), allowlist: {} }).some((f) => f.code === 'value-empty'),
    'an empty single plural arm must trip value-empty',
  );
});

test('rule: placeholder parity is PER ARM, not pooled', () => {
  // Pooled slots would let One dropping {n} hide behind Other doubling it. Drop
  // {n} from the fr One arm only: EN One has {n}, fr One has none → per-arm flag.
  const frArm = FR_GOOD.replace('format!("{n} article")', 'format!("article")');
  assert.ok(
    checkCatalogs({ ...catalogs(EN, frArm), allowlist: {} }).some((f) => f.code === 'placeholder-parity'),
    'a per-arm slot mismatch must trip placeholder-parity',
  );
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
        div { class: "x", "Hello world" }
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

test('literal scan: a copy prop wrapped in Some(...).to_string() is flagged', () => {
  // `hint: Some("…".to_string())` / `optional_label: Some("…".into())` are the
  // normal spellings for Option<String> copy props and must not be exempted as
  // function arguments.
  const source = `
fn view() -> Element {
    rsx! {
        Field { id: "pw".to_string(), label: "Password".to_string(),
                hint: Some("Use eight characters".to_string()) }
    }
}
`;
  const codes = scanComponentLiterals('x.rs', source).map((f) => f.code);
  assert.ok(codes.filter((c) => c === 'copy-attribute').length >= 2, 'label + hint flagged');
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
        div { class: "sidebar", id: "main", tabindex: "-1", "{greeting}" }
        a { class: "skip-link", href: "#main", "aria-label": "{greeting}" }
    }
}
`;
  assert.deepEqual(scanComponentLiterals('x.rs', source), []);
});

test('literal scan: a raw reserved-semantic attr outside a primitive is flagged', () => {
  // A raw `role`/`aria-live`/`aria-modal` must come from a shared primitive
  // (Decision-6); an ad-hoc one bypasses focus containment / announce-once.
  const source = `
fn view() -> Element {
    rsx! {
        div { role: "dialog", "aria-modal": "true", "x" }
        div { "aria-live": "polite" }
    }
}
`;
  const codes = scanComponentLiterals('rogue.rs', source).map((f) => f.code);
  assert.ok(codes.includes('raw-semantic'), 'raw role/aria-live must be flagged');

  // A primitive is exempt ONLY for the constructs it owns, matched by EXACT path
  // suffix. `components/dialog.rs` owns role + aria-modal, so those are allowed…
  const inDialog = scanComponentLiterals('components/dialog.rs', source);
  assert.ok(
    !inDialog.some((f) => f.code === 'raw-semantic' && /role|aria-modal/.test(f.message)),
    'dialog.rs may define role/aria-modal',
  );
  // …but it does NOT own aria-live, so an ad-hoc one there is still flagged.
  assert.ok(
    inDialog.some((f) => f.code === 'raw-semantic' && /aria-live/.test(f.message)),
    'an aria-live in dialog.rs (which does not own it) must still be flagged',
  );
  // A same-basename file in another directory is NOT exempt.
  assert.ok(
    scanComponentLiterals('components/legacy/dialog.rs', source).some((f) => f.code === 'raw-semantic'),
    'a legacy/dialog.rs must not inherit the primitive exemption',
  );
});

test('literal scan: a converted RSX text expression is flagged', () => {
  // `div { {"Delete account".to_string()} }` renders hardcoded copy as a text
  // node; the trailing `.to_string()` must not exempt it (the converted-child
  // bypass). A statement block with a trailing `;` is NOT an expression child.
  const flagged = `
fn view() -> Element {
    rsx! {
        div { {"Delete account".to_string()} }
    }
}
`;
  const codes = scanComponentLiterals('x.rs', flagged).map((f) => f.code);
  assert.ok(codes.includes('rust-text'), 'a converted text expression child must be flagged');

  // A genuine Rust statement (trailing `;`) inside a block is NOT a text child.
  const notFlagged = `
fn view() -> Element {
    rsx! {
        button { onclick: move |_| { let _ = "log".to_string(); }, {greeting} }
    }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', notFlagged).some((f) => f.code === 'rust-text'),
    'a converted string used as a Rust statement must NOT be flagged as copy',
  );
});

test('literal scan: a format! RSX expression child is flagged', () => {
  // `div { {format!("Delete account")} }` renders hardcoded copy via a macro
  // call that IS the expression child; a `class:`/nested-call format! is not.
  const flagged = `
fn view() -> Element {
    rsx! {
        div { {format!("Delete account")} }
    }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', flagged).some((f) => f.code === 'rust-text'),
    'a format! expression child must be flagged',
  );

  // A `format!` used as a structural attribute VALUE (callee preceded by `:`) is
  // not copy — the class is not user-visible text.
  const attrValue = `
fn view() -> Element {
    rsx! {
        div { class: format!("app pane-{}", pane), {body} }
    }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', attrValue).some((f) => f.code === 'rust-text'),
    'a format! attribute value must NOT be flagged as copy',
  );
});

test('placeholder-parity: plural arms are keyed by category, not source order', () => {
  // One uses {a}, Other uses {b}. EN lists One first; FR lists Other first.
  const en = `impl Catalog for En {
    fn n(&self, c: PluralCategory) -> String {
      match c {
        PluralCategory::One => format!("{a} thing"),
        PluralCategory::Other => format!("{b} things"),
      }
    }
  }`;
  const frReordered = `impl Catalog for Fr {
    fn n(&self, c: PluralCategory) -> String {
      match c {
        PluralCategory::Other => format!("{b} choses"),
        PluralCategory::One => format!("{a} chose"),
      }
    }
  }`;
  const check = (frSrc) =>
    checkCatalogs({
      en: parseCatalog(en, 'en.rs'),
      fr: parseCatalog(frSrc, 'fr.rs'),
      allowlist: {},
    }).filter((f) => f.code === 'placeholder-parity');
  // Reordered arms with matching per-category slots must NOT trip parity
  // (a source-index comparison would misalign fr Other against en One).
  assert.deepEqual(check(frReordered), [], 'reordered plural arms must compare by category');
  // A genuine per-category slot drop IS still caught.
  const frDropsOne = frReordered.replace('{a} chose', 'chose');
  assert.ok(check(frDropsOne).length > 0, 'an fr One dropping {a} must trip placeholder-parity');
});

test('placeholder-parity: slotsEqual compares element-wise, not by join', () => {
  // Distinct multisets must NOT collide: `['a','bc']` and `['ab','c']` both
  // join to `'abc'`, so a delimiter-free join would call them equal and miss a
  // renamed/dropped slot.
  assert.equal(slotsEqual(['a', 'bc'], ['ab', 'c']), false);
  assert.equal(slotsEqual(['n'], ['n']), true);
  assert.equal(slotsEqual(['n'], ['n', 'n']), false);
  assert.equal(slotsEqual([], []), true);
});

test('literal scan: a let-bound literal interpolated into RSX copy is flagged', () => {
  // The literal lives OUTSIDE rsx! but renders as copy via `{label}`.
  const flagged = `
fn view() -> Element {
    let label = "Delete account";
    rsx! { div { "{label}" } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', flagged).some((f) => f.code === 'rust-text'),
    'a let-bound literal rendered as RSX copy must be flagged',
  );

  // A CONSTRUCTOR-wrapped binding (`String::from(...)`, `format!(...)`) is also
  // flagged — both create the String later interpolated as copy.
  for (const rhs of ['String::from("Delete account")', 'format!("Delete account")']) {
    const wrapped = `
fn view() -> Element {
    let label = ${rhs};
    rsx! { div { "{label}" } }
}
`;
    assert.ok(
      scanComponentLiterals('x.rs', wrapped).some((f) => f.code === 'rust-text'),
      `a constructor-wrapped copy binding must be flagged: ${rhs}`,
    );
  }

  // A `const`/`static`-bound copy literal interpolated into RSX is also flagged.
  for (const decl of ['const LABEL: &str = "Delete account";', 'static LABEL: &str = "Delete account";']) {
    const constBound = `
${decl}
fn view() -> Element {
    rsx! { div { "{LABEL}" } }
}
`;
    assert.ok(
      scanComponentLiterals('x.rs', constBound).some((f) => f.code === 'rust-text'),
      `a ${decl.split(' ')[0]}-bound copy literal must be flagged`,
    );
  }

  // A TYPED `let` binding (with an annotation, optional `mut`) — the type sits
  // between the name and `=`, so the plain-word walk-back would otherwise land on
  // the type and miss the binding.
  for (const decl of [
    'let label: &str = "Delete account";',
    'let mut label: String = String::from("Delete account");',
  ]) {
    const typedLet = `
fn view() -> Element {
    ${decl}
    rsx! { div { "{label}" } }
}
`;
    assert.ok(
      scanComponentLiterals('x.rs', typedLet).some((f) => f.code === 'rust-text'),
      `a typed let-bound copy literal must be flagged: ${decl}`,
    );
  }

  // CONDITIONALLY-assigned copy: the literals sit inside an `if/else` in the
  // binding RHS (preceded by `{`, not `=`), so they must still be associated with
  // the interpolated binding.
  const conditional = `
fn view() -> Element {
    let label = if destructive { "Delete account" } else { "Remove account" };
    rsx! { div { "{label}" } }
}
`;
  assert.equal(
    scanComponentLiterals('x.rs', conditional).filter((f) => f.code === 'rust-text').length,
    2,
    'both conditional-copy literals must be flagged',
  );
  // The same shape with catalog-derived arms (no string literals) is clean.
  const conditionalCatalog = `
fn view() -> Element {
    let label = if destructive { strings.err_unknown_title() } else { strings.rooms_heading() };
    rsx! { div { "{label}" } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', conditionalCatalog).some((f) => f.code === 'rust-text'),
    'a conditional catalog-derived binding must not be flagged',
  );

  // A catalog-derived binding (no string-literal RHS) is NOT flagged.
  const catalogDerived = `
fn view() -> Element {
    let label = strings.rooms_heading().to_string();
    rsx! { div { "{label}" } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', catalogDerived).some((f) => f.code === 'rust-text'),
    'a catalog-derived binding must not be flagged',
  );

  // A binding interpolated ONLY into a STRUCTURAL attribute (id/class) is not copy.
  const structural = `
fn view() -> Element {
    let anchor = "rooms-nav";
    rsx! { div { id: "{anchor}" } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', structural).some((f) => f.code === 'rust-text'),
    'a binding used only in a structural attribute must not be flagged',
  );
});

test('literal scan: a raw form control outside a Field is flagged, inside is not', () => {
  // A bare `input`/`textarea`/`select` bypasses the Field primitive's label
  // association (Decision-6, §5.6).
  for (const el of ['input', 'textarea', 'select']) {
    const rogue = `
fn view() -> Element {
    rsx! { div { ${el} { id: "email" } } }
}
`;
    assert.ok(
      scanComponentLiterals('rogue.rs', rogue).some((f) => f.code === 'raw-form-control'),
      `a raw ${el} outside a Field must be flagged`,
    );
  }
  // The SAME control wrapped by `Field { … }` (which owns label association) is
  // legitimate — Field renders the control as children.
  const wrapped = `
fn view() -> Element {
    rsx! { Field { id: "email", label: "Email", input { id: "email" } } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', wrapped).some((f) => f.code === 'raw-form-control'),
    'a control inside a Field invocation must not be flagged',
  );
  // A matching id is fully clean (no raw-form-control AND no id-mismatch).
  assert.deepEqual(
    scanComponentLiterals('x.rs', wrapped).filter((f) =>
      /raw-form-control|form-control-id-mismatch/.test(f.code),
    ),
    [],
    'a control whose id matches the Field must be clean',
  );
  // A control inside a Field whose `id` does NOT match the Field's id — the
  // generated `label[for]` would name nothing — is flagged.
  const mismatch = `
fn view() -> Element {
    rsx! { Field { id: "email", label: "Email", input { id: "other" } } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', mismatch).some((f) => f.code === 'form-control-id-mismatch'),
    'a control whose id mismatches the Field must be flagged',
  );
  // A control inside a Field with NO id at all is likewise flagged.
  const missing = `
fn view() -> Element {
    rsx! { Field { id: "email", label: "Email", input { r#type: "text" } } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', missing).some((f) => f.code === 'form-control-id-mismatch'),
    'a control with no id inside a Field must be flagged',
  );
  // EXPRESSION-valued ids (a reusable field): different expressions mismatch,
  // identical ones are clean — an expression id must not slip the check.
  const exprMismatch = `
fn view() -> Element {
    rsx! { Field { id: field_id.clone(), label: "Email", input { id: other_id.clone() } } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', exprMismatch).some((f) => f.code === 'form-control-id-mismatch'),
    'differing expression-valued ids must be flagged',
  );
  const exprMatch = `
fn view() -> Element {
    rsx! { Field { id: field_id.clone(), label: "Email", input { id: field_id.clone() } } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', exprMatch).some((f) => f.code === 'form-control-id-mismatch'),
    'matching expression-valued ids must be clean',
  );
  // A comma NESTED inside a call must not truncate the id expression: differing
  // arguments after the nested comma are a real mismatch.
  const nestedComma = `
fn view() -> Element {
    rsx! { Field { id: make_id("email", a), label: "Email", input { id: make_id("email", b) } } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', nestedComma).some((f) => f.code === 'form-control-id-mismatch'),
    'ids differing only after a nested comma must be flagged (balanced parse)',
  );
  const nestedCommaMatch = `
fn view() -> Element {
    rsx! { Field { id: make_id("email", a), label: "Email", input { id: make_id("email", a) } } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', nestedCommaMatch).some((f) => f.code === 'form-control-id-mismatch'),
    'identical nested-comma id expressions must be clean',
  );
  // When a Field supplies a HINT, the control must reference `{id}-hint` via
  // aria-describedby or the hint is not exposed to assistive tech.
  const hintMissing = `
fn view() -> Element {
    rsx! { Field { id: "email", label: "Email", hint: "we never share it", input { id: "email" } } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', hintMissing).some((f) => f.code === 'form-control-hint-unassociated'),
    'a hinted Field whose control omits aria-describedby must be flagged',
  );
  const hintWrong = `
fn view() -> Element {
    rsx! { Field { id: "email", label: "Email", hint: "x", input { id: "email", "aria-describedby": "other" } } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', hintWrong).some((f) => f.code === 'form-control-hint-unassociated'),
    'a control pointing aria-describedby elsewhere must be flagged',
  );
  const hintOk = `
fn view() -> Element {
    rsx! { Field { id: "email", label: "Email", hint: "x", input { id: "email", "aria-describedby": "email-hint" } } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', hintOk).some((f) => f.code === 'form-control-hint-unassociated'),
    'a control referencing {id}-hint must be clean',
  );
  // A Field with NO hint needs no aria-describedby.
  const noHint = `
fn view() -> Element {
    rsx! { Field { id: "email", label: "Email", input { id: "email" } } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', noHint).some((f) => f.code === 'form-control-hint-unassociated'),
    'a hintless Field needs no aria-describedby',
  );
});

test('literal scan: an unnamed nav named only in a comment is flagged', () => {
  // A comment mentioning `aria-label` before the nav's first child must NOT be
  // read as a real accessible name.
  const commented = `
fn view() -> Element {
    rsx! {
        nav {
            // aria-label: supplied later
            a { href: "#", "Home" }
        }
    }
}
`;
  assert.ok(
    scanComponentLiterals('shell.rs', commented).some(
      (f) => f.code === 'raw-semantic-element' && /nav/.test(f.message),
    ),
    'a nav named only inside a comment must still be flagged as unnamed',
  );
  // A real aria-label attribute on the nav is not flagged.
  const named = `
fn view() -> Element {
    rsx! { nav { "aria-label": "Primary", a { href: "#", "Home" } } }
}
`;
  assert.ok(
    !scanComponentLiterals('shell.rs', named).some((f) => f.code === 'raw-semantic-element'),
    'a nav with a real aria-label must not be flagged',
  );
});

test('literal scan: a reserved attr in a COMMENT is not flagged; a real one is', () => {
  // A comment documenting `role:`/`aria-live:` renders no attribute — the scan
  // must exclude comment ranges even though it matches the raw source (to catch a
  // quoted `"aria-live"`).
  const commented = `
fn view() -> Element {
    rsx! {
        // role: dialog semantics belong in the Dialog primitive
        div { /* aria-live: polite is owned by the status primitive */ "x" }
    }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', commented).some((f) => f.code === 'raw-semantic'),
    'a reserved attr named only inside a comment must not be flagged',
  );
  // A real (uncommented) reserved attr in the same shape is still flagged.
  const real = `
fn view() -> Element {
    rsx! { div { role: "dialog", "x" } }
}
`;
  assert.ok(
    scanComponentLiterals('rogue.rs', real).some((f) => f.code === 'raw-semantic'),
    'a real raw role attr must still be flagged',
  );
});

test('literal scan: a bare semantic ELEMENT outside a primitive is flagged', () => {
  // A bare `dialog { … }` bypasses the Dialog primitive's focus/Escape contract;
  // an UNNAMED `nav { … }` bypasses the named-navigation contract.
  const source = `
fn view() -> Element {
    rsx! {
        dialog { "{message}" }
        nav { "rooms" }
    }
}
`;
  const codes = scanComponentLiterals('rogue.rs', source).map((f) => f.code);
  assert.ok(codes.includes('raw-semantic-element'), 'a bare dialog/unnamed nav must be flagged');
  // The NavLandmark primitive OWNS the `nav` element, so a bare nav is allowed
  // there — but the `dialog` ELEMENT is owned by NO file (Dialog renders `div
  // role="dialog"`), so it is flagged EVERYWHERE, including components/dialog.rs.
  const inNav = scanComponentLiterals('components/nav.rs', source);
  assert.ok(
    !inNav.some((f) => f.code === 'raw-semantic-element' && /nav/.test(f.message)),
    'nav.rs may render a bare nav element',
  );
  assert.ok(
    inNav.some((f) => f.code === 'raw-semantic-element' && /dialog/.test(f.message)),
    'the dialog element is owned by no primitive and is flagged even in nav.rs',
  );
  assert.ok(
    scanComponentLiterals('components/dialog.rs', source).some(
      (f) => f.code === 'raw-semantic-element' && /dialog/.test(f.message),
    ),
    'a bare dialog element is flagged even in components/dialog.rs (Dialog uses div role=dialog)',
  );
});

test('literal scan: a NAMED nav landmark is not flagged', () => {
  // A `nav` carrying an accessible name is the landmark contract, so the app
  // shell's named nav must pass (no false positive).
  const source = `
fn view() -> Element {
    rsx! {
        nav {
            class: "rooms-list",
            "aria-label": "{rooms_label}",
            div { "child" }
        }
    }
}
`;
  assert.ok(
    !scanComponentLiterals('app.rs', source).some((f) => f.code === 'raw-semantic-element'),
    'a named nav landmark must not be flagged',
  );

  // A nav that is itself UNNAMED but contains a named DESCENDANT (e.g. an icon
  // button) is still flagged — the descendant's name does not name the landmark.
  const descendantNamed = `
fn view() -> Element {
    rsx! {
        nav {
            class: "rooms-list",
            button { "aria-label": "{close_label}", "x" }
        }
    }
}
`;
  assert.ok(
    scanComponentLiterals('rogue.rs', descendantNamed).some(
      (f) => f.code === 'raw-semantic-element' && /nav/.test(f.message),
    ),
    'an unnamed nav with a named descendant must still be flagged',
  );
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

test('literal scan: Unicode-letter copy (accents, CJK) is recognized as UI text', () => {
  // An ASCII-only `{2,}` predicate silently skips short accented / non-Latin copy
  // ("Été", "删除账户"), especially on the French surface — Unicode letters count.
  for (const copy of ['Été', 'Ça', 'Удалить', '删除账户']) {
    const source = `
fn view() -> Element {
    rsx! { div { "${copy}" } }
}
`;
    assert.ok(
      scanComponentLiterals('fr.rs', source).some((f) => f.code === 'rust-text'),
      `Unicode copy must be flagged: ${copy}`,
    );
  }
});

test('literal scan: a Dioxus aria_label underscore alias is a copy attribute', () => {
  // Dioxus renders `aria_label` as `aria-label`, so the underscore alias carrying a
  // hardcoded accessible name must be flagged like the quoted hyphen spelling.
  const source = `
fn view() -> Element {
    rsx! { button { aria_label: "Delete account" } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', source).some((f) => f.code === 'copy-attribute'),
    'aria_label (underscore alias) carrying copy must be flagged',
  );
});

test('literal scan: Dioxus aria_live/aria_modal underscore aliases are reserved', () => {
  // Those aliases render `aria-live`/`aria-modal`, so ad-hoc markup using them must
  // still trip the reserved-attribute gate (Decision-6) it would bypass by spelling.
  for (const attr of ['aria_live', 'aria_modal']) {
    const source = `
fn view() -> Element {
    rsx! { div { ${attr}: "polite", "x" } }
}
`;
    assert.ok(
      scanComponentLiterals('rogue.rs', source).some((f) => f.code === 'raw-semantic'),
      `${attr} (renders ${attr.replace('_', '-')}) in ad-hoc markup must be flagged`,
    );
  }
});

test('placeholder-parity: a wildcard `_` plural arm is classified as the remaining category', () => {
  // EN carries {n} only in the explicit One arm; FR carries it only in a wildcard
  // `_ => …` arm (its Other). Without wildcard classification the FR wildcard
  // literal inherits the One marker, so both categories spuriously balance and the
  // per-category placeholder mismatch is missed.
  const en = `impl Catalog for En {
    fn chosen(&self, n: &str, c: PluralCategory) -> String {
      match c {
        PluralCategory::One => format!("{n} chosen"),
        PluralCategory::Other => format!("none chosen"),
      }
    }
  }`;
  const fr = `impl Catalog for Fr {
    fn chosen(&self, n: &str, c: PluralCategory) -> String {
      match c {
        PluralCategory::One => format!("aucun choisi"),
        _ => format!("{n} choisis"),
      }
    }
  }`;
  assert.ok(
    checkCatalogs({ en: parseCatalog(en, 'en.rs'), fr: parseCatalog(fr, 'fr.rs'), allowlist: {} }).some(
      (f) => f.code === 'placeholder-parity',
    ),
    'a {n} misplaced into a wildcard Other arm must trip per-category parity',
  );
});

test('literal scan: a copy prop double-wrapped in nested constructors is flagged', () => {
  // `hint: Some(String::from("…"))` nests the literal in TWO constructors; the
  // walk-back must peel both to the prop colon, or the copy slips as a call arg.
  const source = `
fn view() -> Element {
    rsx! {
        Field { id: "pw".to_string(), label: "Password".to_string(),
                hint: Some(String::from("Use eight characters")) }
    }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', source).some(
      (f) => f.code === 'copy-attribute' && /Use eight characters/.test(f.message),
    ),
    'a double-wrapped hint literal must be flagged',
  );
});

test('literal scan: a literal passed through a shorthand copy prop is flagged', () => {
  // `SkipLink { label }` desugars to `label: label`, so the backing `let label = "…"`
  // must be caught even though no interpolation literal appears inside rsx!.
  const source = `
fn view() -> Element {
    let label = "Skip to rooms".to_string();
    rsx! { SkipLink { anchor: "rooms-nav", label } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', source).some((f) => /Skip to rooms/.test(f.message)),
    'a let-bound literal flowing through a shorthand copy prop must be flagged',
  );
  // A STRUCTURAL shorthand prop (`anchor`) backed by a literal stays clean.
  const structural = `
fn view() -> Element {
    let anchor = "rooms-nav".to_string();
    rsx! { SkipLink { anchor, label: strings.skip_to_rooms() } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', structural).some((f) => /rooms-nav/.test(f.message)),
    'a structural shorthand prop must not be flagged',
  );
});

test('literal scan: a commented-out control id is not accepted as the real id', () => {
  // The control's real id was removed, leaving `// id: "email",` (comma INSIDE the
  // comment, codex's exact shape). Reading the raw slice extracts a clean "email"
  // that matches the Field and stays green; firstIdAttr must mask the comment, so
  // the control has NO id and its label names nothing.
  const source = `
fn view() -> Element {
    rsx! {
        Field { id: "email", label: "Email",
            input {
                // id: "email",
                r#type: "text"
            }
        }
    }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', source).some((f) => f.code === 'form-control-id-mismatch'),
    'a control whose only id is in a comment must be flagged (no real association)',
  );
});

test('literal scan: a nav named only by a look-alike attribute is still flagged', () => {
  // `data-aria-label` / `aria-label-extra` contain the substring `aria-label` but
  // name no landmark — the check must require the exact attribute token.
  for (const attr of ['"data-aria-label"', '"aria-label-extra"']) {
    const source = `
fn view() -> Element {
    rsx! { nav { ${attr}: "test-hook", a { href: "#", "Home" } } }
}
`;
    assert.ok(
      scanComponentLiterals('shell.rs', source).some(
        (f) => f.code === 'raw-semantic-element' && /nav/.test(f.message),
      ),
      `a nav with only ${attr} must be flagged as unnamed`,
    );
  }
  // A real `aria_label` underscore alias DOES name it (renders aria-label).
  const aliased = `
fn view() -> Element {
    rsx! { nav { aria_label: "Primary", a { href: "#", "Home" } } }
}
`;
  assert.ok(
    !scanComponentLiterals('shell.rs', aliased).some((f) => f.code === 'raw-semantic-element'),
    'a nav named via the aria_label alias must not be flagged',
  );
});

test('literal scan: an expression-id hinted control needs a DYNAMIC aria-describedby', () => {
  // With an expression Field id the rendered hint id is dynamic (`{id}-hint`), so a
  // hardcoded literal `aria-describedby` can never reference it and is flagged.
  const literalWrong = `
fn view() -> Element {
    rsx! { Field { id: field_id.clone(), label: "Email", hint: "we never share it",
        input { id: field_id.clone(), "aria-describedby": "wrong" } } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', literalWrong).some((f) => f.code === 'form-control-hint-unassociated'),
    'a literal aria-describedby under an expression id cannot reference the dynamic hint',
  );
  // A DYNAMIC (expression-valued) describedby is accepted — its target cannot be
  // compared statically, but it is at least derived at runtime.
  const dynamicOk = `
fn view() -> Element {
    rsx! { Field { id: field_id.clone(), label: "Email", hint: "we never share it",
        input { id: field_id.clone(), aria_describedby: format!("{field_id}-hint") } } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', dynamicOk).some((f) => f.code === 'form-control-hint-unassociated'),
    'a dynamic aria-describedby under an expression id must be accepted',
  );
});

test('literal scan: a macro-wrapped copy prop (format!) is flagged', () => {
  // `label: format!("Delete {name}")` / `aria_label: format!(...)` construct copy
  // directly in a copy-bearing prop; the wrapper peel must cross the macro `!`.
  const source = `
fn view() -> Element {
    rsx! {
        SkipLink { anchor: "x", label: format!("Delete {name}") }
        button { aria_label: format!("Remove account") }
    }
}
`;
  const copy = scanComponentLiterals('x.rs', source).filter((f) => f.code === 'copy-attribute');
  assert.ok(copy.some((f) => /Delete/.test(f.message)), 'label: format!(...) copy must be flagged');
  assert.ok(
    copy.some((f) => /Remove account/.test(f.message)),
    'aria_label: format!(...) copy must be flagged',
  );
});

test('literal scan: a literal rendered as a braced RSX expression child is flagged', () => {
  // `div { {message} }` renders the binding as a text node — copy regardless of the
  // variable name (it need not be a copy-bearing prop such as `label`).
  const source = `
fn view() -> Element {
    let message = "Delete account".to_string();
    rsx! { div { {message} } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', source).some((f) => /Delete account/.test(f.message)),
    'a let-bound literal rendered as a {message} child must be flagged',
  );
  const named = `
fn view() -> Element {
    let text = "Remove account".to_string();
    rsx! { section { {text} } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', named).some((f) => /Remove account/.test(f.message)),
    'the identifier name is irrelevant for a braced expression child',
  );
  // A structural braced attribute value (`id: {anchor}`) stays clean.
  const structural = `
fn view() -> Element {
    let anchor = "rooms-nav".to_string();
    rsx! { div { id: {anchor} } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', structural).some((f) => /rooms-nav/.test(f.message)),
    'a braced STRUCTURAL attribute value must not be flagged',
  );
});

test('literal scan: a dynamic aria-describedby naming a DIFFERENT id is flagged', () => {
  // Expression Field id, but the describedby references another identifier's hint,
  // so the real `{field_id}-hint` is never referenced.
  const wrong = `
fn view() -> Element {
    rsx! { Field { id: field_id.clone(), label: "Email", hint: "x",
        input { id: field_id.clone(), aria_describedby: format!("{other_id}-hint") } } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', wrong).some((f) => f.code === 'form-control-hint-unassociated'),
    'a dynamic describedby referencing a different id must be flagged',
  );
  // Referencing the SAME base identifier is accepted.
  const right = `
fn view() -> Element {
    rsx! { Field { id: field_id.clone(), label: "Email", hint: "x",
        input { id: field_id.clone(), aria_describedby: format!("{field_id}-hint") } } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', right).some((f) => f.code === 'form-control-hint-unassociated'),
    'a dynamic describedby referencing the Field id must be accepted',
  );
});

test('literal scan: a dynamic aria-describedby naming a related-but-different PATH is flagged', () => {
  // `id: ids.email.clone()` and `aria_describedby: format!("{}-hint", ids.other)`
  // share the `ids` receiver but name DIFFERENT hints — the full access path must
  // match, not just the first identifier.
  const wrong = `
fn view() -> Element {
    rsx! { Field { id: ids.email.clone(), label: "Email", hint: "x",
        input { id: ids.email.clone(), aria_describedby: format!("{}-hint", ids.other) } } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', wrong).some((f) => f.code === 'form-control-hint-unassociated'),
    'a describedby referencing a related-but-different path must be flagged',
  );
  const right = `
fn view() -> Element {
    rsx! { Field { id: ids.email.clone(), label: "Email", hint: "x",
        input { id: ids.email.clone(), aria_describedby: format!("{}-hint", ids.email) } } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', right).some((f) => f.code === 'form-control-hint-unassociated'),
    'a describedby referencing the full id path must be accepted',
  );
});

test('rule 3 plural: a reordered but untranslated French plural is still flagged', () => {
  const en = `impl Catalog for En {
    fn rooms(&self, n: &str, c: PluralCategory) -> String {
      match c {
        PluralCategory::One => format!("{n} room"),
        PluralCategory::Other => format!("{n} rooms"),
      }
    }
  }`;
  // FR reorders the arms but leaves BOTH categories in English.
  const frReordered = `impl Catalog for Fr {
    fn rooms(&self, n: &str, c: PluralCategory) -> String {
      match c {
        PluralCategory::Other => format!("{n} rooms"),
        PluralCategory::One => format!("{n} room"),
      }
    }
  }`;
  assert.ok(
    checkCatalogs({
      en: parseCatalog(en, 'en.rs'),
      fr: parseCatalog(frReordered, 'fr.rs'),
      allowlist: {},
    }).some((f) => f.code === 'fr-untranslated'),
    'a reordered but fully-English plural must be flagged as untranslated',
  );
  // The same shape, genuinely translated, is NOT flagged.
  const frTranslated = frReordered
    .replace('{n} rooms', '{n} salles')
    .replace('{n} room', '{n} salle');
  assert.ok(
    !checkCatalogs({
      en: parseCatalog(en, 'en.rs'),
      fr: parseCatalog(frTranslated, 'fr.rs'),
      allowlist: {},
    }).some((f) => f.code === 'fr-untranslated'),
    'a translated (reordered) plural must not be flagged',
  );
});

test('literal scan: copy returned by a local helper invoked in RSX is flagged', () => {
  // A literal factored into a helper and rendered via its call is still copy.
  const child = `
fn destructive_label() -> &'static str { "Delete account" }
fn view() -> Element {
    rsx! { div { {destructive_label()} } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', child).some((f) => /Delete account/.test(f.message)),
    'a literal returned by a helper invoked as an RSX child must be flagged',
  );
  const attr = `
fn skip_label() -> String { "Skip to rooms".to_string() }
fn view() -> Element {
    rsx! { SkipLink { anchor: "x", label: skip_label() } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', attr).some((f) => /Skip to rooms/.test(f.message)),
    'a literal returned by a helper used as a copy attr must be flagged',
  );
  // A helper invoked ONLY in a structural position is not scanned.
  const structural = `
fn id_for() -> String { format!("email-hint") }
fn view() -> Element {
    rsx! { div { id: id_for() } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', structural).some((f) => /email-hint/.test(f.message)),
    'a helper used only for a structural attribute must not be scanned',
  );
});

test('rule 3 plural: a PARTIALLY translated French plural is flagged per category', () => {
  const en = `impl Catalog for En {
    fn rooms(&self, n: &str, c: PluralCategory) -> String {
      match c {
        PluralCategory::One => format!("{n} room"),
        PluralCategory::Other => format!("{n} rooms"),
      }
    }
  }`;
  // FR translated the One arm but left Other in English — English still ships for
  // every count > 1.
  const frPartial = `impl Catalog for Fr {
    fn rooms(&self, n: &str, c: PluralCategory) -> String {
      match c {
        PluralCategory::One => format!("{n} salle"),
        PluralCategory::Other => format!("{n} rooms"),
      }
    }
  }`;
  assert.ok(
    checkCatalogs({
      en: parseCatalog(en, 'en.rs'),
      fr: parseCatalog(frPartial, 'fr.rs'),
      allowlist: {},
    }).some((f) => f.code === 'fr-untranslated'),
    'a plural with one category still in English must be flagged',
  );
  // A language-NEUTRAL identical arm (`{n}`, no translatable word) does not, on its
  // own, flag a plural whose translatable arm IS translated.
  const enNeutral = `impl Catalog for En {
    fn count(&self, n: &str, c: PluralCategory) -> String {
      match c {
        PluralCategory::One => format!("{n}"),
        PluralCategory::Other => format!("{n} rooms"),
      }
    }
  }`;
  const frNeutral = `impl Catalog for Fr {
    fn count(&self, n: &str, c: PluralCategory) -> String {
      match c {
        PluralCategory::One => format!("{n}"),
        PluralCategory::Other => format!("{n} salles"),
      }
    }
  }`;
  assert.ok(
    !checkCatalogs({
      en: parseCatalog(enNeutral, 'en.rs'),
      fr: parseCatalog(frNeutral, 'fr.rs'),
      allowlist: {},
    }).some((f) => f.code === 'fr-untranslated'),
    'a language-neutral identical arm must not flag an otherwise-translated plural',
  );
});

test('literal scan: a dynamic aria-describedby must CONSTRUCT {id}-hint', () => {
  // References the id ITSELF (no -hint).
  const bare = `
fn view() -> Element {
    rsx! { Field { id: ids.email.clone(), label: "Email", hint: "x",
        input { id: ids.email.clone(), aria_describedby: ids.email.clone() } } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', bare).some((f) => f.code === 'form-control-hint-unassociated'),
    'a describedby that is the id itself (no -hint) must be flagged',
  );
  // A PREFIX of the id must not satisfy it.
  const prefix = `
fn view() -> Element {
    rsx! { Field { id: ids.email.clone(), label: "Email", hint: "x",
        input { id: ids.email.clone(), aria_describedby: format!("{}-hint", ids.email_backup) } } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', prefix).some((f) => f.code === 'form-control-hint-unassociated'),
    'a prefix (ids.email_backup) must not satisfy the hint check',
  );
  // The correct construction passes.
  const ok = `
fn view() -> Element {
    rsx! { Field { id: ids.email.clone(), label: "Email", hint: "x",
        input { id: ids.email.clone(), aria_describedby: format!("{}-hint", ids.email) } } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', ok).some((f) => f.code === 'form-control-hint-unassociated'),
    'format!("{}-hint", ids.email) must be accepted',
  );
});

test('literal scan: two controls in one Field are flagged as duplicate', () => {
  const two = `
fn view() -> Element {
    rsx! { Field { id: "email", label: "Email", input { id: "email" } select { id: "email" } } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', two).some((f) => f.code === 'form-control-count'),
    'a Field wrapping two controls must be flagged',
  );
  const one = `
fn view() -> Element {
    rsx! { Field { id: "email", label: "Email", input { id: "email" } } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', one).some((f) => f.code === 'form-control-count'),
    'a Field with one control is clean',
  );
});

test('literal scan: a commented-out hint does not demand aria-describedby', () => {
  const source = `
fn view() -> Element {
    rsx! { Field { id: "email", label: "Email",
        // hint: Some(catalog_help),
        input { id: "email" } } }
}
`;
  assert.ok(
    !scanComponentLiterals('x.rs', source).some((f) => f.code === 'form-control-hint-unassociated'),
    'a commented-out hint must not require aria-describedby',
  );
});

test('rule 3: a let-bound helper literal does not mask untranslated returned copy', () => {
  const en = `impl Catalog for En {
    fn empty(&self) -> &'static str { "No rooms yet" }
  }`;
  const fr = `impl Catalog for Fr {
    fn empty(&self) -> &'static str { let _note = "Aucun salon"; "No rooms yet" }
  }`;
  assert.ok(
    checkCatalogs({
      en: parseCatalog(en, 'en.rs'),
      fr: parseCatalog(fr, 'fr.rs'),
      allowlist: {},
    }).some((f) => f.code === 'fr-untranslated'),
    'the RETURNED English value must be flagged despite a French let-bound literal',
  );
});

test('literal scan: copy in a call-plus-method-chain expression child is flagged', () => {
  const source = `
fn view() -> Element {
    rsx! { div { {Some("Delete account").unwrap()} } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', source).some(
      (f) => f.code === 'rust-text' && /Delete account/.test(f.message),
    ),
    'a literal in {Some("…").unwrap()} must be flagged',
  );
});

test('literal scan: a role value a primitive does not own is flagged in its file', () => {
  // live_region.rs owns role="status" but NOT role="dialog".
  const rogue = `
fn view() -> Element {
    rsx! { div { role: "dialog", "x" } }
}
`;
  assert.ok(
    scanComponentLiterals('components/live_region.rs', rogue).some((f) => f.code === 'raw-semantic'),
    'role="dialog" in live_region.rs (owns role=status) must be flagged',
  );
  const owned = `
fn view() -> Element {
    rsx! { div { role: "status", "x" } }
}
`;
  assert.ok(
    !scanComponentLiterals('components/live_region.rs', owned).some((f) => f.code === 'raw-semantic'),
    'role="status" is owned by live_region.rs',
  );
});

test('rule 3: one untranslated branch of a nonplural method is flagged', () => {
  const en = `impl Catalog for En {
    fn month(&self, m: u8) -> &'static str {
      match m { 1 => "January", 8 => "August", _ => "?" }
    }
  }`;
  // FR translated January but left August in English.
  const fr = `impl Catalog for Fr {
    fn month(&self, m: u8) -> &'static str {
      match m { 1 => "Janvier", 8 => "August", _ => "?" }
    }
  }`;
  assert.ok(
    checkCatalogs({
      en: parseCatalog(en, 'en.rs'),
      fr: parseCatalog(fr, 'fr.rs'),
      allowlist: {},
    }).some((f) => f.code === 'fr-untranslated'),
    'a single English branch (August) must be flagged even when others are translated',
  );
});

test('rule 3: a non-returned statement literal does not mask untranslated copy', () => {
  const en = `impl Catalog for En {
    fn empty(&self) -> &'static str { "No rooms yet" }
  }`;
  // The French `debug_assert!` MESSAGE is not the returned value.
  const fr = `impl Catalog for Fr {
    fn empty(&self) -> &'static str { debug_assert!(true, "Aucun salon"); "No rooms yet" }
  }`;
  assert.ok(
    checkCatalogs({ en: parseCatalog(en, 'en.rs'), fr: parseCatalog(fr, 'fr.rs'), allowlist: {} }).some(
      (f) => f.code === 'fr-untranslated',
    ),
    'the returned English value must be flagged despite a non-returned French literal',
  );
});

test('rule 3: a reordered nonplural match with an untranslated arm is flagged', () => {
  const en = `impl Catalog for En {
    fn month(&self, m: u8) -> &'static str { match m { 1 => "January", _ => "August" } }
  }`;
  // FR reorders arms AND leaves the `_` arm in English.
  const fr = `impl Catalog for Fr {
    fn month(&self, m: u8) -> &'static str { match m { _ => "August", 1 => "janvier" } }
  }`;
  assert.ok(
    checkCatalogs({ en: parseCatalog(en, 'en.rs'), fr: parseCatalog(fr, 'fr.rs'), allowlist: {} }).some(
      (f) => f.code === 'fr-untranslated',
    ),
    'a reordered untranslated match arm must be caught by pattern key',
  );
});

test('literal scan: a dynamic aria-describedby must build {id}-hint as a unit', () => {
  const bad = `
fn view() -> Element {
    rsx! { Field { id: ids.email.clone(), label: "Email", hint: "x",
        input { id: ids.email.clone(), aria_describedby: format!("wrong-hint {}", ids.email) } } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', bad).some((f) => f.code === 'form-control-hint-unassociated'),
    'containing `-hint` and idCore INDEPENDENTLY must not pass',
  );
});

test('literal scan: a literal input value is copy', () => {
  const source = `
fn view() -> Element {
    rsx! { input { r#type: "submit", value: "Delete account" } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', source).some(
      (f) => f.code === 'copy-attribute' && /Delete account/.test(f.message),
    ),
    'a control value literal is its visible label — copy',
  );
});

test('literal scan: an empty aria_label does not name a nav', () => {
  const source = `
fn view() -> Element {
    rsx! { nav { aria_label: "", a { href: "#", "Home" } } }
}
`;
  assert.ok(
    scanComponentLiterals('shell.rs', source).some(
      (f) => f.code === 'raw-semantic-element' && /nav/.test(f.message),
    ),
    'aria_label="" is not an accessible name',
  );
});

test('literal scan: a char literal delimiter does not break RSX ranges', () => {
  const source = `
fn view() -> Element {
    rsx! { if delimiter == '}' { div { "Delete account" } } }
}
`;
  assert.ok(
    scanComponentLiterals('x.rs', source).some((f) => /Delete account/.test(f.message)),
    "copy after a char literal ('}') must still be scanned",
  );
});

test('parseCatalog: a method with a tuple-typed parameter is parsed', () => {
  const src = `impl Catalog for En {
    fn range(&self, bounds: (u32, u32)) -> String { format!("{} to {}", bounds.0, bounds.1) }
    fn hello(&self) -> &'static str { "Hello" }
  }`;
  const { entries } = parseCatalog(src, 'en.rs');
  assert.ok(entries.has('range'), 'a tuple-param method must be parsed, not omitted');
  assert.ok(entries.has('hello'), 'and methods after it');
});

test('rule 2: a placeholder-only value is not empty', () => {
  const en = `impl Catalog for En {
    fn count(&self, n: &str) -> String { format!("{n}") }
  }`;
  const fr = `impl Catalog for Fr {
    fn count(&self, n: &str) -> String { format!("{n}") }
  }`;
  assert.ok(
    !checkCatalogs({ en: parseCatalog(en, 'en.rs'), fr: parseCatalog(fr, 'fr.rs'), allowlist: {} }).some(
      (f) => f.code === 'value-empty',
    ),
    'format!("{n}") renders a count and is not value-empty',
  );
});

test('rule 3: a one-letter word keeps identical copy from being auto-exempted', () => {
  const en = `impl Catalog for En {
    fn note(&self) -> &'static str { "A daemon" }
  }`;
  const fr = `impl Catalog for Fr {
    fn note(&self) -> &'static str { "A daemon" }
  }`;
  assert.ok(
    checkCatalogs({ en: parseCatalog(en, 'en.rs'), fr: parseCatalog(fr, 'fr.rs'), allowlist: {} }).some(
      (f) => f.code === 'fr-untranslated',
    ),
    '"A daemon" is translatable copy, not an auto-exempt never-translate phrase',
  );
});

test('rule 2: a literal-free String::new() branch is value-empty; _ => None is not (#274 22-1)', () => {
  const src = `impl Catalog for En {
    fn month(&self, m: u8) -> Option<&'static str> { match m { 1 => Some("January"), _ => None } }
    fn tag(&self, n: u8) -> String { match n { 1 => format!("One {}", "item"), _ => String::new() } }
  }`;
  const empty = checkCatalogs({ ...catalogs(src, src), allowlist: {} }).filter((f) => f.code === 'value-empty');
  assert.ok(empty.some((f) => /tag/.test(f.message)), 'the String::new() branch renders blank → value-empty');
  assert.ok(!empty.some((f) => /month/.test(f.message)), '_ => None returns Option::None, not empty copy');
});

test('rule: nonplural match placeholder parity is per-arm — a slot swapped between branches is flagged (#274 22-2)', () => {
  const en = `impl Catalog for En {
    fn label(&self, n: &str, x: &str, b: bool) -> String { match b { true => format!("Count {n}"), false => format!("Name {x}") } }
  }`;
  const fr = `impl Catalog for Fr {
    fn label(&self, n: &str, x: &str, b: bool) -> String { match b { true => format!("Compte {x}"), false => format!("Nom {n}") } }
  }`;
  const codes = checkCatalogs({ ...catalogs(en, fr), allowlist: {} }).map((f) => f.code);
  assert.ok(codes.includes('placeholder-parity'), 'fr swapped {n}/{x} between arms — identical pooled multiset, per-arm catches it');
});

test('literal scan: copy nested in a constructor inside an expression child is flagged (#274 22-3)', () => {
  const source = `fn C() -> Element { rsx! { div { {Some(String::from("Delete account")).unwrap()} } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && /Delete account/.test(f.message)),
    'a literal nested in Some(String::from(...)).unwrap() inside a text expression child is rendered copy',
  );
});

test('literal scan: production copy AFTER an inline #[cfg(test)] module is still scanned (#274 22-4)', () => {
  const source = `
    fn Before() -> Element { rsx! { div { {strings.hello()} } } }
    #[cfg(test)]
    mod tests { fn t() { let _ = "in test only"; } }
    fn After() -> Element { rsx! { div { "Delete account" } } }
  `;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && /Delete account/.test(f.message)),
    'copy in a production component AFTER a test module must be flagged',
  );
  assert.ok(
    !findings.some((f) => /in test only/.test(f.message)),
    'copy INSIDE the masked test module stays exempt',
  );
});

test('rule 3: a split-literal untranslated value (concat!) is flagged (#274 22-6)', () => {
  const en = `impl Catalog for En { fn title(&self) -> String { concat!("Delete ", "account").to_string() } }`;
  const fr = `impl Catalog for Fr { fn title(&self) -> String { "Delete account".to_string() } }`;
  const codes = checkCatalogs({ ...catalogs(en, fr), allowlist: {} }).map((f) => f.code);
  assert.ok(
    codes.includes('fr-untranslated'),
    'EN concat!("Delete ", "account") and FR "Delete account" render byte-identical text',
  );
});

test('rule 3: an untranslated if/else return branch is flagged (#274 23-1)', () => {
  const en = `impl Catalog for En {
    fn label(&self, c: bool) -> &'static str { if c { "Delete" } else { "Cancel" } }
  }`;
  const fr = `impl Catalog for Fr {
    fn label(&self, c: bool) -> &'static str { if c { "Supprimer" } else { "Cancel" } }
  }`;
  const codes = checkCatalogs({ ...catalogs(en, fr), allowlist: {} }).map((f) => f.code);
  assert.ok(
    codes.includes('fr-untranslated'),
    'the untranslated else branch ("Cancel") must be flagged, not hidden by the translated if branch',
  );
});

test('rule 3: a translated if/else method is NOT flagged (#274 23-1 control)', () => {
  const en = `impl Catalog for En {
    fn label(&self, c: bool) -> &'static str { if c { "Delete" } else { "Cancel" } }
  }`;
  const fr = `impl Catalog for Fr {
    fn label(&self, c: bool) -> &'static str { if c { "Supprimer" } else { "Annuler" } }
  }`;
  assert.ok(
    !checkCatalogs({ ...catalogs(en, fr), allowlist: {} }).some((f) => f.code === 'fr-untranslated'),
    'both branches translated → no fr-untranslated',
  );
});

test('literal scan: a qualified helper call in an expression child is traced (#274 23-2)', () => {
  const source = `fn hardcoded() -> &'static str { "Delete account" }
fn C() -> Element { rsx! { div { {Self::hardcoded()} } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && /Delete account/.test(f.message)),
    'a qualified helper call (Self::hardcoded) must be traced so its returned literal is flagged',
  );
});

test('rule: if/else placeholder parity is per-branch (#274 24-1)', () => {
  const en = `impl Catalog for En {
    fn label(&self, n: &str, x: &str, c: bool) -> String { if c { format!("Count {n}") } else { format!("Name {x}") } }
  }`;
  const fr = `impl Catalog for Fr {
    fn label(&self, n: &str, x: &str, c: bool) -> String { if c { format!("Compte {x}") } else { format!("Nom {n}") } }
  }`;
  const codes = checkCatalogs({ ...catalogs(en, fr), allowlist: {} }).map((f) => f.code);
  assert.ok(codes.includes('placeholder-parity'), 'fr swapped {n}/{x} between if/else branches (identical pooled set) must be caught');
});

test('rule 2: a Default::default() branch is value-empty (#274 24-2)', () => {
  const src = `impl Catalog for En {
    fn tag(&self, c: bool) -> String { if c { "Traduit".to_owned() } else { Default::default() } }
  }`;
  const empty = checkCatalogs({ ...catalogs(src, src), allowlist: {} }).filter((f) => f.code === 'value-empty');
  assert.ok(empty.some((f) => /tag/.test(f.message)), 'the Default::default() branch renders blank → value-empty');
});

test('literal scan: a transitively-called helper is traced (#274 24-3)', () => {
  const source = `fn inner() -> &'static str { "Delete account" }
fn outer() -> &'static str { inner() }
fn C() -> Element { rsx! { div { {outer()} } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && /Delete account/.test(f.message)),
    'outer() delegates to inner(): the inner helper literal must be traced transitively',
  );
});

test('literal scan: a qualified helper resolves only the matching receiver, not a same-named sibling (#274 25-3)', () => {
  // `A::greeting()` is copy; `B::greeting()` is never invoked. Resolving the
  // helper by TERMINAL name alone marks BOTH bodies as copy, so B's structural
  // literal (outside RSX, never rendered as copy) is wrongly flagged. Scoping
  // resolution to the receiver's `impl`/`mod` resolves only A's method.
  const source = `impl A {
    fn greeting() -> &'static str { "Please translate me" }
}
impl B {
    fn greeting() -> &'static str { "Structural token value" }
}
fn C() -> Element { rsx! { div { {A::greeting()} } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && /Please translate me/.test(f.message)),
    'A::greeting is invoked as copy, so its returned literal must be flagged',
  );
  assert.ok(
    !findings.some((f) => /Structural token value/.test(f.message)),
    'B::greeting is never invoked: resolving A::greeting must not mark the same-named impl B body as copy',
  );
});

test('rule 2: a qualified/std empty constructor branch is value-empty (#274 25-2)', () => {
  // A branch returning `std::string::String::new()` (or spaced `String :: new ()`)
  // renders blank exactly like the bare `String::new()` — the empty-ctor regex must
  // tolerate a path prefix and interior whitespace, else the blank branch is missed.
  const src = `impl Catalog for En {
    fn tag(&self, c: bool) -> String { if c { "Traduit".to_owned() } else { std::string::String::new() } }
  }`;
  const empty = checkCatalogs({ ...catalogs(src, src), allowlist: {} }).filter((f) => f.code === 'value-empty');
  assert.ok(empty.some((f) => /tag/.test(f.message)), 'std::string::String::new() renders blank → value-empty');
});

test('scan: an unescaped newline is part of the string, not its end (#274 25-4)', () => {
  // A Rust ordinary string may span lines. Breaking the scan at `\n` truncates the
  // literal at the first line, so copy AFTER the newline (`beta gamma`) is treated as
  // code and never flagged. Scanning to the closing quote captures the whole literal.
  const source = `fn C() -> Element {
    rsx! {
        div { "alpha
beta gamma" }
    }
}`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && /beta gamma/.test(f.message)),
    'copy after an unescaped newline must be part of the flagged literal, not treated as code',
  );
});

test('literal scan: a single-letter rendered label is copy (#274 25-5)', () => {
  // A one-character label (`"X"` on a close button, a lone CJK glyph) is rendered to
  // users too; a `{2,}`-letter threshold would skip it. Structural position, not
  // length, is what separates copy from identifiers.
  const source = `fn C() -> Element { rsx! { button { "X" } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && f.message.includes('X')),
    'a single-letter RSX text child must be flagged as copy',
  );
});

test('rule 4: fr typography checks the COMPLETE rendered branch, not each fragment (#274 25-6)', () => {
  // The narrow no-break space sits in the PREVIOUS fragment; the branch is correct once
  // joined. A per-fragment check sees the bare `"!"` and wrongly demands a U+202F before
  // it — the rule must run over the concatenated branch value.
  const en = `impl Catalog for En {
    fn bye(&self) -> String { concat!("Goodbye", "!").to_owned() }
  }`;
  const fr = `impl Catalog for Fr {
    fn bye(&self) -> String { concat!("Au revoir\\u{202f}", "!").to_owned() }
  }`;
  const codes = checkCatalogs({ ...catalogs(en, fr), allowlist: {} }).map((f) => f.code);
  assert.ok(
    !codes.includes('fr-narrow-space'),
    'the U+202F is in the preceding fragment; the joined branch is correct, so no fr-narrow-space',
  );
});

test('literal scan: an i18n-exempt comment cannot silence a reserved-semantic finding (#274 25-7)', () => {
  // `i18n-exempt` clears literal-COPY findings only. A reserved semantic element
  // (Decision-6) is an accessibility gate a diagnostic comment must NOT disable.
  const source = `fn C() -> Element { rsx! { dialog { "hi" } } } // i18n-exempt: legacy`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'raw-semantic-element'),
    'a raw dialog element must be flagged even with an i18n-exempt marker on the line',
  );
});

test('source hygiene: the scanner source has no raw NUL byte (#274 27-ip)', () => {
  // A literal NUL (U+0000) makes rg/grep classify the file as binary and hide its
  // contents from searches and some review/lint tooling. The map-key delimiter must
  // be written as the SOURCE escape `\0`, not embedded as a raw NUL byte.
  const src = readFileSync(new URL('./lib/jeliya-ui-catalog.mjs', import.meta.url), 'utf8');
  assert.equal(src.indexOf('\0'), -1, 'the scanner source must not embed a raw NUL byte');
});

test('rule 5: plural coverage is per-category, not pooled fragment count (#274 26-1)', () => {
  // `One` has two fragments; `Other` renders nothing. A pooled `values.length >= 2`
  // check passes it, but a category that renders nothing is a real defect.
  const src = `impl Catalog for En {
    fn rooms(&self, n: &str, category: PluralCategory) -> String {
      match category {
        PluralCategory::One => concat!("One ", "room").to_owned(),
        PluralCategory::Other => unreachable!(),
      }
    }
  }`;
  const codes = checkCatalogs({ ...catalogs(src, src), allowlist: {} }).map((f) => f.code);
  assert.ok(
    codes.includes('plural-parity'),
    'Other renders nothing → plural-parity, despite two pooled One fragments',
  );
});

test('rule 2: a None.unwrap_or_default() branch is value-empty (#274 26-2)', () => {
  const src = `impl Catalog for En {
    fn tag(&self, c: bool) -> String { if c { "Traduit".to_owned() } else { Option::<String>::None.unwrap_or_default() } }
  }`;
  const empty = checkCatalogs({ ...catalogs(src, src), allowlist: {} }).filter((f) => f.code === 'value-empty');
  assert.ok(
    empty.some((f) => /tag/.test(f.message)),
    'Option::<String>::None.unwrap_or_default() renders blank → value-empty',
  );
});

test('scan: non-BMP text before markup does not drift literal offsets (#274 28-utf16)', () => {
  // Five emoji (each a surrogate PAIR) in an earlier literal. A code-POINT mask makes
  // the later `"Delete account"` map to the wrong skeleton range and be missed; a
  // code-UNIT mask keeps offsets aligned.
  const source = `fn A() -> Element { rsx! { div { "😀😀😀😀😀" } } }
fn C() -> Element { rsx! { span { "Delete account" } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && /Delete account/.test(f.message)),
    'a literal after non-BMP (surrogate-pair) text must still be flagged',
  );
});

test('rule 4: a Rust line continuation elides the newline+indent before typography (#274 28-linecont)', () => {
  const en = `impl Catalog for En {
    fn bye(&self) -> String { "Goodbye!".to_owned() }
  }`;
  const fr = `impl Catalog for Fr {
    fn bye(&self) -> String { "Au revoir\\u{202f}\\
    !".to_owned() }
  }`;
  const codes = checkCatalogs({ ...catalogs(en, fr), allowlist: {} }).map((f) => f.code);
  assert.ok(
    !codes.includes('fr-narrow-space'),
    'the line continuation renders U+202F immediately before !, so no fr-narrow-space',
  );
});

test('rule: escaped braces {{ }} are not placeholder slots (#274 28-escbrace)', () => {
  const en = `impl Catalog for En {
    fn count(&self, n: &str) -> String { format!("Count {n}") }
  }`;
  const fr = `impl Catalog for Fr {
    fn count(&self, n: &str) -> String { format!("Compte {{n}}") }
  }`;
  const codes = checkCatalogs({ ...catalogs(en, fr), allowlist: {} }).map((f) => f.code);
  assert.ok(
    codes.includes('placeholder-parity'),
    'FR {{n}} renders literal text, not a slot, so its slot set differs from EN {n}',
  );
});

test('rule 1: a multi-word all-token value is not auto-exempted (#274 28-exempt)', () => {
  // `hash` and `mismatch` are both protocol tokens, but "Hash mismatch" is prose:
  // only the exact wire code is exempt; the UI label must be translated.
  const en = `impl Catalog for En {
    fn err(&self) -> &'static str { "Hash mismatch" }
  }`;
  const fr = `impl Catalog for Fr {
    fn err(&self) -> &'static str { "Hash mismatch" }
  }`;
  const codes = checkCatalogs({ ...catalogs(en, fr), allowlist: {} }).map((f) => f.code);
  assert.ok(
    codes.includes('fr-untranslated'),
    'identical multi-word prose is flagged untranslated despite its words being tokens',
  );
  assert.equal(
    identityExemption('x', 'hash')?.source,
    'automatic',
    'an exact single protocol token stays auto-exempt',
  );
});

test('rule 2: a String::with_capacity() branch is value-empty (#274 29-capacity)', () => {
  const src = `impl Catalog for En {
    fn tag(&self, c: bool) -> String { if c { "Bonjour".to_owned() } else { String::with_capacity(16) } }
  }`;
  const empty = checkCatalogs({ ...catalogs(src, src), allowlist: {} }).filter((f) => f.code === 'value-empty');
  assert.ok(
    empty.some((f) => /tag/.test(f.message)),
    'String::with_capacity(16) renders the empty string → value-empty',
  );
});

test('form: a shorthand Field id is compared against the control id (#274 29-shorthand)', () => {
  // `Field { id, … input { id: "other" } }` — shorthand `id` (= `id: id`). The
  // label[for] uses `id` but the control uses `"other"`, so the control is unnamed;
  // the mismatch must be caught, not skipped for want of an explicit `id:` on Field.
  const source = `fn F() -> Element { rsx! { Field { id, label: "Email", input { id: "other" } } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'form-control-id-mismatch'),
    'a shorthand Field id mismatched by the control must be flagged',
  );
});

test('literal scan: an iframe srcdoc HTML literal is copy (#274 30-srcdoc)', () => {
  const source = `fn C() -> Element { rsx! { iframe { srcdoc: "<p>Delete account</p>" } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => /Delete account/.test(f.message)),
    'iframe srcdoc renders embedded HTML copy and must be flagged',
  );
});

test('literal scan: copy bound to a FORMATTED interpolation is flagged (#274 30-fmtspec)', () => {
  const source = `fn C() -> Element { let label = "Delete account"; rsx! { div { "{label:>20}" } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && /Delete account/.test(f.message)),
    'a let-bound literal rendered via {label:>20} must be flagged',
  );
});

test('literal scan: copy ASSIGNED after declaration is flagged (#274 30-assign)', () => {
  const source = `fn C() -> Element { let mut label = String::new(); label = "Delete account"; rsx! { div { "{label}" } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && /Delete account/.test(f.message)),
    'a literal assigned to an interpolated binding after declaration must be flagged',
  );
});

test('rule 2: a whitespace fragment in a concat is not value-empty (#274 30-concatspace)', () => {
  const src = `impl Catalog for En {
    fn greeting(&self) -> String { concat!("Hello", " ", "world").to_owned() }
  }`;
  const empty = checkCatalogs({ ...catalogs(src, src), allowlist: {} }).filter((f) => f.code === 'value-empty');
  assert.ok(
    !empty.some((f) => /greeting/.test(f.message)),
    'the joined value "Hello world" is nonempty; the " " fragment must not trip value-empty',
  );
});

test('rule 1: a differing statement literal in a branch does not hide an untranslated tail (#274 30-stmtlit)', () => {
  const en = `impl Catalog for En {
    fn msg(&self, c: bool) -> String { if c { let _note = "EN note"; "Delete account".to_owned() } else { "Other".to_owned() } }
  }`;
  const fr = `impl Catalog for Fr {
    fn msg(&self, c: bool) -> String { if c { let _note = "FR note"; "Delete account".to_owned() } else { "Autre".to_owned() } }
  }`;
  const codes = checkCatalogs({ ...catalogs(en, fr), allowlist: {} }).map((f) => f.code);
  assert.ok(
    codes.includes('fr-untranslated'),
    'the if-branch tail "Delete account" is still English and must be flagged, not hidden by the differing note',
  );
});

test('rule 5: an if/else plural dispatch recognizes both categories (#274 30-ifplural)', () => {
  const src = `impl Catalog for En {
    fn rooms(&self, category: PluralCategory) -> String {
      if category == PluralCategory::One { "One room".to_owned() } else { "Many rooms".to_owned() }
    }
  }`;
  const codes = checkCatalogs({ ...catalogs(src, src), allowlist: {} }).map((f) => f.code);
  assert.ok(
    !codes.includes('plural-parity'),
    'both One and Other branches are recognized in an if/else dispatch, so no plural-parity',
  );
});

test('rule 1: an explicit return literal in a branch is a rendered value (#274 31-return)', () => {
  const en = `impl Catalog for En {
    fn msg(&self, c: bool) -> String { if c { return "Delete account".to_owned(); } "English".to_owned() }
  }`;
  const fr = `impl Catalog for Fr {
    fn msg(&self, c: bool) -> String { if c { return "Delete account".to_owned(); } "Francais".to_owned() }
  }`;
  const codes = checkCatalogs({ ...catalogs(en, fr), allowlist: {} }).map((f) => f.code);
  assert.ok(
    codes.includes('fr-untranslated'),
    'the identical return "Delete account" is a rendered value and must be flagged, not excluded as a statement',
  );
});

test('rule 3: an allowlisted value split differently across locales is not stale (#274 31-allowsplit)', () => {
  const en = `impl Catalog for En { fn diag(&self) -> String { "Diagnostics".to_owned() } }`;
  const fr = `impl Catalog for Fr { fn diag(&self) -> String { concat!("Diag", "nostics").to_owned() } }`;
  const codes = checkCatalogs({
    ...catalogs(en, fr),
    allowlist: { diag: 'brand term, identical by design' },
  }).map((f) => f.code);
  assert.ok(
    !codes.includes('allowlist-stale'),
    'EN "Diagnostics" and FR concat render the same text, so the exemption is not stale',
  );
});

test('rule: positional format arguments are rejected in favor of named (#274 31-positional)', () => {
  const src = `impl Catalog for En {
    fn room(&self, r: &str, u: &str) -> String { format!("Room {0} for {1}", r, u) }
  }`;
  const codes = checkCatalogs({ ...catalogs(src, src), allowlist: {} }).map((f) => f.code);
  assert.ok(
    codes.includes('positional-format'),
    'positional {0}/{1} slots must be flagged; use named params',
  );
});

test('literal scan: a Field with no control is flagged (#274 32-nocontrol)', () => {
  const source = `fn F() -> Element { rsx! { Field { id: "email", label: "Email", div {} } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'form-control-count'),
    'a Field with zero controls names nothing and must be flagged',
  );
});

test('literal scan: a char literal bound as copy is flagged (#274 32-charlit)', () => {
  const source = `fn C() -> Element { let label = 'A'; rsx! { button { "{label}" } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && /A/.test(f.message)),
    'a char literal rendered as copy must be flagged like a one-char string',
  );
});

test('literal scan: copy appended via push_str to an interpolated binding is flagged (#274 33-pushstr)', () => {
  const source = `fn C() -> Element { let mut label = String::new(); label.push_str("Delete account"); rsx! { div { "{label}" } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && /Delete account/.test(f.message)),
    'copy appended via push_str to an interpolated binding must be flagged',
  );
});

test('literal scan: copy in a LATER mutation arg (insert_str) is flagged (#274 34-insertarg)', () => {
  const source = `fn C() -> Element { let mut label = String::new(); label.insert_str(0, "Delete account"); rsx! { div { "{label}" } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && /Delete account/.test(f.message)),
    'copy in insert_str(0, "…") (a later arg) must be flagged',
  );
});

test('literal scan: copy bound to an explicit copy PROP identifier is flagged (#274 34-explicitprop)', () => {
  const source = `fn C() -> Element { let text = "Delete account".to_string(); rsx! { Banner { label: text } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && /Delete account/.test(f.message)),
    'a let bound to an explicit copy prop value (label: text) must be flagged',
  );
});

test('rule 1: escaped braces {{ }} keep their letters for the untranslated check (#274 35-escbracetext)', () => {
  const src = `impl Catalog for En { fn m(&self) -> String { format!("{{Delete}}") } }`;
  const codes = checkCatalogs({ ...catalogs(src, src), allowlist: {} }).map((f) => f.code);
  assert.ok(
    codes.includes('fr-untranslated'),
    'identical "{{Delete}}" renders {Delete} with letters, so it is untranslated, not language-neutral',
  );
});

test('rule: placeholder parity ignores the format spec (#274 35-slotspec)', () => {
  const en = `impl Catalog for En { fn m(&self, n: &str) -> String { format!("Count {n}") } }`;
  const fr = `impl Catalog for Fr { fn m(&self, n: &str) -> String { format!("Compte {n:>5}") } }`;
  const codes = checkCatalogs({ ...catalogs(en, fr), allowlist: {} }).map((f) => f.code);
  assert.ok(
    !codes.includes('placeholder-parity'),
    'EN {n} and FR {n:>5} bind the same param n, so no placeholder-parity',
  );
});

test('literal scan: copy via compound += to an interpolated binding is flagged (#274 35-compound)', () => {
  const source = `fn C() -> Element { let mut label = String::new(); label += "Delete account"; rsx! { div { "{label}" } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && /Delete account/.test(f.message)),
    'copy appended via += to an interpolated binding must be flagged',
  );
});

test('literal scan: a non-BMP char literal bound as copy is flagged (#274 35-nonbmpchar)', () => {
  const source = `fn C() -> Element { let label = '𠀀'; rsx! { div { "{label}" } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text'),
    'a non-BMP char (surrogate pair) rendered as copy must be recorded and flagged',
  );
});

test('literal scan: a checkbox value is structural, a text value is copy (#274 35-checkboxvalue)', () => {
  const cb = `fn F() -> Element { rsx! { input { type: "checkbox", value: "daily_digest" } } }`;
  assert.ok(
    !scanComponentLiterals('x.rs', cb).some((f) => f.code === 'copy-attribute'),
    'a checkbox value is submitted data, not copy',
  );
  const txt = `fn F() -> Element { rsx! { input { type: "text", value: "Default name" } } }`;
  assert.ok(
    scanComponentLiterals('x.rs', txt).some((f) => f.code === 'copy-attribute'),
    'a text input default value is visible copy',
  );
});

test('literal scan: a nested block comment does not leak its brace (#274 24-5)', () => {
  // The inner comment contains a `}`: with a first-`*/` scan the comment ends at the
  // INNER terminator, leaking that `}` into the skeleton, closing the rsx! range early,
  // and letting the later copy escape. Depth-tracking closes at the OUTER `*/`.
  const source = `fn C() -> Element { rsx! { /* outer /* inner */ } still outer */ div { "Delete account" } } }`;
  const findings = scanComponentLiterals('x.rs', source);
  assert.ok(
    findings.some((f) => f.code === 'rust-text' && /Delete account/.test(f.message)),
    'the nested comment must close at the OUTER terminator so the later copy is still scanned',
  );
});

test('literal scan: dangerous_inner_html copy is flagged (#274 24-7)', () => {
  const source = `fn C() -> Element { rsx! { div { dangerous_inner_html: "<b>Delete account</b>" } } }`;
  const codes = scanComponentLiterals('x.rs', source).map((f) => f.code);
  assert.ok(codes.includes('copy-attribute'), 'dangerous_inner_html renders visible text — it is copy, not structural');
});

test('literal scan: a structural attr computed as if/else is NOT flagged; a copy attr IS (#274 24-8)', () => {
  const structural = `fn C() -> Element { rsx! { div { class: if selected { "selected-state" } else { "default-state" } } } }`;
  assert.ok(
    !scanComponentLiterals('x.rs', structural).some((f) => /selected-state|default-state/.test(f.message)),
    'a class computed inline as if/else is structural, not UI copy',
  );
  const copy = `fn C() -> Element { rsx! { SkipLink { label: if x { "Skip to rooms" } else { "Skip" } } } }`;
  assert.ok(
    scanComponentLiterals('x.rs', copy).some((f) => f.code === 'copy-attribute'),
    'a copy-bearing attr (label) computed as if/else still carries copy that must be flagged',
  );
});

test('the real jeliya-ui tree is clean across all groups', () => {
  const findings = checkJeliyaUiI18n({});
  assert.deepEqual(
    findings,
    [],
    `the actual catalogs/components must be clean:\n${findings.map((f) => `  ${f.file}:${f.line} [${f.code}] ${f.message}`).join('\n')}`,
  );
});
