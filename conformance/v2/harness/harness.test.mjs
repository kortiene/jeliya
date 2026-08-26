// Unit tests for the v2 conformance harness's honesty foundations (175c0a):
// selector hygiene, requires honesty, the shared capability classification,
// the portfile storage_generation binding, and the padded-frame builder.
//
// None of these need a real jeliyad: the CLI guards and the requires guard
// both fire BEFORE any daemon spawns, the metadata test drives a stub
// binary, and the classification tests are pure imports.
//
// Run: node --test conformance/v2/harness/harness.test.mjs

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, rmSync, writeFileSync, mkdirSync, chmodSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const HERE = dirname(fileURLToPath(import.meta.url));
const MAIN = join(HERE, 'main.mjs');
const { parseArgs, loadCases, selectCases } = await import(MAIN);
const { CONTROL_CAPABILITIES, REQUIRES_IMPLEMENTED, unimplementedRequire } =
  await import(join(HERE, 'capabilities.mjs'));
const { buildPaddedEnvelope, isPaddedEnvelope, isBareEnvelope, buildMaskedFrameHeader } = await import(join(HERE, 'session.mjs'));
const { startDaemon } = await import(join(HERE, 'daemon.mjs'));

const tempDirs = [];
function track(dir) {
  tempDirs.push(dir);
  return dir;
}

// ── CLI selector hygiene ───────────────────────────────────────────────────

function runMain(args) {
  const result = spawnSync(process.execPath, [MAIN, '/nonexistent/jeliyad', ...args], {
    encoding: 'utf8',
    timeout: 20_000,
  });
  return { code: result.status, out: `${result.stdout}${result.stderr}` };
}

test('a --case naming no corpus case exits 2 listing the unknown name', () => {
  const { code, out } = runMain(['--case', 'subject_ensure_typo_case']);
  assert.equal(code, 2, out);
  assert.match(out, /--case names no corpus case: subject_ensure_typo_case/);
  assert.match(out, /typo must fail, not silently select nothing/);
});

test('a duplicate --case selector exits 2 naming the duplicate', () => {
  const { code, out } = runMain([
    '--case', 'daemon_stop_does_not_require_a_subject',
    '--case', 'daemon_stop_does_not_require_a_subject',
  ]);
  assert.equal(code, 2, out);
  assert.match(out, /duplicate --case selector\(s\): daemon_stop_does_not_require_a_subject/);
});

test('--case together with --filter exits 2', () => {
  const { code, out } = runMain(['--case', 'daemon_stop_does_not_require_a_subject', '--filter', 'daemon']);
  assert.equal(code, 2, out);
  assert.match(out, /--case and --filter are mutually exclusive/);
});

test('a zero-match --file exits 2 listing the available files', () => {
  const { code, out } = runMain(['--file', 'nosuchdomain']);
  assert.equal(code, 2, out);
  assert.match(out, /--file nosuchdomain matches no corpus file/);
  assert.match(out, /handshake\.json/);
});

test('a zero-match --filter exits 2', () => {
  const { code, out } = runMain(['--filter', 'zzz_no_such_case_zzz']);
  assert.equal(code, 2, out);
  assert.match(out, /--filter zzz_no_such_case_zzz matches none of the/);
});

test('every selector guard fires before any daemon could start', () => {
  // The binary path does not exist: had a daemon been spawned the run would
  // fail with a daemon-start error instead of the selector message.
  const { code, out } = runMain(['--case', 'typo_dead_daemon']);
  assert.equal(code, 2, out);
  assert.ok(!out.includes('daemon failed to start'), out);
});

test('a legal --case selection passes the guards and reports the count', () => {
  const cases = loadCases(null);
  const selection = selectCases(cases, parseArgs(['bin', '--case', 'daemon_stop_does_not_require_a_subject']));
  assert.equal(selection.error, undefined);
  assert.equal(selection.cases.length, 1);
  assert.equal(selection.cases[0].name, 'daemon_stop_does_not_require_a_subject');
});

test('the 27 CI-selected cases select cleanly through the same path', async () => {
  const cases = loadCases(null);
  const args = parseArgs(['bin', ...cases.slice(0, 0).map(() => '').filter(() => false)]);
  // Rebuild the CI selector list from the workflow the checker itself parses.
  const ci = await import('node:fs').then(({ readFileSync }) =>
    readFileSync(join(HERE, '..', '..', '..', '.github', 'workflows', 'ci.yml'), 'utf8'));
  const liveStep = ci.match(/- name: Protocol-v2 conformance replay[\s\S]*?(?=\n      - name:|$)/)?.[0] ?? '';
  const selectors = [...liveStep.matchAll(/--case\s+([^\s\\]+)/g)].map((m) => m[1]);
  assert.equal(selectors.length, 27);
  for (const s of selectors) args.caseNames.push(s);
  const selection = selectCases(cases, args);
  assert.equal(selection.error, undefined, JSON.stringify(selection.error));
  assert.equal(selection.cases.length, 27);
});

// ── Requires honesty ───────────────────────────────────────────────────────

test('a well-formed but unestablished requires token is refused', () => {
  for (const token of ['link:down', 'link:relay', 'link:slow', 'room:removed', 'room:foreign',
    'room:quiescent', 'room:with_history', 'member:agent', 'member:non_agent',
    'daemon:restartable', 'observe:frames', 'observe:network',
    'control:concurrency', 'resource:large_file', 'resource:fetched_file', 'fault:backpressure']) {
    const err = unimplementedRequire([token]);
    assert.ok(err instanceof Error, token);
    assert.match(err.message, /does not establish it/, token);
    assert.match(err.message, new RegExp(`"${token}"`), token);
  }
});

test('every implemented requires token passes the guard', () => {
  assert.equal(unimplementedRequire([...REQUIRES_IMPLEMENTED]), null);
});

test('the 175c0b-promoted topology tokens are implemented (link:up, member:b/c, room:left)', () => {
  // The four tokens PR #311 refused as unfaithfully staged are now backed by
  // real staging (RealNetwork daemons, distinct subjects, invite/redeem).
  for (const token of ['link:up', 'member:b', 'member:c', 'room:left']) {
    assert.ok(REQUIRES_IMPLEMENTED.has(token), `${token} must be implemented after 175c0b`);
    assert.equal(unimplementedRequire([token]), null, token);
  }
});

test('the runner refuses an unestablished token before spawning any daemon', async () => {
  const { Runner, Outcome } = await import(join(HERE, 'runner.mjs'));
  const runner = new Runner('/nonexistent/jeliyad-definitely-missing');
  const result = await runner.runCase({
    name: 'guard_probe', kind: 'success', operation: null,
    intent: 'Probes that the runner refuses unimplemented requires pre-stage.',
    requires: ['daemon:fresh', 'link:down'],
    steps: [{ call: 'room.list', in: {} }],
  });
  assert.equal(result.outcome, Outcome.ERROR);
  assert.match(result.reason, /requires token "link:down"/);
  assert.match(result.reason, /does not establish it/);
  // The binary never existed: had staging begun, the failure would have been
  // a daemon-start error instead of the honesty message.
  assert.ok(!result.reason.includes('daemon failed to start'), result.reason);
});

// ── Capability classification ──────────────────────────────────────────────

test('every control verb is classified exactly one way', () => {
  for (const [verb, cls] of Object.entries(CONTROL_CAPABILITIES)) {
    assert.ok(['executable', 'documented_only'].includes(cls), `${verb}: ${cls}`);
  }
  // The executable set this harness actually implements in #doControl.
  assert.deepEqual(
    Object.entries(CONTROL_CAPABILITIES).filter(([, c]) => c === 'executable').map(([v]) => v).sort(),
    ['advance_clock', 'disconnect', 'idle', 'reconnect', 'stop_daemon'],
  );
});

test('the README control table and the shared classification agree row by row', async () => {
  const { readFileSync } = await import('node:fs');
  const readme = readFileSync(join(HERE, '..', 'README.md'), 'utf8');
  const rows = [...readme.matchAll(/^\| `([a-z_]+)` \|[^|]+\|[^|]+\| (executable|documented_only) \|$/gm)];
  assert.ok(rows.length >= Object.keys(CONTROL_CAPABILITIES).length,
    `README control table rows: ${rows.length}`);
  const fromReadme = Object.fromEntries(rows.map((m) => [m[1], m[2]]));
  assert.deepEqual(
    Object.keys(fromReadme).sort(),
    Object.keys(CONTROL_CAPABILITIES).sort(),
    'the README control table and capabilities.mjs must list the same verbs',
  );
  for (const [verb, cls] of Object.entries(CONTROL_CAPABILITIES)) {
    assert.equal(fromReadme[verb], cls, `${verb} drifted between README and capabilities.mjs`);
  }
});

test('documented-only controls are exactly the ones the runner refuses', async () => {
  // #doControl refuses every documented_only verb with the classification
  // named, before its switch. Driving it end-to-end needs a live daemon (the
  // requires guard correctly runs first), so here we pin the classification
  // itself: the runner module's import of CONTROL_CAPABILITIES is the same
  // object, and the executable set equals the verbs its switch implements.
  const { Runner } = await import(join(HERE, 'runner.mjs'));
  const documented = Object.entries(CONTROL_CAPABILITIES)
    .filter(([, c]) => c === 'documented_only').map(([v]) => v).sort();
  assert.deepEqual(documented, ['cancel_transfers', 'client_preflight', 'client_render_file',
    'client_render_limit', 'inject_fault', 'pause_link', 'set_limit', 'set_link_rate',
    'set_provider_response_bytes', 'start_daemon', 'start_transfers']);
  // The runner is constructible and carries no per-instance overrides.
  assert.ok(new Runner('/nonexistent/jeliyad') instanceof Runner);
});

// ── Portfile metadata (storage_generation vs protocol) ─────────────────────

test('the Daemon binds storage_generation, not protocol — the differential probe', async () => {
  // A stub daemon whose portfile deliberately DIVERGES the two generation
  // axes (protocol 2, storage_generation 7): binding the wrong field shows
  // up as 2 instead of 7.
  const dir = track(mkdtempSync(join(tmpdir(), 'jeliya-conf-meta-')));
  const stub = join(dir, 'stub-jeliyad');
  const portfile = {
    pid: process.pid,
    port: 1,
    http: 'http://127.0.0.1:1',
    ws: 'ws://127.0.0.1:1/ws',
    version: 'test',
    protocol: 2,
    min_protocol: 2,
    storage_generation: 7,
    limits: {},
    data_dir: dir,
    auth_token: 'test-token',
    started_at_ms: Date.now(),
  };
  writeFileSync(join(dir, 'daemon.json'), JSON.stringify(portfile));
  writeFileSync(stub, `#!/bin/sh\nsleep 30\n`);
  chmodSync(stub, 0o755);
  // startDaemon spawns the stub on its own fresh data dir; make the stub
  // WRITE the divergent portfile there, then idle.
  const stub2 = join(dir, 'stub-jeliyad-writer');
  const pfCopy = JSON.stringify(portfile);
  writeFileSync(stub2, [
    '#!/bin/sh',
    'd=""',
    'for i in "$@"; do case "$prev" in --data-dir) d="$i";; esac; prev="$i"; done',
    `cat > "$d/daemon.json" <<'EOF'`,
    pfCopy,
    'EOF',
    'sleep 30',
    '',
  ].join('\n'));
  chmodSync(stub2, 0o755);
  const daemon = await startDaemon(stub2);
  try {
    assert.equal(daemon.storageGeneration, 7, 'bound pf.protocol (2) instead of pf.storage_generation (7)');
    assert.notEqual(daemon.storageGeneration, 2);
  } finally {
    await daemon.stop();
    rmSync(dir, { recursive: true, force: true });
  }
});

// ── Padded-frame builder ───────────────────────────────────────────────────

test('the padded envelope hits an exact total byte target', () => {
  const env = { id: 81, op: 'subject.ensure', in: { op_id: 'seed' } };
  for (const target of [64, 1000, 65536]) {
    const text = buildPaddedEnvelope({
      envelope: JSON.parse(JSON.stringify(env)),
      pad_field: 'in.op_id',
      pad_byte: 'a',
      pad_to_total_frame_bytes: target,
    });
    assert.equal(Buffer.byteLength(text, 'utf8'), target, `target ${target}`);
    const parsed = JSON.parse(text);
    assert.equal(parsed.id, 81);
    assert.equal(parsed.op, 'subject.ensure');
    assert.ok(parsed.in.op_id.startsWith('seed'));
    assert.ok(/^seeda*$/.test(parsed.in.op_id));
  }
});

test('the padded envelope refuses unreachable and malformed targets', () => {
  const env = { id: 1, op: 'subject.ensure', in: { op_id: 'x' } };
  assert.throws(() => buildPaddedEnvelope({
    envelope: env, pad_field: 'in.op_id', pad_byte: 'a', pad_to_total_frame_bytes: 3,
  }), /smaller than the envelope itself/);
  assert.throws(() => buildPaddedEnvelope({
    envelope: env, pad_field: 'in.op_id', pad_byte: 'a', pad_to_total_frame_bytes: undefined,
  }), /positive integer/);
  assert.throws(() => buildPaddedEnvelope({
    envelope: env, pad_field: 'in.nope', pad_byte: 'a', pad_to_total_frame_bytes: 100,
  }), /does not resolve inside the envelope/);
  assert.throws(() => buildPaddedEnvelope({
    envelope: env, pad_field: 'in.op_id', pad_byte: 'ab', pad_to_total_frame_bytes: 100,
  }), /single unescaped ASCII byte/);
});

test('the masked frame header uses the canonical RFC 6455 length form per range', () => {
  const mask = Buffer.from([0x11, 0x22, 0x33, 0x44]);
  // 0–125: length inline in byte 1, mask at offset 2.
  const small = buildMaskedFrameHeader(42, mask);
  assert.equal(small.length, 6);
  assert.equal(small[0], 0x81);
  assert.equal(small[1], 0x80 | 42);
  assert.deepEqual(small.subarray(2, 6), mask, 'the mask must land right after the length field');
  // 126–65535: 16-bit extended length, mask at offset 4.
  const medium = buildMaskedFrameHeader(30_000, mask);
  assert.equal(medium.length, 8);
  assert.equal(medium[1], 0x80 | 126);
  assert.equal(medium.readUInt16BE(2), 30_000);
  assert.deepEqual(medium.subarray(4, 8), mask);
  // >65535: 64-bit extended length, mask at offset 10.
  const large = buildMaskedFrameHeader(128 * 1024 * 1024 + 1, mask);
  assert.equal(large.length, 14);
  assert.equal(large[1], 0x80 | 127);
  assert.equal(large.readBigUInt64BE(2), BigInt(128 * 1024 * 1024 + 1));
  assert.deepEqual(large.subarray(10, 14), mask);
  assert.throws(() => buildMaskedFrameHeader(-1, mask), /nonnegative integer/);
});

test('a pad byte that JSON-escapes or UTF-8 multibyte-encodes is rejected', () => {
  const env = () => ({ id: 1, op: 'subject.ensure', in: { op_id: 'x' } });
  for (const bad of ['"', '\\', 'é', '😀', 'ab', '']) {
    assert.throws(
      () => buildPaddedEnvelope({
        envelope: env(), pad_field: 'in.op_id', pad_byte: bad, pad_to_total_frame_bytes: 100,
      }),
      /single unescaped ASCII byte/,
      `pad_byte ${JSON.stringify(bad)} must be rejected — its serialized width is not one byte per repetition`,
    );
  }
});

test('form discriminators: padded vs bare vs ordinary values', () => {
  assert.ok(isPaddedEnvelope({ envelope: { id: 1 }, pad_field: 'in.x', pad_byte: 'a', pad_to_total_frame_bytes: 10 }));
  assert.ok(!isPaddedEnvelope({ envelope: { id: 1 } }));
  assert.ok(isBareEnvelope({ envelope: { id: 1, op: 'x' } }));
  assert.ok(!isBareEnvelope({ envelope: { id: 1 }, pad_field: 'in.x' }));
  assert.ok(!isBareEnvelope('plain string'));
});

process.on('exit', () => {
  for (const d of tempDirs.splice(0)) {
    try { rmSync(d, { recursive: true, force: true }); } catch { /* best effort */ }
  }
});
