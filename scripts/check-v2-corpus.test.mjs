import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, test } from 'node:test';

// The variable-binding contract for files.json: a "$name" reference must be
// captured by a save on an earlier step of the same case, or be one of the
// documented runner/precondition variables — the harness degrades an unbound
// reference to a literal string, which satisfies subset matching and turns
// the step into no evidence (the $fid2 failure mode that retired the
// aggregate no-path case).
//
// The validator resolves its corpus directory relative to its own location,
// and the ESM loader realpaths the entry module (a symlink would validate the
// real repository), so each scenario copies the script into a temp tree with
// a synthetic conformance/v2/files.json. The temp tree carries no manifest or
// workflow file; those produce unrelated problem lines, so assertions match
// only the binding-contract messages.

const SCRIPT = join(dirname(fileURLToPath(import.meta.url)), 'check-v2-corpus.mjs');
const NEVER_BOUND = 'is never bound';
const BEFORE_SAVE = 'is used before its save';

const tempRoots = [];

afterEach(() => {
  for (const root of tempRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function runValidator(cases) {
  const root = mkdtempSync(join(tmpdir(), 'jeliya-v2-corpus-check-'));
  tempRoots.push(root);
  mkdirSync(join(root, 'scripts'), { recursive: true });
  mkdirSync(join(root, 'conformance', 'v2'), { recursive: true });
  copyFileSync(SCRIPT, join(root, 'scripts', 'check-v2-corpus.mjs'));
  writeFileSync(
    join(root, 'conformance', 'v2', 'files.json'),
    JSON.stringify({ domain: 'files', note: 'binding-contract test fixture', cases }, null, 2),
  );
  const result = spawnSync(process.execPath, [join(root, 'scripts', 'check-v2-corpus.mjs')], {
    encoding: 'utf8',
  });
  assert.equal(result.error, undefined);
  return `${result.stdout}${result.stderr}`;
}

function fileCase(overrides) {
  return {
    name: 'binding_probe_case',
    kind: 'success',
    operation: 'file.list',
    intent: 'Proves the validator enforces the files.json variable-binding contract.',
    requires: ['subject', 'room:live'],
    steps: [],
    ...overrides,
  };
}

const LIST_IN = { cursor: { state: 'start' }, direction: 'forward', limit: 50 };

test('a prior save permits later use of the captured variable', () => {
  const output = runValidator([
    fileCase({
      steps: [
        { call: 'room.create', in: { name: 'Bind' }, save: { r: 'out.room_id' }, op_id: 'op-b-1' },
        { call: 'file.list', in: { room_id: '$r', ...LIST_IN } },
      ],
    }),
  ]);
  assert.ok(!output.includes(NEVER_BOUND), output);
  assert.ok(!output.includes(BEFORE_SAVE), output);
});

test('use before save fails with the one-indexed binding step named', () => {
  const output = runValidator([
    fileCase({
      steps: [
        { call: 'file.list', in: { room_id: '$r', ...LIST_IN } },
        { call: 'room.create', in: { name: 'Bind' }, save: { r: 'out.room_id' }, op_id: 'op-b-1' },
      ],
    }),
  ]);
  assert.ok(
    output.includes('binding_probe_case [step 1]: "$r" (in.room_id) is used before its save on step 2'),
    output,
  );
});

test('a never-bound variable such as $fid2 fails while documented $rid passes', () => {
  const output = runValidator([
    fileCase({
      operation: 'file.fetch',
      steps: [{ call: 'file.fetch', in: { room_id: '$rid', file_id: '$fid2' }, op_id: 'op-b-2' }],
    }),
  ]);
  assert.ok(output.includes('"$fid2" (in.file_id) is never bound'), output);
  assert.ok(!output.includes('"$rid"'), output);
});

test('documented precondition and runner-provided variables remain valid', () => {
  const output = runValidator([
    fileCase({
      operation: 'file.fetch',
      requires: ['subject', 'room:foreign', 'resource:fetched_file'],
      steps: [
        {
          call: 'file.fetch',
          in: { room_id: '$foreign_rid', file_id: '$foreign_fid' },
          op_id: '$op_id_new',
        },
        {
          call: 'file.read',
          in: { room_id: '$rid', file_id: '$fid' },
          stream: { receive_bytes: 4096 },
        },
      ],
    }),
  ]);
  assert.ok(!output.includes(NEVER_BOUND), output);
  assert.ok(!output.includes(BEFORE_SAVE), output);
});

test('computed-node operands and stream counts are checked', () => {
  const output = runValidator([
    fileCase({
      operation: 'file.share',
      steps: [
        {
          call: 'file.share',
          in: {
            room_id: '$rid',
            name: 'a.bin',
            declared_bytes: { $add: ['$lim', 1] },
            declared_content_type: 'application/octet-stream',
          },
          op_id: 'op-b-3',
          stream: { send_bytes: '$lim' },
        },
      ],
    }),
  ]);
  assert.ok(output.includes('"$lim" (in.declared_bytes.$add[0]) is never bound'), output);
  assert.ok(output.includes('"$lim" (stream.send_bytes) is never bound'), output);
});

test('a hello-captured limit satisfies computed-node and stream references', () => {
  const output = runValidator([
    fileCase({
      operation: 'file.share',
      steps: [
        { upgrade: { query: {}, headers: {} } },
        { await: { frame: { t: 'hello' } }, save: { lim: 'frame.limits.max_shared_file_bytes' } },
        {
          call: 'file.share',
          in: {
            room_id: '$rid',
            name: 'a.bin',
            declared_bytes: { $add: ['$lim', 1] },
            declared_content_type: 'application/octet-stream',
          },
          op_id: 'op-b-3',
          stream: { send_bytes: '$lim' },
        },
      ],
    }),
  ]);
  assert.ok(!output.includes(NEVER_BOUND), output);
  assert.ok(!output.includes(BEFORE_SAVE), output);
});

test('a documented precondition variable without its binding precondition fails', () => {
  const output = runValidator([
    fileCase({
      operation: 'file.fetch',
      requires: ['subject', 'room:live'],
      steps: [{ call: 'file.fetch', in: { room_id: '$rid', file_id: '$fid' }, op_id: 'op-b-6' }],
    }),
  ]);
  assert.ok(
    output.includes(
      '"$fid" (in.file_id) names a precondition variable, but this case declares no precondition that binds it',
    ),
    output,
  );
  assert.ok(!output.includes('"$rid"'), output);
});

test('an explicit save binds a documented name whose precondition is absent', () => {
  const output = runValidator([
    fileCase({
      operation: 'file.fetch',
      requires: ['subject', 'room:live', 'link:up'],
      steps: [
        {
          call: 'file.list',
          in: { room_id: '$rid', ...LIST_IN },
          save: { fid: 'out.files[0].file_id' },
        },
        { call: 'file.fetch', in: { room_id: '$rid', file_id: '$fid' }, op_id: 'op-b-7' },
      ],
    }),
  ]);
  assert.ok(!output.includes(NEVER_BOUND), output);
  assert.ok(!output.includes('names a precondition variable'), output);
});

test('the named ledger exempts the pre-existing U2 fixtures, and only them', () => {
  const steps = [
    { call: 'file.fetch', in: { room_id: '$rid', file_id: '$fid' }, op_id: 'op-b-8' },
    { call: 'file.fetch', in: { room_id: '$rid', file_id: '$fid_unsized' }, op_id: 'op-b-9' },
  ];
  const requires = ['subject', 'room:live', 'link:up'];
  const exempted = runValidator([
    fileCase({
      name: 'a_transfer_with_no_forward_progress_fails_with_transfer_stalled',
      operation: 'file.fetch',
      requires,
      steps,
    }),
  ]);
  assert.ok(!exempted.includes('names a precondition variable'), exempted);
  const unexempted = runValidator([
    fileCase({ name: 'some_other_case_with_the_same_shape', operation: 'file.fetch', requires, steps }),
  ]);
  assert.ok(unexempted.includes('"$fid" (in.file_id) names a precondition variable'), unexempted);
  assert.ok(unexempted.includes('"$fid_unsized" (in.file_id) names a precondition variable'), unexempted);
});

test('a malformed $-prefixed value the harness cannot resolve is invalid', () => {
  const output = runValidator([
    fileCase({
      operation: 'file.fetch',
      steps: [{ call: 'file.fetch', in: { room_id: '$rid', file_id: '$fid-2' }, op_id: 'op-b-10' }],
    }),
  ]);
  assert.ok(
    output.includes('"$fid-2" (in.file_id) is not a well-formed $variable reference'),
    output,
  );
});

test('a $-rooted path must be a bare root plus well-formed dot segments', () => {
  const output = runValidator([
    fileCase({
      steps: [
        {
          call: 'file.list',
          in: { room_id: '$rid', ...LIST_IN },
          save: { first: 'out' },
        },
        { assert: [{ path: '$rid[0].secret', op: 'absent' }] },
        { assert: [{ path: '$rid..secret', op: 'absent' }] },
        { assert: [{ path: '$rid.foo[0]', op: 'absent' }] },
        { assert: [{ path: '$first.files[*].file_id', op: 'present' }] },
      ],
    }),
  ]);
  assert.ok(
    output.includes('"$rid[0].secret" (assert[0].path) is not a well-formed $variable reference'),
    output,
  );
  assert.ok(
    output.includes('"$rid..secret" (assert[0].path) is not a well-formed $variable reference'),
    output,
  );
  assert.ok(
    output.includes('"$rid.foo[0]" (assert[0].path) is not a well-formed $variable reference'),
    output,
  );
  assert.ok(!output.includes('"$first.files[*].file_id"'), output);
});

test('http bodies and upgrade queries resolve whole, unlike headers and paths', () => {
  const output = runValidator([
    fileCase({
      kind: 'handshake',
      steps: [
        {
          http: {
            method: 'POST',
            path: '/api/session',
            headers: {},
            body: { room_id: '$rid[0]' },
          },
        },
      ],
    }),
  ]);
  assert.ok(
    output.includes('"$rid[0]" (http.body.room_id) is not a well-formed $variable reference'),
    output,
  );
});

test('a save on a send or control step captures nothing and is invalid', () => {
  const output = runValidator([
    fileCase({
      steps: [
        {
          control: { do: 'idle', ms: 0 },
          save: { ghost_capture: 'out.value' },
        },
        { call: 'file.list', in: { room_id: '$rid', ...LIST_IN, limit: '$ghost_capture' } },
      ],
    }),
  ]);
  assert.ok(output.includes('save on a control step captures nothing'), output);
  assert.ok(output.includes('"$ghost_capture"'), output);
});

test('the await reply builtin $id is not treated as a variable', () => {
  const output = runValidator([
    fileCase({
      operation: 'room.create',
      steps: [
        { call: 'room.create', in: { name: 'Reply' }, op_id: 'op-b-11' },
        { await: { reply: '$id' }, expect: { ok: true, out: { room_id: '<room_id>' } } },
      ],
    }),
  ]);
  assert.ok(!output.includes('"$id"'), output);
});

test('references embedded inside http and upgrade strings are checked', () => {
  const output = runValidator([
    fileCase({
      kind: 'handshake',
      steps: [
        {
          http: {
            method: 'GET',
            path: '/files/$ghost_http',
            headers: { authorization: 'Bearer $ghost_header' },
            body: null,
          },
        },
        { upgrade: { query: { sg: '$daemon_sg' }, headers: {} } },
      ],
    }),
  ]);
  assert.ok(output.includes('"$ghost_http" (http.path) is never bound'), output);
  assert.ok(output.includes('"$ghost_header" (http.headers.authorization) is never bound'), output);
  assert.ok(!output.includes('"$daemon_sg"'), output);
});

test('nested expectation, assertion-path, and observation references are checked', () => {
  const output = runValidator([
    fileCase({
      operation: 'transfer.cancel',
      steps: [
        {
          call: 'transfer.cancel',
          in: { transfer_op_id: 'op-b-4' },
          expect: {
            ok: false,
            err: { code: 'transfer_unknown', transfer_op_id: '$ghost' },
          },
        },
        { assert: [{ path: '$missing.field', op: 'present' }] },
        { assert: [{ observe: 'no_push', room_id: '$ghost_room', scope: 'case' }] },
        { assert: [{ observe: 'close_code', value: '$ghost_code', on: 'subject:self' }] },
      ],
    }),
  ]);
  assert.ok(output.includes('"$ghost" (expect.err.transfer_op_id) is never bound'), output);
  assert.ok(output.includes('"$missing" (assert[0].path) is never bound'), output);
  assert.ok(output.includes('"$ghost_room" (assert[0].room_id) is never bound'), output);
  assert.ok(output.includes('"$ghost_code" (assert[0].value) is never bound'), output);
});

test('captures do not leak between cases', () => {
  const output = runValidator([
    fileCase({
      name: 'binding_scope_case_a',
      operation: 'room.create',
      steps: [
        { call: 'room.create', in: { name: 'A' }, save: { shared_var: 'out.room_id' }, op_id: 'op-b-5' },
      ],
    }),
    fileCase({
      name: 'binding_scope_case_b',
      steps: [{ call: 'file.list', in: { room_id: '$shared_var', ...LIST_IN } }],
    }),
  ]);
  assert.ok(
    output.includes('binding_scope_case_b [step 1]: "$shared_var" (in.room_id) is never bound'),
    output,
  );
  assert.ok(!output.includes('binding_scope_case_a [step 1]: "$shared_var"'), output);
});
