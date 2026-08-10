// Unit tests for the stream.mjs byte-stream codec and executor utilities.
// These cover the pure framing layer (encode/decode/pattern/limits/deadline)
// without requiring a live daemon; the live corpus replay harness tests the
// full upload/download round-trips end-to-end.
//
// Run: node --test conformance/v2/harness/stream.test.mjs

import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  STREAM_HEADER_BYTES,
  MAGIC,
  KIND,
  ABORT_REASON,
  maxDataPayloadBytes,
  encodeRecord,
  decodeRecord,
  patternChunk,
  CallStreamTracker,
  streamWaitMs,
} from './stream.mjs';

// ── maxDataPayloadBytes ─────────────────────────────────────────────────────

test('maxDataPayloadBytes: capped at 65536 when frame is large', () => {
  assert.equal(maxDataPayloadBytes(65_536 + STREAM_HEADER_BYTES), 65_536);
  assert.equal(maxDataPayloadBytes(65_536 + STREAM_HEADER_BYTES + 10_000), 65_536);
});

test('maxDataPayloadBytes: frame - header when smaller than 65536 cap', () => {
  assert.equal(maxDataPayloadBytes(STREAM_HEADER_BYTES + 100), 100);
  assert.equal(maxDataPayloadBytes(STREAM_HEADER_BYTES + 1), 1);
});

test('maxDataPayloadBytes: throws when frame equals header (no payload room)', () => {
  assert.throws(() => maxDataPayloadBytes(STREAM_HEADER_BYTES), /cannot carry a byte-stream record/);
});

test('maxDataPayloadBytes: throws when frame is smaller than header', () => {
  assert.throws(() => maxDataPayloadBytes(0), /cannot carry a byte-stream record/);
  assert.throws(() => maxDataPayloadBytes(-1), /cannot carry a byte-stream record/);
});

test('maxDataPayloadBytes: throws for non-integer inputs', () => {
  assert.throws(() => maxDataPayloadBytes(64.5), /cannot carry a byte-stream record/);
  assert.throws(() => maxDataPayloadBytes(NaN), /cannot carry a byte-stream record/);
  assert.throws(() => maxDataPayloadBytes(Infinity), /cannot carry a byte-stream record/);
  assert.throws(() => maxDataPayloadBytes('65584'), /cannot carry a byte-stream record/);
  assert.throws(() => maxDataPayloadBytes(null), /cannot carry a byte-stream record/);
});

// ── encodeRecord / decodeRecord round-trips ─────────────────────────────────

const STREAM_ID = Buffer.from('0102030405060708090a0b0c0d0e0f10', 'hex');

test('encode+decode: OPEN kind — no payload, magic present', () => {
  const encoded = encodeRecord({ kind: KIND.OPEN, id: 42, streamId: STREAM_ID, offset: 0, value: 4096 });
  assert.equal(encoded.length, STREAM_HEADER_BYTES);
  assert.ok(encoded.subarray(0, 4).equals(MAGIC));
  const rec = decodeRecord(encoded);
  assert.equal(rec.malformed, undefined);
  assert.equal(rec.kind, KIND.OPEN);
  assert.equal(rec.kindName, 'OPEN');
  assert.equal(rec.id, 42);
  assert.ok(rec.streamId.equals(STREAM_ID));
  assert.equal(rec.offset, 0);
  assert.equal(rec.value, 4096);
  assert.equal(rec.payload, null);
  assert.ok(rec.reservedZero);
});

test('encode+decode: DATA kind — payload preserved, offset reflects position', () => {
  const payload = Buffer.from([0xde, 0xad, 0xbe, 0xef]);
  const encoded = encodeRecord({ kind: KIND.DATA, id: 7, streamId: STREAM_ID, offset: 100, value: 0, payload });
  assert.equal(encoded.length, STREAM_HEADER_BYTES + payload.length);
  const rec = decodeRecord(encoded);
  assert.equal(rec.malformed, undefined);
  assert.equal(rec.kind, KIND.DATA);
  assert.equal(rec.kindName, 'DATA');
  assert.equal(rec.id, 7);
  assert.equal(rec.offset, 100);
  assert.equal(rec.value, 0);
  assert.ok(rec.payload.equals(payload));
});

test('encode+decode: CREDIT kind — accepted and send-through encoded', () => {
  const encoded = encodeRecord({ kind: KIND.CREDIT, id: 3, streamId: STREAM_ID, offset: 512, value: 1024 });
  const rec = decodeRecord(encoded);
  assert.equal(rec.malformed, undefined);
  assert.equal(rec.kind, KIND.CREDIT);
  assert.equal(rec.kindName, 'CREDIT');
  assert.equal(rec.offset, 512);
  assert.equal(rec.value, 1024);
  assert.equal(rec.payload, null);
});

test('encode+decode: END kind — offset is final byte count', () => {
  const encoded = encodeRecord({ kind: KIND.END, id: 1, streamId: STREAM_ID, offset: 8192, value: 0 });
  const rec = decodeRecord(encoded);
  assert.equal(rec.malformed, undefined);
  assert.equal(rec.kind, KIND.END);
  assert.equal(rec.kindName, 'END');
  assert.equal(rec.offset, 8192);
  assert.equal(rec.value, 0);
  assert.equal(rec.payload, null);
});

test('encode+decode: ABORT kind with cancelled reason', () => {
  const encoded = encodeRecord({
    kind: KIND.ABORT, id: 5, streamId: STREAM_ID, offset: 0, value: ABORT_REASON.cancelled,
  });
  const rec = decodeRecord(encoded);
  assert.equal(rec.malformed, undefined);
  assert.equal(rec.kind, KIND.ABORT);
  assert.equal(rec.kindName, 'ABORT');
  assert.equal(rec.value, ABORT_REASON.cancelled);
});

test('encode+decode: ACK kind carries the kind byte of the record it acknowledges', () => {
  const encoded = encodeRecord({ kind: KIND.ACK, id: 9, streamId: STREAM_ID, offset: 0, value: 0x05 });
  const rec = decodeRecord(encoded);
  assert.equal(rec.malformed, undefined);
  assert.equal(rec.kind, KIND.ACK);
  assert.equal(rec.kindName, 'ACK');
  assert.equal(rec.value, 0x05);
});

test('encode+decode: unknown kind byte decodes, kindName shows hex', () => {
  const encoded = encodeRecord({ kind: 0xff, id: 1, streamId: STREAM_ID });
  const rec = decodeRecord(encoded);
  assert.equal(rec.malformed, undefined);
  assert.equal(rec.kind, 0xff);
  assert.equal(rec.kindName, '0xff');
});

test('encode+decode: stream id is preserved byte-for-byte', () => {
  const sid = Buffer.from('ffffffffffffffffffffffffffffffff', 'hex');
  const rec = decodeRecord(encodeRecord({ kind: KIND.OPEN, id: 1, streamId: sid, value: 0 }));
  assert.ok(rec.streamId.equals(sid));
});

test('encode+decode: large id (near Number.MAX_SAFE_INTEGER)', () => {
  const id = Number.MAX_SAFE_INTEGER;
  const rec = decodeRecord(encodeRecord({ kind: KIND.OPEN, id, streamId: STREAM_ID, value: 0 }));
  assert.equal(rec.id, id);
});

test('encode+decode: defaults offset=0 and value=0 when omitted', () => {
  const rec = decodeRecord(encodeRecord({ kind: KIND.OPEN, id: 1, streamId: STREAM_ID }));
  assert.equal(rec.offset, 0);
  assert.equal(rec.value, 0);
});

// ── decodeRecord error cases ────────────────────────────────────────────────

test('decodeRecord: buffer shorter than 48-byte header is malformed', () => {
  const rec = decodeRecord(Buffer.alloc(47));
  assert.ok(rec.malformed);
  assert.ok(rec.malformed.includes('shorter than the 48-byte header'), rec.malformed);
});

test('decodeRecord: zero-length buffer is malformed', () => {
  const rec = decodeRecord(Buffer.alloc(0));
  assert.ok(rec.malformed);
});

test('decodeRecord: bad magic is malformed', () => {
  const buf = Buffer.alloc(STREAM_HEADER_BYTES);
  buf.write('XXXX', 0, 'ascii');
  const rec = decodeRecord(buf);
  assert.ok(rec.malformed);
  assert.ok(rec.malformed.includes('bad magic'), rec.malformed);
});

test('decodeRecord: accepts Uint8Array (non-Buffer binary data)', () => {
  const encoded = encodeRecord({ kind: KIND.OPEN, id: 1, streamId: STREAM_ID, value: 0 });
  const rec = decodeRecord(new Uint8Array(encoded));
  assert.equal(rec.malformed, undefined);
  assert.equal(rec.kind, KIND.OPEN);
});

test('decodeRecord: reservedZero is false when reserved bytes 5-7 are nonzero', () => {
  const buf = encodeRecord({ kind: KIND.DATA, id: 1, streamId: STREAM_ID });
  buf.writeUInt8(0xff, 5); // reserved byte
  const rec = decodeRecord(buf);
  assert.equal(rec.malformed, undefined);
  assert.equal(rec.reservedZero, false);
});

test('decodeRecord: exactly-48-byte message has null payload', () => {
  const encoded = encodeRecord({ kind: KIND.CREDIT, id: 1, streamId: STREAM_ID, offset: 0, value: 0 });
  assert.equal(encoded.length, 48);
  const rec = decodeRecord(encoded);
  assert.equal(rec.payload, null);
});

// ── patternChunk ────────────────────────────────────────────────────────────

test('patternChunk: byte at zero-based offset i is (i mod 251)', () => {
  const chunk = patternChunk(0, 10);
  for (let i = 0; i < 10; i++) assert.equal(chunk[i], i % 251, `byte ${i}`);
});

test('patternChunk: offset shifts the pattern — wraps at 251', () => {
  // offset 250: bytes are 250, 0, 1, 2 (wrapping at 251)
  const chunk = patternChunk(250, 4);
  assert.equal(chunk[0], 250 % 251); // 250
  assert.equal(chunk[1], 251 % 251); // 0
  assert.equal(chunk[2], 252 % 251); // 1
  assert.equal(chunk[3], 253 % 251); // 2
});

test('patternChunk: zero length returns empty buffer', () => {
  const chunk = patternChunk(0, 0);
  assert.equal(chunk.length, 0);
});

test('patternChunk: adjacent chunks concatenate to match a single chunk of the combined length', () => {
  const a = patternChunk(0, 100);
  const b = patternChunk(100, 100);
  const whole = patternChunk(0, 200);
  assert.ok(Buffer.concat([a, b]).equals(whole));
});

test('patternChunk: concatenation holds across the 251 boundary', () => {
  const a = patternChunk(240, 15);  // spans the 251 wrap
  const b = patternChunk(255, 15);
  const whole = patternChunk(240, 30);
  assert.ok(Buffer.concat([a, b]).equals(whole));
});

// ── streamWaitMs ────────────────────────────────────────────────────────────

const VALID_LIMITS = {
  transfer_connect_allowance_ms: 5_000,
  transfer_floor_bits_per_second: 1_000,
};

test('streamWaitMs: minimum is 60000 ms for a zero-byte stream', () => {
  assert.equal(streamWaitMs({ send_bytes: 0 }, VALID_LIMITS), 60_000);
  assert.equal(streamWaitMs({ receive_bytes: 0 }, VALID_LIMITS), 60_000);
});

test('streamWaitMs: large stream is capped at 80000 ms (before scaling)', () => {
  // 1 GiB at 1000 bps → budget far exceeds 60 s; capped at 80 000
  const ms = streamWaitMs({ send_bytes: 1_073_741_824 }, VALID_LIMITS);
  assert.equal(ms, 80_000);
});

test('streamWaitMs: receive_bytes is used for downloads just like send_bytes', () => {
  const upload = streamWaitMs({ send_bytes: 0 }, VALID_LIMITS);
  const download = streamWaitMs({ receive_bytes: 0 }, VALID_LIMITS);
  assert.equal(upload, download);
});

test('streamWaitMs: timeoutScale multiplies the pre-cap result', () => {
  // For send_bytes=0 the pre-scale result is 60000; *2 gives 120000
  const ms = streamWaitMs({ send_bytes: 0 }, VALID_LIMITS, 2);
  assert.equal(ms, 120_000);
});

test('streamWaitMs: throws for floor of zero (invalid config)', () => {
  assert.throws(
    () => streamWaitMs({ send_bytes: 100 }, { ...VALID_LIMITS, transfer_floor_bits_per_second: 0 }),
    /invalid configuration/,
  );
});

test('streamWaitMs: throws for negative floor', () => {
  assert.throws(
    () => streamWaitMs({ send_bytes: 100 }, { ...VALID_LIMITS, transfer_floor_bits_per_second: -1 }),
    /invalid configuration/,
  );
});

test('streamWaitMs: throws for fractional floor', () => {
  assert.throws(
    () => streamWaitMs({ send_bytes: 100 }, { ...VALID_LIMITS, transfer_floor_bits_per_second: 1.5 }),
    /invalid configuration/,
  );
});

test('streamWaitMs: throws for negative allowance', () => {
  assert.throws(
    () => streamWaitMs({ send_bytes: 100 }, { ...VALID_LIMITS, transfer_connect_allowance_ms: -1 }),
    /invalid configuration/,
  );
});

test('streamWaitMs: allowance of zero is valid', () => {
  const ms = streamWaitMs({ send_bytes: 0 }, { ...VALID_LIMITS, transfer_connect_allowance_ms: 0 });
  assert.equal(ms, 60_000);
});

// ── CallStreamTracker ───────────────────────────────────────────────────────

test('CallStreamTracker: initial state has zero byte counts and null id binding', () => {
  const t = new CallStreamTracker(99);
  assert.equal(t.id, 99);
  assert.equal(t.generated, 0);
  assert.equal(t.socketSent, 0);
  assert.equal(t.received, 0);
  assert.equal(t.accepted, 0);
  assert.equal(t.opened, false);
  assert.equal(t.streamId, null);
  assert.equal(t.failure, null);
  assert.deepEqual(t.queue, []);
});

test('CallStreamTracker.deliver: marks opened=true on KIND.OPEN', () => {
  const t = new CallStreamTracker(1);
  t.deliver({ kind: KIND.OPEN });
  assert.equal(t.opened, true);
  assert.equal(t.queue.length, 1);
});

test('CallStreamTracker.deliver: does not mark opened for other kinds', () => {
  const t = new CallStreamTracker(1);
  t.deliver({ kind: KIND.DATA });
  assert.equal(t.opened, false);
});

test('CallStreamTracker.deliver: assigns strictly increasing seq to each record', () => {
  const t = new CallStreamTracker(1);
  t.deliver({ kind: KIND.DATA });
  t.deliver({ kind: KIND.CREDIT });
  t.deliver({ kind: KIND.END });
  assert.equal(t.queue[0].seq, 1);
  assert.equal(t.queue[1].seq, 2);
  assert.equal(t.queue[2].seq, 3);
});

test('CallStreamTracker.fail: stores the first error only', () => {
  const t = new CallStreamTracker(1);
  const first = new Error('first');
  const second = new Error('second');
  t.fail(first);
  t.fail(second);
  assert.equal(t.failure, first);
});

test('CallStreamTracker.next: returns queued record when reply is not yet done', async () => {
  const t = new CallStreamTracker(1);
  t.deliver({ kind: KIND.DATA });
  const ev = await t.next({ done: false }, 1_000, 'test');
  assert.ok(ev.record, 'expected a record event');
  assert.equal(ev.record.kind, KIND.DATA);
  assert.equal(t.queue.length, 0);
});

test('CallStreamTracker.next: returns reply object when done and queue is empty', async () => {
  const t = new CallStreamTracker(1);
  const replyState = { done: true, seq: 99 };
  const ev = await t.next(replyState, 1_000, 'test');
  assert.ok(ev.reply, 'expected a reply event');
  assert.equal(ev.reply, replyState);
});

test('CallStreamTracker.next: a record whose seq < reply seq is returned before the reply', async () => {
  const t = new CallStreamTracker(1);
  t.deliver({ kind: KIND.END }); // gets seq=1
  const replyState = { done: true, seq: 2 }; // reply seq is later
  const ev = await t.next(replyState, 1_000, 'test');
  assert.ok(ev.record, 'expected record to be returned before the reply');
  assert.equal(ev.record.kind, KIND.END);
});

test('CallStreamTracker.next: a record whose seq > reply seq is NOT returned before the reply', async () => {
  const t = new CallStreamTracker(1);
  const replyState = { done: true, seq: 1 };
  t.deliver({ kind: KIND.DATA }); // gets seq=1; seq === reply.seq means reply wins
  // actually seq starts at 0 and gets incremented, so first deliver gives seq=1
  // reply.seq = 1 means head.seq (1) < reply.seq (1) is false, so reply wins
  const ev = await t.next(replyState, 1_000, 'test');
  // The record has seq=1 and reply has seq=1: condition is head.seq < replyState.seq → false
  // so the reply is returned
  assert.ok(ev.reply, 'expected reply when record seq equals reply seq');
});

test('CallStreamTracker.next: rejects immediately when failure is set', async () => {
  const t = new CallStreamTracker(1);
  t.fail(new Error('injected failure'));
  await assert.rejects(() => t.next({ done: false }, 1_000, 'test'), /injected failure/);
});

test('CallStreamTracker.next: wakes up when record is delivered asynchronously', async () => {
  const t = new CallStreamTracker(1);
  const replyState = { done: false };
  const p = t.next(replyState, 5_000, 'async deliver test');
  setImmediate(() => t.deliver({ kind: KIND.CREDIT }));
  const ev = await p;
  assert.ok(ev.record);
  assert.equal(ev.record.kind, KIND.CREDIT);
});

test('CallStreamTracker.next: wakes up when failure is set asynchronously', async () => {
  const t = new CallStreamTracker(1);
  const replyState = { done: false };
  const p = t.next(replyState, 5_000, 'async fail test');
  setImmediate(() => t.fail(new Error('async failure')));
  await assert.rejects(() => p, /async failure/);
});
