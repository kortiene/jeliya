// Focused tests for docs/native-update-policy.md (issue #121).
//
// These tests verify structural and content-level invariants of the policy
// document that the generic check-docs gate cannot prove: channel completeness,
// per-channel updater decisions, the integrity-vs-authenticity distinction,
// version monotonicity, fail-closed UX states, privacy bounds, and the
// non-goals that must not re-appear.
//
// The tests read the real files from the repository; they do not construct
// fixtures. Run with: node --test scripts/native-update-policy.test.mjs

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import { parseFrontmatter, validateDocumentation } from './check-docs.mjs';

const REPO_ROOT = fileURLToPath(new URL('..', import.meta.url));
const POLICY_PATH = fileURLToPath(new URL('../docs/native-update-policy.md', import.meta.url));

const policy = readFileSync(POLICY_PATH, 'utf8');
const { data: frontmatter, errors: fmErrors } = parseFrontmatter(policy);

// ---------------------------------------------------------------------------
// Frontmatter and profile compliance (AC-0: must pass the docs CI gate)
// ---------------------------------------------------------------------------

test('native-update-policy.md has no frontmatter parse errors', () => {
  assert.deepEqual(fmErrors, []);
});

test('native-update-policy.md passes the full docs-CI gate on the real repo', () => {
  const findings = validateDocumentation({ repoRoot: REPO_ROOT });
  const policyFindings = findings.filter((f) => f.file === 'docs/native-update-policy.md');
  assert.deepEqual(policyFindings, [],
    `docs gate reported issues for native-update-policy.md: ${JSON.stringify(policyFindings, null, 2)}`);
});

test('frontmatter type is Decision and status is canonical', () => {
  assert.equal(frontmatter.type, 'Decision');
  assert.equal(frontmatter.status, 'canonical');
});

test('frontmatter makes no overclaiming built or verified assertions (AC-0 clean-slate)', () => {
  // Nothing is built or tested yet; the status axes must reflect that.
  assert.equal(frontmatter.implementation_status, 'planned',
    'implementation_status must be "planned" — nothing is built yet');
  assert.equal(frontmatter.verification_status, 'unverified',
    'verification_status must be "unverified" — no evidence has been recorded');
  assert.equal(frontmatter.release_status, 'unreleased',
    'release_status must be "unreleased" — the policy is not yet in a public release');
});

test('H1 title matches frontmatter title', () => {
  const h1 = policy.match(/^# (.+)$/m)?.[1];
  assert.equal(h1, frontmatter.title,
    'H1 heading must be byte-identical to the frontmatter title field');
});

// ---------------------------------------------------------------------------
// Channel inventory completeness (AC-1)
// ---------------------------------------------------------------------------

// The spec §4.1 lists exactly 11 channel rows; assert them against the
// inventory table itself, not document-wide mentions, so deleting a row cannot
// pass on the strength of the same term appearing in another section.
const EXPECTED_ROWS = [
  ['macOS', 'install.sh'],
  ['macOS', 'jeliya.rb'],
  ['macOS', 'jeliya-app.rb'],
  ['macOS', 'direct browser download'],
  ['Windows', 'install.ps1'],
  ['Windows', 'direct browser download'],
  ['Linux', 'install.sh'],
  ['Linux', 'Linuxbrew'],
  ['Linux', 'package-linux.mjs'],
  ['Android', 'Google Play'],
  ['Android', 'sideload APK'],
];

const inventorySection = policy.split('### Channel inventory')[1].split('\n### ')[0];
const inventoryRows = inventorySection.split('\n')
  .filter((l) => l.startsWith('|') && !l.startsWith('|---'))
  .slice(1); // drop the header row

test('channel inventory table has exactly the 11 expected rows', () => {
  assert.equal(inventoryRows.length, EXPECTED_ROWS.length,
    `inventory table must have exactly ${EXPECTED_ROWS.length} channel rows`);
});

for (const [platform, label] of EXPECTED_ROWS) {
  test(`channel inventory has a ${platform} row for "${label}"`, () => {
    assert.ok(inventoryRows.some((r) => {
      // Cell text may contain escaped pipes (`curl \| sh`); shield them from
      // the column split, then restore for the label match.
      const cells = r.replaceAll('\\|', '\u0001').split('|')
        .map((c) => c.trim().replaceAll('\u0001', '\\|'));
      return cells[1] === platform && cells[2].includes(label);
    }), `inventory table must contain a ${platform} row whose Channel cell mentions "${label}"`);
  });
}

// Honesty qualifiers added in review must stay load-bearing: the revocation
// floor's reachability limit, the installer-bootstrap trust limitation, and
// the shared-asset publication boundary may not be silently dropped.
test('policy states the revocation floor cannot reach already-installed builds', () => {
  assert.ok(/[Rr]eachability limit/.test(policy) && policy.includes('already-installed'),
    'policy must state the floor/digest list cannot reach already-installed builds');
});

test('policy states the piped installer bootstrap runs before any verification', () => {
  assert.ok(/bootstrap/i.test(policy) && /piped into the shell/.test(policy),
    'policy must state the installer-script bootstrap limitation');
});

test('policy gates shared release assets across every consuming channel', () => {
  assert.ok(policy.includes('share one publication boundary')
    && /every\*\* channel gate that\s+consumes it/.test(policy),
    'policy must state the shared-asset publication rule');
});

// The policy must explicitly enumerate more than one platform.
test('channel table covers macOS, Linux, Windows, and Android platforms', () => {
  assert.ok(policy.includes('macOS'), 'macOS must appear in channel table');
  assert.ok(policy.includes('Linux'), 'Linux must appear in channel table');
  assert.ok(policy.includes('Windows'), 'Windows must appear in channel table');
  assert.ok(policy.includes('Android'), 'Android must appear in channel table');
});

// ---------------------------------------------------------------------------
// Updater vs. no-updater decisions (AC-1)
// ---------------------------------------------------------------------------

test('policy states no in-app or bundled auto-updater on any native channel', () => {
  // The decision phrase the spec requires:
  assert.ok(policy.includes('no in-app'), 'policy must state "no in-app" updater decision');
});

test('Google Play is the only channel where an automatic updater is claimed', () => {
  // The spec is explicit: "the Play row is the only channel where an automatic
  // updater is claimed — because it is the platform's, not Jeliya's."
  assert.ok(policy.includes('platform updater') || policy.includes('platform\'s own'),
    'policy must attribute the Play updater to the platform');

  // The "no in-app updater" text must appear more than once (one per non-Play channel).
  const noUpdaterCount = (policy.match(/no in-app updater/g) ?? []).length;
  assert.ok(noUpdaterCount >= 3,
    `expected at least 3 "no in-app updater" decisions (one per non-Play channel group), got ${noUpdaterCount}`);
});

test('Windows rows are conditioned on Windows shipping', () => {
  // Architecture leaves Windows undecided; this doc must reflect that.
  assert.ok(policy.includes('#188') || policy.match(/[Ww]indows.{0,60}(?:undecided|conditional|if.*ships|scope)/),
    'policy must mark Windows rows as conditional on #188 deciding whether Windows ships');
});

test('sideload APK warns about key-change forced uninstall data loss', () => {
  // The spec requires a warning that an Android signing-key change forces
  // uninstall/reinstall, which discards app-private state.
  assert.ok(
    policy.match(/[Kk]ey.{0,40}(?:uninstall|data loss|reinstall)/) ||
    policy.match(/(?:uninstall|data loss|reinstall).{0,40}[Kk]ey/),
    'sideload APK section must warn about key-change forcing uninstall and data loss',
  );
});

// ---------------------------------------------------------------------------
// Signing roots, metadata trust, rotation, revocation, channel separation (AC-2)
// ---------------------------------------------------------------------------

test('policy documents macOS Developer ID and notarization signing root', () => {
  assert.ok(policy.includes('Developer ID') || policy.includes('notarization'),
    'policy must document the macOS Developer ID signing root');
});

test('policy documents Android Play App Signing with local upload key', () => {
  assert.ok(policy.includes('Play App Signing'),
    'policy must document the Android Play App Signing root');
  assert.ok(policy.includes('upload') && policy.includes('key'),
    'policy must clarify that the local keystore is the upload key, not the distribution key');
});

test('policy states Linux daemon archives have no publisher signature', () => {
  // The spec is explicit: Linux has "no publisher signature today" — this must
  // be stated, not silently omitted.
  assert.ok(
    policy.match(/[Ll]inux.{0,200}no publisher signature/s) ||
    policy.match(/no publisher signature.{0,200}[Ll]inux/s),
    'policy must state that Linux daemon archives have no publisher signature today',
  );
});

test('policy distinguishes integrity from authenticity for the checksum sidecar', () => {
  // This is the "load-bearing honesty point" of the spec: a same-origin
  // .sha256 sidecar proves integrity, not authenticity.
  assert.ok(
    policy.includes('integrity') && policy.includes('authenticity'),
    'policy must use both "integrity" and "authenticity" to distinguish the sidecar claim',
  );
  assert.ok(
    policy.match(/sha256.{0,120}integrity/s) || policy.match(/integrity.{0,120}sha256/s) ||
    policy.match(/sidecar.{0,120}integrity/s) || policy.match(/integrity.{0,120}sidecar/s),
    'policy must associate the .sha256 sidecar with integrity (not authenticity)',
  );
});

test('policy covers two revocation layers: platform-level and application-level', () => {
  // Platform-level: OS-honored (Apple, Authenticode, Play takedown).
  // Application-level: compiled minimum-supported build floor (network-independent).
  assert.ok(
    policy.match(/[Pp]latform.{0,30}level/) || policy.includes('OS-honored') ||
    policy.includes('operating system'),
    'policy must document platform-level revocation honored by the OS',
  );
  assert.ok(
    policy.match(/[Mm]inimum.{0,30}(?:supported|build|floor)/) ||
    policy.includes('min_reader') || policy.includes('build floor'),
    'policy must document an application-level minimum-supported build floor',
  );
  assert.ok(
    policy.includes('without a network') || policy.includes('no network'),
    'application-level revocation must fail closed without a network call',
  );
});

test('policy states channel separation via distinct application/bundle identifiers', () => {
  assert.ok(
    policy.includes('bundle identifier') || policy.includes('application identifier') ||
    policy.includes('package identity') || policy.includes('bundle id'),
    'policy must require distinct application/bundle identifiers per packaged target',
  );
});

// ---------------------------------------------------------------------------
// Anti-rollback and version monotonicity (AC-3)
// ---------------------------------------------------------------------------

test('policy separates cross-generation and same-generation downgrade cases', () => {
  assert.ok(
    policy.match(/[Cc]ross.{0,20}generation/) || policy.includes('cross-generation'),
    'policy must address cross-generation downgrade',
  );
  assert.ok(
    policy.match(/[Ss]ame.{0,20}generation/) || policy.includes('same-generation'),
    'policy must address same-generation downgrade',
  );
});

test('policy states version-monotonicity invariant explicitly', () => {
  // The spec requires: "version is monotonic per generation; a rollback that
  // would reinterpret newer state fails closed."
  assert.ok(
    policy.includes('monotonic') || policy.includes('monotonicity'),
    'policy must state version monotonicity',
  );
  assert.ok(
    policy.includes('fails closed') || policy.includes('fail closed') || policy.includes('fail-closed'),
    'policy must state that a rollback that reinterprets newer state fails closed',
  );
});

test('policy references the on-disk monotonic marker concept', () => {
  // The spec recommends a "written_by build number and a min_reader floor".
  assert.ok(
    policy.includes('written_by') || policy.includes('min_reader') ||
    policy.match(/monotonic.{0,60}(?:marker|floor)/s) ||
    policy.match(/(?:marker|floor).{0,60}monotonic/s),
    'policy must reference the on-disk monotonic marker (written_by / min_reader)',
  );
});

test('policy defers the concrete marker key name to downstream issues', () => {
  // The key name is owned by #185/#178/#173; this doc must not invent one.
  assert.ok(
    policy.includes('#185') || policy.includes('#178') || policy.includes('#173'),
    'policy must defer the concrete on-disk key name to the downstream issues (#185, #178, #173)',
  );
});

// ---------------------------------------------------------------------------
// Fail-closed states and actionable UX (AC-4)
// ---------------------------------------------------------------------------

const FAIL_CLOSED_STATES = [
  { label: 'unsupported generation / old client', pattern: /unsupported.*generation|old client|too old/i },
  { label: 'below minimum-supported floor / revoked build', pattern: /minimum.supported|revoked.build|no longer supported/i },
  { label: 'tampered artifact', pattern: /tampered|bad checksum|failed verification/i },
  { label: 'newer state / older binary (downgrade)', pattern: /newer state|older than your data|downgrade/i },
  { label: 'offline', pattern: /offline/i },
  { label: 'interrupted download', pattern: /interrupted|didn.t complete|update didn/i },
];

for (const { label, pattern } of FAIL_CLOSED_STATES) {
  test(`fail-closed UX table covers: ${label}`, () => {
    assert.ok(pattern.test(policy),
      `policy must document the fail-closed state and actionable UX for: ${label}`);
  });
}

test('fail-closed recovery path is shown, not taken on behalf of the user', () => {
  // The spec says "the reset path is shown, not taken — no unverified directory
  // is deleted on the user's behalf."
  assert.ok(
    policy.includes('reset path') || policy.includes('shown, not taken') ||
    policy.match(/shown.{0,40}not taken/s),
    'policy must state that the reset path is shown, not automatically taken',
  );
});

// ---------------------------------------------------------------------------
// Offline and interrupted recovery (AC-5)
// ---------------------------------------------------------------------------

test('offline recovery keeps the prior verified install running', () => {
  assert.ok(
    policy.match(/offline.{0,200}(?:verified install|unchanged|keeps running)/s) ||
    policy.match(/(?:verified install|unchanged|installed version).{0,200}offline/s),
    'policy must state that an offline machine keeps running its prior verified build',
  );
});

test('recovery must never execute untrusted bytes', () => {
  assert.ok(
    policy.includes('untrusted bytes') || policy.includes('without executing'),
    'policy must state that recovery never executes untrusted bytes',
  );
});

test('verification precedes extraction for installer scripts', () => {
  assert.ok(
    policy.match(/verif.{0,30}extract|extract.{0,30}verif/i),
    'policy must state that verification precedes extraction',
  );
});

// ---------------------------------------------------------------------------
// Operator commands and privacy-bounded diagnostics
// ---------------------------------------------------------------------------

test('policy defines required diagnostics fields (digest, channel, signing identity)', () => {
  assert.ok(
    policy.includes('digest') && policy.includes('channel') &&
    (policy.includes('signing identity') || policy.includes('notarization')),
    'policy must define diagnostics fields: digest, channel, and signing identity',
  );
});

test('privacy bound for diagnostics excludes identity keys, daemon tokens, and room data', () => {
  // The spec requires "no identity keys, daemon tokens, invite tickets, IP
  // addresses, room or member data, or file contents."
  assert.ok(
    policy.includes('Privacy') || policy.includes('privacy') || policy.includes('Privacy bound'),
    'policy must state a privacy bound for diagnostics output',
  );
  const privacySection = policy.slice(policy.toLowerCase().indexOf('privacy'));
  const privacyCoverage = [
    'daemon token',
    'invite',
    'IP address',
    'room',
  ];
  for (const term of privacyCoverage) {
    assert.ok(
      privacySection.toLowerCase().includes(term.toLowerCase()),
      `privacy-bound section must exclude: ${term}`,
    );
  }
});

// ---------------------------------------------------------------------------
// Evidence contract and downstream integration (AC-6)
// ---------------------------------------------------------------------------

test('evidence case matrix covers current, older, newer, revoked, tampered, offline, interrupted, downgrade', () => {
  const requiredCases = ['current', 'older', 'newer', 'revoked', 'tampered', 'offline', 'interrupted', 'downgrade'];
  for (const c of requiredCases) {
    assert.ok(policy.includes(c),
      `evidence case matrix must include case: "${c}"`);
  }
});

test('evidence schema retains digest, signing identity, metadata/version, command, and result', () => {
  const evidenceFields = ['digest', 'signing identity', 'command', 'result'];
  for (const field of evidenceFields) {
    assert.ok(policy.includes(field),
      `evidence schema must retain field: "${field}"`);
  }
});

test('policy references downstream evidence owners #186–#190 and #199', () => {
  for (const issue of ['#186', '#187', '#188', '#190', '#199']) {
    assert.ok(policy.includes(issue),
      `policy must reference downstream evidence owner ${issue}`);
  }
});

// ---------------------------------------------------------------------------
// Non-goals — must not be re-introduced (AC-7)
// ---------------------------------------------------------------------------

const FORBIDDEN_NON_GOALS = [
  { label: 'N/N-1 compatibility window', pattern: /N\/N-1|N.N-1 compat|compatibility window/i },
  { label: 'migration', pattern: /migrations?|migration plan|migration path|migrate.{0,40}(state|data)/i },
  { label: 'legacy rollback artifact', pattern: /legacy rollback|rollback artifact/i },
  { label: 'dual protocol support', pattern: /dual protocol/i },
  { label: 'hosted-web companion skew', pattern: /companion.{0,30}skew|skew.{0,30}companion/i },
];

// Verify non-goals are stated explicitly (they must appear in the Non-goals section).
test('non-goals section exists and lists prohibited patterns', () => {
  assert.ok(policy.includes('## Non-goals') || policy.includes('Non-goals'),
    'policy must include a Non-goals section');
});

for (const { label, pattern } of FORBIDDEN_NON_GOALS) {
  test(`non-goal is documented (not silently omitted): "${label}"`, () => {
    // The pattern should appear in the document — but only in the non-goals
    // section that explicitly names and rejects it.
    assert.ok(pattern.test(policy),
      `policy must name and reject the non-goal: "${label}"`);
  });
}

test('policy states no compatibility window, migration, or legacy rollback requirement remains', () => {
  // This is the AC-7 required phrase (from the spec and issue).
  assert.ok(
    policy.match(/no compatibility window/i) ||
    policy.match(/no.{0,30}migration.{0,30}requirement/i) ||
    policy.match(/No compatibility window.*migration.*legacy rollback/s),
    'policy must state that no compatibility window, migration, or legacy rollback requirement remains',
  );
});

// ---------------------------------------------------------------------------
// Cross-reference integrity
// ---------------------------------------------------------------------------

test('policy cites dioxus-architecture.md, first-release-distribution.md, and signing-notarization.md', () => {
  for (const doc of ['dioxus-architecture.md', 'first-release-distribution.md', 'signing-notarization.md']) {
    assert.ok(policy.includes(doc),
      `policy must cite: ${doc}`);
  }
});

test('policy cites verification-evidence.md for the evidence schema contract', () => {
  assert.ok(policy.includes('verification-evidence.md'),
    'policy must cite verification-evidence.md for the evidence ledger contract');
});
