// Unit + live tests for the 175c0b actor topology: the declared catalog,
// the four-axis resolver, the slot planner's RealNetwork mode rules, the
// bounded witness poll, and (with a real jeliyad) the RealNetwork canary,
// the missing-role refusal, and the total-cleanup leak check.
//
// Binary-free tests cover the pure modules. The live tests read JELIYAD_BIN
// and FAIL HARD when it is unset or missing — a silently-skipped canary
// reads as green while covering nothing (the hard-fail rule).
//
// Run: JELIYAD_BIN=/path/to/jeliyad node --test topology.test.mjs

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readdirSync, readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const HERE = dirname(fileURLToPath(import.meta.url));
const {
  ACTOR_CATALOG, DOMAIN_OVERRIDES, OBSERVE_ALIASES, SLOTS,
  resolveActor, planCaseTopology, caseNeedsRealNetwork, CaseTopology, pollUntil,
} = await import(join(HERE, 'topology.mjs'));

const CORPUS_DIR = join(HERE, '..');

/** Every label the corpus uses, with the domains that use it. */
function corpusLabels() {
  const byLabel = new Map();
  for (const f of readdirSync(CORPUS_DIR).filter((x) => x.endsWith('.json'))) {
    const cases = JSON.parse(readFileSync(join(CORPUS_DIR, f), 'utf8'));
    for (const c of cases.cases || []) {
      const s = JSON.stringify(c.steps || []);
      for (const m of s.matchAll(/"on":\s*"([^"]*)"/g)) {
        const label = m[1];
        if (!byLabel.has(label)) byLabel.set(label, new Set());
        byLabel.get(label).add(f);
      }
    }
  }
  return byLabel;
}

// ── Catalog meta-test: the corpus and the catalog cannot drift ─────────────

test('every actor label the corpus uses is declared in the catalog', () => {
  const labels = corpusLabels();
  for (const [label, domains] of labels) {
    if (label === 'none') continue; // observe-only unreachability probe
    // A label is declared iff it is a catalog entry / domain override /
    // observe alias, OR it is a `#N` connection suffix whose BASE is
    // declared in that domain (the connection-axis rule).
    const suffix = /^(.*)#[2-9][0-9]*$/.exec(label);
    const declaredIn = (domain) => {
      const base = suffix ? suffix[1] : label;
      return Object.prototype.hasOwnProperty.call(ACTOR_CATALOG, base)
        || Object.prototype.hasOwnProperty.call(DOMAIN_OVERRIDES[domain] ?? {}, label)
        || Object.prototype.hasOwnProperty.call(OBSERVE_ALIASES, label)
        || (suffix && Object.prototype.hasOwnProperty.call(DOMAIN_OVERRIDES[domain] ?? {}, base));
    };
    for (const domain of domains) {
      assert.ok(declaredIn(domain),
        `"${label}" (used in ${domain}) is not declared — add it to topology.mjs or refuse the label`);
    }
  }
});

test('every catalog entry resolves onto a declared slot with a known principal kind', () => {
  const kinds = new Set(['canonical', 'own', 'ephemeral', 'replace']);
  for (const [label, entry] of Object.entries(ACTOR_CATALOG)) {
    assert.ok(SLOTS.includes(entry.slot), `${label}: unknown slot ${entry.slot}`);
    assert.ok(kinds.has(entry.principal), `${label}: unknown principal kind ${entry.principal}`);
    // The label must resolve to its own entry.
    const a = resolveActor('', label);
    assert.equal(a.slot, entry.slot, label);
  }
});

test('the authority, invitee, member, member2 and outsider families are distinct slots', () => {
  // Role collapse guard: merging two families into one slot is exactly the
  // false-green the four-axis resolver exists to prevent (#46 needs three
  // distinct subjects).
  const slots = new Map();
  for (const [label, entry] of Object.entries(ACTOR_CATALOG)) {
    if (entry.principal !== 'canonical') continue;
    const prev = slots.get(entry.slot);
    if (prev) continue;
    slots.set(entry.slot, label);
  }
  for (const need of ['authority', 'invitee', 'member', 'member2', 'outsider']) {
    assert.ok(slots.has(need), `slot ${need} must have at least one canonical family label`);
  }
  const families = {
    authority: ['subject:A', 'subject:authority', 'subject:self'],
    invitee: ['subject:second', 'subject:invitee', 'subject:second_subject', 'subject:joiner'],
    member: ['subject:B', 'subject:member', 'subject:member_b', 'subject:member_nonagent'],
    member2: ['subject:D', 'subject:member_c'],
    outsider: ['subject:outsider', 'subject:bystander', 'subject:C'],
  };
  const slotOf = new Map(Object.entries(families).flatMap(([slot, labels]) =>
    labels.map((l) => [l, slot])));
  for (const [label, slot] of slotOf) {
    assert.equal(resolveActor('', label).slot, slot, `${label} must stay in the ${slot} family`);
  }
});

test('domain overrides are exactly where the labels are used with the other meaning', () => {
  const labels = corpusLabels();
  // subject:B's override domain must actually use subject:B, and no OTHER
  // domain may need it (the override is a recorded exception, not a habit).
  const bDomains = labels.get('subject:B') ?? new Set();
  assert.ok(bDomains.has('subject-daemon.json'), 'the subject-daemon override for subject:B is dead');
  assert.deepEqual(
    Object.keys(DOMAIN_OVERRIDES['subject-daemon.json']).sort(),
    ['subject:B', 'subject:C'],
  );
  // In the overridden domain the label routes to the authority slot; in every
  // other domain it stays the member family.
  assert.equal(resolveActor('subject-daemon.json', 'subject:B').slot, 'authority');
  assert.equal(resolveActor('pipes.json', 'subject:B').slot, 'member');
  const cDomains = labels.get('subject:C') ?? new Set();
  assert.ok(cDomains.has('subject-daemon.json'), 'the subject-daemon override for subject:C is dead');
  assert.equal(resolveActor('subject-daemon.json', 'subject:C').slot, 'authority');
  assert.equal(resolveActor('pipes.json', 'subject:C').slot, 'outsider');
});

test('c1 aliases the canonical extra connection and only appears beside subject:self#2', () => {
  const labelsByCase = [];
  for (const f of readdirSync(CORPUS_DIR).filter((x) => x.endsWith('.json'))) {
    const cases = JSON.parse(readFileSync(join(CORPUS_DIR, f), 'utf8'));
    for (const c of cases.cases || []) {
      const s = JSON.stringify(c.steps || []);
      if (s.includes('"c1"')) {
        labelsByCase.push({ f, name: c.name, hasSelf2: s.includes('"subject:self#2"') || s.includes('"subject:self#3"') });
      }
    }
  }
  assert.ok(labelsByCase.length > 0, 'the c1 alias must still correspond to real corpus usage');
  for (const e of labelsByCase) {
    assert.ok(e.hasSelf2, `${e.f}:${e.name} uses c1 without an extra canonical connection (self#2/self#3) — the alias would dangle`);
  }
  assert.equal(OBSERVE_ALIASES.c1, 'subject:self#2');
});

test('an undeclared label fails resolution with the catalog named', () => {
  for (const bad of ['subject:zzz', 'subject:E', 'connection:9', 'subject:', 'principal:third']) {
    assert.throws(() => resolveActor('', bad), /not declared in the harness actor catalog/, bad);
  }
});

// ── The four axes ──────────────────────────────────────────────────────────

test('#N suffixes affect the CONNECTION axis only — same slot, same principal', () => {
  const self = resolveActor('', 'subject:self');
  const self2 = resolveActor('', 'subject:self#2');
  const self7 = resolveActor('', 'subject:self#7');
  const a2conn = resolveActor('', 'subject:A#2');
  assert.equal(self2.slot, self.slot);
  assert.equal(self2.principalKey, self.principalKey);
  assert.equal(self2.connectionKind, 'extra');
  assert.equal(self7.connectionKind, 'extra');
  assert.equal(a2conn.slot, 'authority');
  assert.equal(a2conn.principalKey, 'canonical');
  assert.equal(a2conn.connectionKind, 'extra');
});

test('A2 is NOT a #N suffix — it is a distinct principal on the same subject', () => {
  const a = resolveActor('', 'subject:A');
  const a2 = resolveActor('', 'subject:A2');
  assert.equal(a2.slot, a.slot, 'A2 stays on the authority daemon/subject');
  assert.notEqual(a2.principalKey, a.principalKey, 'A2 must be a DISTINCT principal');
  assert.equal(a2.principalKind, 'own');
  // And self#2 vs A2 differ exactly on the principal axis.
  const self2 = resolveActor('', 'subject:self#2');
  assert.equal(self2.principalKey, a.principalKey, 'self#2 shares the canonical principal');
  assert.notEqual(a2.principalKey, self2.principalKey);
});

test('principal:self / principal:second are distinct cids on their base slots', () => {
  assert.equal(resolveActor('', 'principal:self').slot, 'authority');
  assert.equal(resolveActor('', 'principal:self').principalKind, 'own');
  assert.equal(resolveActor('', 'principal:second').slot, 'invitee');
  assert.equal(resolveActor('', 'principal:second').principalKind, 'own');
});

test('same_principal is the canonical principal reconnecting and replacing', () => {
  const sp = resolveActor('', 'subject:same_principal');
  assert.equal(sp.slot, 'authority');
  assert.equal(sp.principalKey, 'canonical');
  assert.equal(sp.connectionKind, 'replace');
});

test('session:cX is an attached session: ephemeral principal on the invitee slot', () => {
  const x = resolveActor('', 'session:cX');
  assert.equal(x.slot, 'invitee');
  assert.equal(x.principalKind, 'ephemeral');
});

test('the principal map: #N shares the canonical cid; own labels mint their own; reconnects keep it', () => {
  const t = new CaseTopology('');
  const self = t.principalIdFor('authority', 'canonical', 'canonical');
  const selfAgain = t.principalIdFor('authority', 'canonical', 'canonical');
  const a2 = t.principalIdFor('authority', 'subject:A2', 'own');
  const ps = t.principalIdFor('authority', 'principal:self', 'own');
  assert.equal(self, selfAgain, 'the canonical principal is stable across lookups (reconnects reuse it)');
  assert.notEqual(a2, self, 'A2 is a distinct principal');
  assert.notEqual(ps, self, 'principal:self is a distinct principal');
  assert.notEqual(a2, ps, 'two own-principal labels never share a principal');
  assert.equal(t.principalIdFor('authority', 'canonical', 'ephemeral'), null, 'ephemeral presents no cid');
});

// ── The slot planner and RealNetwork mode ──────────────────────────────────

test('non-link cases stay loopback — RealNetwork only when a daemon must reach another', () => {
  assert.equal(caseNeedsRealNetwork(['subject']), false);
  assert.equal(caseNeedsRealNetwork(['subject', 'room:live']), false);
  assert.equal(caseNeedsRealNetwork(['daemon:second', 'room:plain', 'subject:second']), false,
    'subject.ensure on a second daemon is local — loopback');
  assert.equal(caseNeedsRealNetwork(['subject:outsider']), false);
  assert.equal(caseNeedsRealNetwork(['link:up']), true);
  assert.equal(caseNeedsRealNetwork(['subject', 'room:live', 'link:up']), true);
  assert.equal(caseNeedsRealNetwork(['member:b', 'room:live']), true);
  assert.equal(caseNeedsRealNetwork(['member:c', 'room:live']), true);
  assert.equal(caseNeedsRealNetwork(['member:b', 'room:left']), true);
});

test('the slot plan stages one daemon per distinct subject', () => {
  const p = planCaseTopology(['subject']);
  assert.deepEqual(p.slots, ['authority']);
  assert.deepEqual(planCaseTopology(['daemon:second', 'room:plain']).slots, ['authority', 'invitee']);
  assert.deepEqual(planCaseTopology(['subject:outsider', 'room:plain']).slots, ['authority', 'outsider']);
  assert.deepEqual(planCaseTopology(['member:b', 'room:live']).slots, ['authority', 'member']);
  assert.deepEqual(planCaseTopology(['member:b', 'member:c', 'room:live']).slots, ['authority', 'member', 'member2']);
  // link:up with no peer stages the member slot as the link peer.
  assert.deepEqual(planCaseTopology(['subject', 'room:live', 'link:up']).slots, ['authority', 'member']);
  // link:up with an already-staged second subject uses it (no extra daemon).
  assert.deepEqual(planCaseTopology(['daemon:second', 'room:plain', 'room:live', 'link:up']).slots,
    ['authority', 'invitee']);
});

test('room:left stages the member slot itself (a departed member is its topology)', () => {
  const p = planCaseTopology(['room:left']);
  assert.deepEqual(p.slots, ['authority', 'member']);
  assert.equal(p.realNetwork, true, 'the depart join redeems cross-daemon — RealNetwork');
  // With member:b co-declared the plan is the same shape (one member slot).
  assert.deepEqual(planCaseTopology(['member:b', 'room:left']).slots, ['authority', 'member']);
});

// ── The bounded witness poll ───────────────────────────────────────────────

test('pollUntil returns once the probe converges', async () => {
  let n = 0;
  const r = await pollUntil({
    deadlineMs: 1000,
    describe: 'probe converges',
    intervalMs: 5,
    probe: async () => ({ done: ++n >= 3, detail: `n=${n}` }),
  });
  assert.equal(r.done, true);
  assert.ok(n >= 3);
});

test('pollUntil expiry is a loud error carrying the description and last detail', async () => {
  await assert.rejects(
    pollUntil({
      deadlineMs: 30,
      describe: 'the described convergence',
      intervalMs: 5,
      probe: async () => ({ done: false, detail: 'nothing synced' }),
    }),
    /did not converge within 30ms: the described convergence \(last: nothing synced\)/,
  );
});

// ── Live tests (JELIYAD_BIN; hard-fail when missing) ──────────────────────

function jeliyadBin() {
  const bin = process.env.JELIYAD_BIN;
  if (!bin) {
    throw new Error(
      'JELIYAD_BIN is not set — the live topology tests refuse to skip silently. ' +
        'Build jeliyad (cargo build --locked -p jeliyad) and export JELIYAD_BIN=/path/to/jeliyad',
    );
  }
  return bin;
}

test('the RealNetwork canary proves the live path (local gate; CI runs it too)', async () => {
  const { runRealNetworkCanary } = await import(join(HERE, 'canary.mjs'));
  const canary = await runRealNetworkCanary(jeliyadBin(), { deadlineMs: 60_000 });
  assert.ok(canary.redeemMs > 0 && canary.redeemMs < 60_000);
  assert.ok(canary.convergeMs < 60_000);
  assert.ok(canary.aPort > 0 && canary.bPort > 0 && canary.aPort !== canary.bPort);
});

test('total cleanup: RealNetwork and control-stopped runs leave zero new daemon temp dirs', async () => {
  const { Runner } = await import(join(HERE, 'runner.mjs'));
  const countDirs = () => readdirSync('/tmp').filter((d) => d.startsWith('jeliya-conf-')).length;
  const before = countDirs();
  const runner = new Runner(jeliyadBin());
  const rooms = JSON.parse(readFileSync(join(CORPUS_DIR, 'rooms.json'), 'utf8')).cases;
  const sd = JSON.parse(readFileSync(join(CORPUS_DIR, 'subject-daemon.json'), 'utf8')).cases;
  // A RealNetwork member case (alive-at-cleanup daemons) AND a case that
  // stops its own daemon via the stop_daemon control (a PRE-EXITED daemon at
  // cleanup — the exact shape whose early return used to leak one dir per
  // case).
  const c1 = rooms.find((x) => x.name === 'room_peers_on_a_live_room_reports_a_path_per_linked_device');
  const c2 = sd.find((x) => x.name === 'daemon_stop_flushes_its_reply_before_teardown');
  const r1 = await runner.runCase({ ...c1, _file: 'rooms.json' });
  const r2 = await runner.runCase({ ...c2, _file: 'subject-daemon.json' });
  const after = countDirs();
  assert.equal(r1.outcome, 'pass', r1.reason);
  assert.equal(r2.outcome, 'pass', r2.reason);
  // A leak strictly INCREASES the count; an unrelated dir disappearing on
  // this shared host must not false-red the probe.
  assert.ok(after <= before, `leaked ${after - before} daemon data dirs — total cleanup is broken`);
});

test('own-principal labels run on their OWN connections and cids (runner-level pin)', async () => {
  const { Runner, Outcome } = await import(join(HERE, 'runner.mjs'));
  const runner = new Runner(jeliyadBin());
  // The corpus case that pins the semantics: the SAME op_id from two
  // distinct session principals must both succeed (the dedup ledger is
  // keyed per principal). Collapsing the principals onto one connection
  // (the reviewer-caught bug) makes the second call an op_id_conflict —
  // a runner-caused false-red.
  const rooms = JSON.parse(readFileSync(join(CORPUS_DIR, 'rooms.json'), 'utf8')).cases;
  const c = rooms.find((x) =>
    x.name === 'room_create_dedup_ledger_is_scoped_to_the_session_principal_not_the_subject');
  const result = await runner.runCase({ ...c, _file: 'rooms.json' });
  assert.equal(result.outcome, Outcome.PASS, result.reason);
});

test('a missing binary fails CLEAN (message + nonzero, no leaked temp dir)', async () => {
  const { startDaemon } = await import(join(HERE, 'daemon.mjs'));
  const before = readdirSync('/tmp').filter((d) => d.startsWith('jeliya-conf-')).length;
  await assert.rejects(
    () => startDaemon('/nonexistent/jeliyad-definitely-missing'),
    /daemon failed to start:.*ENOENT/s,
  );
  const after = readdirSync('/tmp').filter((d) => d.startsWith('jeliya-conf-')).length;
  assert.ok(after <= before, `spawn failure leaked ${after - before} temp dirs`);
});

// NOTE (no separate red-demo test for the multi-store mutation watch): the
// dir baseline legitimately advances after EVERY daemon-interacting step
// (a successful op legally writes durably), so a DSL-authored
// "mutate-then-observe" sequence passes by design in both the single- and
// multi-store versions. The pre-fix gap (PR #312 review) was LATE ASYNC
// writes on a secondary daemon's store — the window between the last
// settle and a later observe — which no fixture can author on demand. The
// fix is verified by inspection (both #dirStateSignature and
// #stagingResidue iterate every staged daemon) plus the corpus-wide
// no-regression run; the scenario slices that will exercise it durably
// are 175c1+.

test('observe aliases resolve onto the connection that was opened (c1 pin)', async () => {
  const { Runner } = await import(join(HERE, 'runner.mjs'));
  const runner = new Runner(jeliyadBin());
  // The corpus's own c1 usage: subject:self#2 opens the extra connection,
  // then asserts `connection_open on c1`. Before the alias-target fix the
  // observe looked up the literal "c1" key and false-red with "no session
  // was ever opened for it" (PR #312 review).
  const tl = JSON.parse(readFileSync(join(CORPUS_DIR, 'timeline-streams.json'), 'utf8')).cases;
  const c = tl.find((x) => x.name === 'message_body_of_exactly_max_message_body_bytes_is_accepted');
  const result = await runner.runCase({ ...c, _file: 'timeline-streams.json' });
  assert.equal(result.outcome, 'pass', result.reason);
});

test('a label whose slot was never staged fails as a missing role (not default-to-primary)', async () => {
  const { Runner, Outcome } = await import(join(HERE, 'runner.mjs'));
  const runner = new Runner(jeliyadBin());
  const result = await runner.runCase({
    name: 'missing_role_probe', kind: 'success', operation: null,
    intent: 'subject:second with no invitee staged must refuse, not route to the primary.',
    requires: ['subject', 'room:live', 'member:b'],
    steps: [{ call: 'room.list', in: {}, on: 'subject:second' }],
    _file: 'rooms.json',
  });
  assert.equal(result.outcome, Outcome.ERROR);
  assert.match(result.reason, /missing role/);
  assert.match(result.reason, /subject:second/);
  assert.match(result.reason, /invitee/);
});

test('same_principal reconnects with the SAME cid and REPLACES the canonical connection (live)', async () => {
  const { Runner, Outcome } = await import(join(HERE, 'runner.mjs'));
  const runner = new Runner(jeliyadBin());
  // A dedup replay across the replacement proves all three properties at
  // once: the replacement presents the canonical principal's cid (else the
  // ledger misses and the op executes again), and it truly replaced the
  // canonical connection (else the old socket's ledger would answer from a
  // different principal). room.create is deterministic-safe to replay.
  const result = await runner.runCase({
    name: 'same_principal_replacement_probe', kind: 'success', operation: 'room.create',
    intent: 'same_principal = same cid, connection replaced; dedup ledger replays across it.',
    requires: ['subject'],
    steps: [
      { call: 'room.create', in: { name: 'dedup-room' }, on: 'subject:self', op_id: 'cf-same-principal-1',
        save: { rid: 'out.room_id' } },
      { call: 'room.create', in: { name: 'dedup-room' }, on: 'subject:same_principal', op_id: 'cf-same-principal-1',
        expect: { ok: true, out: { room_id: '$rid' } } },
      // And the canonical label resolves to the REPLACEMENT connection: the
      // same op_id replays there too (same principal, same ledger).
      { call: 'room.create', in: { name: 'dedup-room' }, on: 'subject:self', op_id: 'cf-same-principal-1',
        expect: { ok: true, out: { room_id: '$rid' } } },
      // Replacement made OBSERVABLE: disconnecting through the CANONICAL
      // label must close the REPLACEMENT connection (subject:self and
      // subject:same_principal are one connection after the takeover). A
      // stale alias would close some other socket and this observe would
      // time out.
      { control: { do: 'disconnect', on: 'subject:self' } },
      { assert: [{ observe: 'close_code', on: 'subject:same_principal', value: 1006 }] },
    ],
    _file: 'rooms.json',
  });
  assert.equal(result.outcome, Outcome.PASS, result.reason);
});

test('an undeclared label fails before any daemon spawns', async () => {
  const { Runner, Outcome } = await import(join(HERE, 'runner.mjs'));
  const runner = new Runner('/nonexistent/jeliyad-definitely-missing');
  const result = await runner.runCase({
    name: 'undeclared_label_probe', kind: 'success', operation: null,
    intent: 'A label outside the catalog must fail pre-spawn.',
    requires: ['subject'],
    steps: [{ call: 'room.list', in: {}, on: 'subject:zzz' }],
    _file: 'rooms.json',
  });
  assert.equal(result.outcome, Outcome.ERROR);
  assert.match(result.reason, /not declared in the harness actor catalog/);
  assert.ok(!result.reason.includes('daemon failed to start'), result.reason);
});

test('jobs>1 over a RealNetwork selection exits 2 before any daemon', async () => {
  const main = join(HERE, 'main.mjs');
  const res = spawnSync(process.execPath, [
    main, '/nonexistent/jeliyad-definitely-missing',
    '--case', 'room_peers_on_a_live_room_reports_a_path_per_linked_device',
    '--jobs', '2',
  ], { encoding: 'utf8', timeout: 30_000 });
  assert.equal(res.status, 2, `${res.stdout}${res.stderr}`);
  assert.match(`${res.stdout}${res.stderr}`, /--jobs 2 is not available for RealNetwork cases/);
  assert.ok(!`${res.stdout}${res.stderr}`.includes('daemon failed to start'));
});

test('loopback selections keep parallelism (the guard is mode-conditional)', async () => {
  const { selectCases, parseArgs, loadCases } = await import(join(HERE, 'main.mjs'));
  const selection = selectCases(loadCases(null), parseArgs(['bin', '--case', 'subject_ensure_creates_the_subject_on_a_fresh_daemon']));
  const realNetwork = selection.cases.filter((c) => caseNeedsRealNetwork(c.requires));
  assert.equal(realNetwork.length, 0, 'a loopback case must not trip the RealNetwork jobs guard');
});
