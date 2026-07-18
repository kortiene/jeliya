import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { afterEach, test } from 'node:test';

import {
  CATALOG_INTERFACE,
  EXEMPT_FILES,
  IDENTICAL_ALLOWLIST,
  LOCALE_FILES,
  checkUiI18n,
  identityExemption,
  parseCatalogInterface,
  parseLocaleCatalog,
  scanComponentLiterals,
  scanSource,
} from './check-ui-i18n.mjs';

// Every rule gets a fixture that PASSES and a fixture that FAILS. A gate is
// only worth its runtime if both are true of it: a rule that cannot fire is
// decoration, and a rule that fires on correct input is worse than none.

const tempRoots = [];

afterEach(() => {
  for (const root of tempRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});

/** A fixture repository. `allowlist`/`exemptFiles` default to empty so a
 *  fixture exercises the RULE, not today's exemptions. */
function repo(files) {
  const root = mkdtempSync(join(tmpdir(), 'jeliya-ui-i18n-'));
  tempRoots.push(root);
  for (const [path, contents] of Object.entries(files)) {
    const target = join(root, path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, contents);
  }
  return root;
}

function check(files, options = {}) {
  return checkUiI18n({
    repoRoot: repo(files),
    scanLiterals: false,
    allowlist: {},
    exemptFiles: {},
    ...options,
  });
}

function codes(findings) {
  return findings.map((entry) => entry.code);
}

const NNBSP = ' '; // narrow no-break space — before ; ! ? % and in « »
const NBSP = ' '; // no-break space — before :

function catalog(members) {
  return `export interface Catalog {\n${members.map((line) => `  ${line}\n`).join('')}}\n`;
}

function locale(name, entries) {
  return `import type { Catalog } from './catalog';\n\nexport const ${name}: Catalog = {\n${entries
    .map((line) => `  ${line}\n`)
    .join('')}};\n`;
}

/** The smallest tree that satisfies every rule: two keys, one plain and one
 *  parameterized, translated, with French typography. */
function cleanTree(overrides = {}) {
  return {
    [CATALOG_INTERFACE]: catalog([
      'localeTag: string;',
      'roomsCreate: Message;',
      'roomsMemberCount: MessageFn<[n: number]>;',
    ]),
    [LOCALE_FILES.en]: locale('en', [
      "localeTag: 'en',",
      "roomsCreate: 'Create a room',",
      'roomsMemberCount: (n) => `${n} members`,',
    ]),
    [LOCALE_FILES.fr]: locale('fr', [
      "localeTag: 'fr',",
      "roomsCreate: 'Créer un salon',",
      'roomsMemberCount: (n) => `${n} membres`,',
    ]),
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// The scanner the rules are built on
// ---------------------------------------------------------------------------

test('the scanner blanks comments, keeps line numbers, and splits template slots', () => {
  const source = [
    "// a commented 'literal' must not be a literal",
    '/* nor',
    '   a block one */',
    "const value = 'kept';",
    'const message = (n) => `${n} salons ouverts`;',
  ].join('\n');
  const { code, skeleton, literals } = scanSource(source);

  assert.equal(code.split('\n').length, source.split('\n').length);
  assert.equal(skeleton.split('\n').length, source.split('\n').length);
  assert.ok(!code.includes('commented'));
  assert.ok(!code.includes('block one'));
  // The literal survives in `code` and is blanked in `skeleton`, so structural
  // scanning can never trip over a brace inside copy.
  assert.ok(code.includes("'kept'"));
  assert.ok(!skeleton.includes('kept'));
  assert.deepEqual(
    literals.map((entry) => entry.value),
    ['kept', '', ' salons ouverts'],
  );
});

test('the interface parser reads member names and ignores MessageFn parameter labels', () => {
  const parsed = parseCatalogInterface(
    catalog([
      '/** doc comment: notAKey: Message; */',
      'localeTag: string;',
      'roomsPin: MessageFn<[room: string, at: number]>;',
      'roomsEmpty: Message;',
    ]),
  );

  assert.deepEqual(parsed.errors, []);
  assert.deepEqual(
    parsed.keys.map((member) => member.key),
    ['localeTag', 'roomsPin', 'roomsEmpty'],
  );
});

test('a module that is not a catalog is reported rather than read as empty', () => {
  const parsed = parseLocaleCatalog('export const fr = { roomsCreate: "x" };\n', 'fr.ts');
  assert.deepEqual(codes(parsed.errors), ['catalog-unparsed']);

  const spread = parseLocaleCatalog(
    "export const fr: Catalog = {\n  ...base,\n  roomsCreate: 'Créer',\n};\n",
    'fr.ts',
  );
  assert.deepEqual(codes(spread.errors), ['catalog-unparsed']);
});

// ---------------------------------------------------------------------------
// Rule 1 — key parity
// ---------------------------------------------------------------------------

test('a complete, translated pair of catalogs passes every rule', () => {
  assert.deepEqual(check(cleanTree()), []);
});

test('a key missing from one locale, or absent from the interface, is reported', () => {
  const findings = check(
    cleanTree({
      [LOCALE_FILES.fr]: locale('fr', ["localeTag: 'fr',", "roomsSurprise: 'Surprise',"]),
    }),
  );

  assert.deepEqual(
    findings.map((entry) => [entry.code, entry.message.split(': ').at(-1)]),
    [
      ['key-missing', 'roomsCreate'],
      ['key-missing', 'roomsMemberCount'],
      ['key-undeclared', 'roomsSurprise'],
    ],
  );
});

test('a missing locale file fails the gate rather than passing vacuously', () => {
  const tree = cleanTree();
  delete tree[LOCALE_FILES.fr];
  assert.deepEqual(codes(check(tree)), ['catalog-missing']);
});

// ---------------------------------------------------------------------------
// Rule 2 — empty values
// ---------------------------------------------------------------------------

test('an empty or whitespace-only value is reported in either locale', () => {
  const findings = check(
    cleanTree({
      [LOCALE_FILES.en]: locale('en', [
        "localeTag: 'en',",
        "roomsCreate: '',",
        'roomsMemberCount: (n) => `${n} members`,',
      ]),
      [LOCALE_FILES.fr]: locale('fr', [
        "localeTag: 'fr',",
        "roomsCreate: '   ',",
        'roomsMemberCount: (n) => `${n} membres`,',
      ]),
    }),
  );

  assert.deepEqual(codes(findings), ['value-empty', 'value-empty']);
  assert.ok(findings.some((entry) => entry.file === LOCALE_FILES.en));
  assert.ok(findings.some((entry) => entry.file === LOCALE_FILES.fr));
});

test('a message that renders a bare value opts out with i18n-exempt', () => {
  const bare = (name, tag) =>
    `import type { Catalog } from './catalog';\n\nexport const ${name}: Catalog = {\n` +
    `  localeTag: '${tag}',\n` +
    `  roomsCreate: 'x',\n` +
    '  // i18n-exempt: the count IS the message on a badge — no words to translate.\n' +
    '  roomsMemberCount: (n) => `${n}`,\n};\n';

  assert.deepEqual(
    codes(
      check(
        cleanTree({
          [LOCALE_FILES.en]: bare('en', 'en'),
          [LOCALE_FILES.fr]: bare('fr', 'fr'),
        }),
      ),
    ),
    // Both values are now the bare slot: identical, but exempt as punctuation.
    [],
  );
});

// ---------------------------------------------------------------------------
// Rule 3 — French left in English
// ---------------------------------------------------------------------------

test('a French value byte-identical to English is reported', () => {
  const findings = check(
    cleanTree({
      [LOCALE_FILES.fr]: locale('fr', [
        "localeTag: 'fr',",
        "roomsCreate: 'Create a room',",
        'roomsMemberCount: (n) => `${n} membres`,',
      ]),
    }),
  );

  assert.deepEqual(codes(findings), ['fr-untranslated']);
  assert.match(findings[0].message, /roomsCreate/);
});

test('identity is legitimate for the never-translate lexicon and for wordless values', () => {
  for (const [key, value] of [
    ['roomsCreate', 'Jeliya'],
    ['roomsCreate', 'Ticket'],
    ['roomsCreate', 'Agent'],
    ['roomsCreate', 'Pipe'],
    ['roomsCreate', 'jeliyad'],
    ['roomsCreate', 'direct'],
    ['roomsCreate', 'relay'],
    ['roomsCreate', 'unauthorized'],
    ['roomsCreate', '—'],
    ['roomsCreate', '99+'],
  ]) {
    assert.ok(identityExemption(key, value, {}), `${value} should be exempt automatically`);
  }
  assert.equal(identityExemption('roomsCreate', 'Create a room', {}), null);
});

test('an allowlist entry exempts a key, and a stale one is reported', () => {
  const identical = {
    [LOCALE_FILES.fr]: locale('fr', [
      "localeTag: 'fr',",
      "roomsCreate: 'Create a room',",
      'roomsMemberCount: (n) => `${n} membres`,',
    ]),
  };
  const reason = 'test fixture: the English happens to be the French.';

  assert.deepEqual(
    codes(check(cleanTree(identical), { allowlist: { roomsCreate: reason } })),
    [],
  );
  // Stale, sense 1: the values are no longer identical, so the exemption is
  // now a claim about copy nobody has re-read.
  assert.deepEqual(
    codes(check(cleanTree(), { allowlist: { roomsCreate: reason } })),
    ['allowlist-stale'],
  );
  // Stale, sense 2: the key is gone entirely.
  assert.deepEqual(
    codes(check(cleanTree(), { allowlist: { roomsRenamedAway: reason } })),
    ['allowlist-stale'],
  );
});

// ---------------------------------------------------------------------------
// Rule 4 — French typography
// ---------------------------------------------------------------------------

test('correct French typography passes', () => {
  const findings = check({
    [CATALOG_INTERFACE]: catalog([
      'localeTag: string;',
      'a: Message;',
      'b: Message;',
      'c: Message;',
      'd: MessageFn<[n: number]>;',
    ]),
    [LOCALE_FILES.en]: locale('en', [
      "localeTag: 'en',",
      "a: 'Really leave?',",
      "b: 'Identity: unknown',",
      "c: 'The room “Build” is archived…',",
      'd: (n) => `${n}% synced`,',
    ]),
    [LOCALE_FILES.fr]: locale('fr', [
      "localeTag: 'fr',",
      `a: 'Vraiment quitter${NNBSP}?',`,
      `b: 'Identité${NBSP}: inconnue',`,
      `c: 'Le salon «${NNBSP}Build${NNBSP}» est archivé…',`,
      `d: (n) => \`\${n}${NNBSP}% synchronisé\`,`,
    ]),
  });

  assert.deepEqual(findings, []);
});

test('every typography violation is reported, including inside a MessageFn', () => {
  const findings = check({
    [CATALOG_INTERFACE]: catalog([
      'localeTag: string;',
      'a: Message;',
      'b: Message;',
      'c: Message;',
      'd: Message;',
      'e: Message;',
      'f: MessageFn<[n: number]>;',
    ]),
    [LOCALE_FILES.en]: locale('en', [
      "localeTag: 'en',",
      "a: 'Leave A?',",
      "b: 'Identity: A',",
      "c: 'Identity of A',",
      "d: 'The room “A”',",
      "e: 'Loading A',",
      'f: (n) => `${n} percent of A`,',
    ]),
    [LOCALE_FILES.fr]: locale('fr', [
      "localeTag: 'fr',",
      // Plain space before '?' — must be U+202F.
      "a: 'Quitter B ?',",
      // Plain space before ':' — must be U+00A0.
      "b: 'Identité B :',",
      // Straight apostrophe — must be U+2019.
      "c: \"L'identité de B\",",
      // Curly double quotes — must be guillemets, and ... must be U+2026.
      "d: 'Le salon “B”...',",
      // Guillemets without the inner narrow space.
      "e: 'Le salon «B»',",
      // A break that straddles a slot boundary is still one sentence.
      'f: (n) => `${n} % de B`,',
    ]),
  });

  assert.deepEqual(codes(findings).sort(), [
    'fr-apostrophe',
    'fr-ellipsis',
    'fr-guillemet-space',
    'fr-narrow-space',
    'fr-narrow-space',
    'fr-no-break-space',
    'fr-quotes',
  ]);
  assert.ok(findings.every((entry) => entry.file === LOCALE_FILES.fr));
});

test('a no-break space where a NARROW one belongs is still reported', () => {
  const findings = check({
    [CATALOG_INTERFACE]: catalog(['localeTag: string;', 'a: Message;']),
    [LOCALE_FILES.en]: locale('en', ["localeTag: 'en',", "a: 'Leave?',"]),
    [LOCALE_FILES.fr]: locale('fr', ["localeTag: 'fr',", `a: 'Quitter${NBSP}?',`]),
  });

  assert.deepEqual(codes(findings), ['fr-narrow-space']);
  assert.match(findings[0].message, /U\+00A0/);
});

// ---------------------------------------------------------------------------
// Rule 5 — literals that never reach the catalog
// ---------------------------------------------------------------------------

test('a migrated component has no user-visible literals', () => {
  const source = `import { Glyph } from '../l10n/tokens';
import { useStrings } from '../l10n/strings';

export function RoomNav({ onCreate }: { onCreate(): void }) {
  const s = useStrings();
  return (
    <nav aria-label={s.destRooms} className="room-nav">
      <span aria-hidden="true">{Glyph.rooms}</span>
      <button type="button" title={s.roomsCreate} onClick={onCreate}>
        {s.roomsCreate}
      </button>
    </nav>
  );
}
`;
  assert.deepEqual(scanComponentLiterals('ui/src/components/RoomNav.tsx', source), []);
});

test('JSX text, copy attributes, and copy tables are reported', () => {
  const source = `const TABS = [{ key: 'rooms', label: 'Your Rooms' }];

export function Bar() {
  return (
    <nav aria-label="Primary">
      <h1>Your Rooms</h1>
      <input placeholder="Search rooms" />
      <button title="Create a room">{TABS[0].label}</button>
    </nav>
  );
}
`;
  const findings = scanComponentLiterals('ui/src/components/Bar.tsx', source);

  assert.deepEqual(codes(findings).sort(), [
    'copy-attribute',
    'copy-attribute',
    'copy-attribute',
    'copy-attribute',
    'jsx-text',
  ]);
  assert.ok(findings.every((entry) => entry.file === 'ui/src/components/Bar.tsx'));
});

test('rule 5 ignores comments, class names, and i18n-exempt lines', () => {
  const source = `// A comment mentioning <b>copy like this</b> is not copy.
export function Panel({ open }: { open: boolean }) {
  /* Nor is a title="Blocked" inside a block comment. */
  return (
    <div className="panel is-open" data-state={open ? 'open' : 'closed'}>
      {/* i18n-exempt: the wire value IS the label here — docs/i18n.md rule 3. */}
      <code className="mono">hash_mismatch</code>
      <span aria-hidden="true">{'\\u25a6'}</span>
    </div>
  );
}
`;
  assert.deepEqual(scanComponentLiterals('ui/src/components/Panel.tsx', source), []);
});

test('rule 5 does not mistake JSX slots or TypeScript generics for copy', () => {
  const source = `export function Panel({ rows }: { rows: Record<string, Row> }) {
  return (
    <Template
      template={s.panelPipeMeta}
      slots={{
        openedBy: <SenderName id={rows.openedBy} />,
        authorized: <SenderName id={rows.authorized} />,
      }}
    />
  );
}
`;
  assert.deepEqual(scanComponentLiterals('ui/src/components/Panel.tsx', source), []);
});

test('rule 5 catches visible copy immediately after a self-closing child', () => {
  const source = `export function DeleteButton() {
  return <button><Icon />Delete account</button>;
}
`;
  const findings = scanComponentLiterals('ui/src/components/DeleteButton.tsx', source);
  assert.deepEqual(codes(findings), ['jsx-text']);
  assert.match(findings[0].message, /Delete account/);
});

test('rule 5 skips the modules that stay English by decision', () => {
  const tree = cleanTree({
    'ui/src/App.tsx': 'export const App = () => <p>Untranslated copy here</p>;\n',
    'ui/src/components/Ok.tsx': "export const Ok = () => <p>{s.commonRetry}</p>;\n",
    'ui/src/lib/diagnostics.ts': "export const label = 'Daemon unreachable';\n",
  });
  const root = repo(tree);

  const withScan = checkUiI18n({
    repoRoot: root,
    scanLiterals: true,
    allowlist: {},
    exemptFiles: { 'ui/src/lib/diagnostics.ts': 'support artifact stays English' },
  });
  assert.deepEqual(
    withScan.map((entry) => [entry.file, entry.code]),
    [['ui/src/App.tsx', 'jsx-text']],
  );

  // And the same tree passes with the scan staged off, so an integrator can
  // ship rules 1-4 before the migration finishes.
  assert.deepEqual(
    checkUiI18n({ repoRoot: root, scanLiterals: false, allowlist: {}, exemptFiles: {} }),
    [],
  );
});

test('an exemption that names a file which no longer exists is reported', () => {
  const findings = check(cleanTree(), {
    exemptFiles: { 'ui/src/lib/moved-away.ts': 'a reason that outlived its file' },
  });
  assert.deepEqual(codes(findings), ['exempt-stale']);
});

// ---------------------------------------------------------------------------
// The real tree
// ---------------------------------------------------------------------------

test('the repository catalogs are complete, translated, and typographically French', () => {
  const findings = checkUiI18n({ scanLiterals: false });
  assert.deepEqual(
    findings.map((entry) => `${entry.file}:${entry.line} [${entry.code}] ${entry.message}`),
    [],
  );
});

test('every shipped exemption names a file that exists and gives a reason', () => {
  for (const [path, reason] of Object.entries(EXEMPT_FILES)) {
    assert.match(path, /^ui\/src\//, `${path} must be a repo-relative ui path`);
    assert.ok(reason.length > 40, `${path} needs a reason, not a label`);
  }
  for (const [key, reason] of Object.entries(IDENTICAL_ALLOWLIST)) {
    assert.ok(reason.length > 40, `${key} needs a reason, not a label`);
    assert.doesNotMatch(
      reason,
      /not translated|todo|later/i,
      `${key}: "not translated yet" is the finding, not the exemption`,
    );
  }
  // The stale halves of both rules run against the real tree here: if a listed
  // file moved, or an allowlisted key was renamed, this is where it surfaces.
  assert.deepEqual(codes(checkUiI18n({ scanLiterals: false })), []);
});
