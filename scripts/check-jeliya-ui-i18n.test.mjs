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

test('the real jeliya-ui tree is clean across all groups', () => {
  const findings = checkJeliyaUiI18n({});
  assert.deepEqual(
    findings,
    [],
    `the actual catalogs/components must be clean:\n${findings.map((f) => `  ${f.file}:${f.line} [${f.code}] ${f.message}`).join('\n')}`,
  );
});
