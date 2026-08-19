// The byte-stream executor for the v2 conformance harness (the framing
// section of docs/protocol-v2.md, #233).
//
// Encodes and decodes JBS2 records and drives the client half of the two
// legal stream shapes: `stream: {send_bytes: N}` uploads (file.share) and
// `stream: {receive_bytes: N}` downloads (file.read). A call carrying
// `stream` runs as one duplex operation — the Text request is sent with its
// reply promise kept pending, Binary records are routed by request id, and
// the terminal Text reply settles the call. `send_bytes` is deliberately
// independent of `in.declared_bytes`: short, long, and over-limit streams
// stay expressible, which is how declared_size_mismatch and stage_stream
// are reached.
//
// Counters are kept separately per call (generated, socket-sent, received,
// receiver-accepted); `observe: bytes_streamed` reports receiver-ACCEPTED
// payload bytes only — the daemon's cumulative CREDIT/ABORT acknowledgement
// for an upload, the harness sink's accepted count for a download. A
// terminal pre-OPEN reply records zero without an OPEN.

import { AssertFailure, TransportFailure } from './assert.mjs';

export const STREAM_HEADER_BYTES = 48;
export const MAGIC = Buffer.from('JBS2', 'ascii'); // 4a 42 53 32
export const KIND = { OPEN: 0x01, DATA: 0x02, CREDIT: 0x03, END: 0x04, ABORT: 0x05, ACK: 0x06 };
const KIND_NAME = { 1: 'OPEN', 2: 'DATA', 3: 'CREDIT', 4: 'END', 5: 'ABORT', 6: 'ACK' };
export const ABORT_REASON = {
  cancelled: 0x01,
  source_failed: 0x02,
  sink_failed: 0x03,
  protocol_error: 0x04,
  operation_error: 0x05,
};

/** The DATA payload bound: `min(65_536, max_frame_bytes - 48)`, subtraction
 * checked before use. A served limit that cannot carry a record is an invalid
 * configuration the daemon must refuse readiness over — never papered over. */
export function maxDataPayloadBytes(maxFrameBytes) {
  // The limits contract serves JSON integers — a numeric string or fraction
  // is a nonconforming configuration, never coerced.
  if (typeof maxFrameBytes !== 'number' || !Number.isInteger(maxFrameBytes) || maxFrameBytes <= STREAM_HEADER_BYTES) {
    throw new AssertFailure(
      `served max_frame_bytes ${JSON.stringify(maxFrameBytes)} cannot carry a byte-stream record`,
    );
  }
  return Math.min(65_536, maxFrameBytes - STREAM_HEADER_BYTES);
}

/** Encode one record. `streamId` is a 16-byte Buffer (zeros before OPEN). */
export function encodeRecord({ kind, id, streamId, offset = 0, value = 0, payload = null }) {
  const buf = Buffer.alloc(STREAM_HEADER_BYTES + (payload ? payload.length : 0));
  MAGIC.copy(buf, 0);
  buf.writeUInt8(kind, 4);
  // Bytes 5..8 are reserved and stay zero (alloc zero-fills).
  buf.writeBigUInt64BE(BigInt(id), 8);
  streamId.copy(buf, 16);
  buf.writeBigUInt64BE(BigInt(offset), 32);
  buf.writeBigUInt64BE(BigInt(value), 40);
  if (payload) payload.copy(buf, STREAM_HEADER_BYTES);
  return buf;
}

/** Decode one Binary message into a record, or `{malformed}` when it cannot
 * carry one. u64 fields are safe as Numbers here: offsets and totals are
 * bounded by the served file limit and ids by 2^53-1. */
export function decodeRecord(data) {
  const buf = Buffer.isBuffer(data) ? data : Buffer.from(data);
  if (buf.length < STREAM_HEADER_BYTES) {
    return { malformed: `binary message of ${buf.length} bytes is shorter than the ${STREAM_HEADER_BYTES}-byte header` };
  }
  if (!buf.subarray(0, 4).equals(MAGIC)) return { malformed: 'bad magic' };
  const kind = buf.readUInt8(4);
  return {
    kind,
    kindName: KIND_NAME[kind] || `0x${kind.toString(16)}`,
    reservedZero: buf.readUInt8(5) === 0 && buf.readUInt8(6) === 0 && buf.readUInt8(7) === 0,
    id: Number(buf.readBigUInt64BE(8)),
    streamId: Buffer.from(buf.subarray(16, 32)),
    offset: Number(buf.readBigUInt64BE(32)),
    value: Number(buf.readBigUInt64BE(40)),
    payload: buf.length > STREAM_HEADER_BYTES ? Buffer.from(buf.subarray(STREAM_HEADER_BYTES)) : null,
  };
}

const ZERO_STREAM_ID = Buffer.alloc(16);

/** The deterministic upload generator: byte at zero-based offset i is i mod 251. */
export function patternChunk(offset, len) {
  const b = Buffer.allocUnsafe(len);
  for (let j = 0; j < len; j++) b[j] = (offset + j) % 251;
  return b;
}

/**
 * Per-call stream state. One tracker exists for EVERY `call` step (streaming
 * or not), so `bytes_streamed` can observe zero for a pre-OPEN refusal and an
 * unexpected OPEN on a non-streaming call (e.g. an unfaithful op_id replay)
 * is recorded rather than silently dropped.
 */
export class CallStreamTracker {
  constructor(id) {
    this.id = id;
    this.streamId = null;
    this.opened = false;
    this.openTotal = null;
    this.generated = 0;
    this.socketSent = 0;
    this.received = 0;
    this.accepted = 0;
    this.queue = [];
    this.failure = null;
    this.wakers = [];
    // Delivery sequence, shared by records and the terminal reply, so their
    // wire order survives batched delivery.
    this.seq = 0;
  }

  deliver(record) {
    if (record.kind === KIND.OPEN) this.opened = true;
    record.seq = ++this.seq;
    this.queue.push(record);
    this.wake();
  }

  fail(err) {
    if (!this.failure) this.failure = err;
    this.wake();
  }

  wake() {
    for (const w of this.wakers.splice(0)) w();
  }

  /** Wait for the next record or the reply settling, whichever first. */
  async next(replyState, timeoutMs, what) {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      // Order matters: deliver records and the reply in WIRE order (both are
      // sequence-stamped), and surface a recorded violation before honouring
      // the settled reply — a connection-fatal violation must not be
      // laundered by a reply that arrived in the same batch. A record
      // delivered AFTER the reply stays queued for the post-terminal checks.
      const head = this.queue[0];
      if (head && (!replyState.done || head.seq < replyState.seq)) {
        return { record: this.queue.shift() };
      }
      if (this.failure) throw this.failure;
      if (replyState.done) return { reply: replyState };
      const remain = deadline - Date.now();
      if (remain <= 0) {
        const openNote = this.opened ? '' : ' (no OPEN was received)';
        throw new TransportFailure(`stream call (id ${this.id}) timed out waiting for ${what}${openNote}`);
      }
      await new Promise((resolve) => {
        const timer = setTimeout(resolve, remain);
        this.wakers.push(() => {
          clearTimeout(timer);
          resolve();
        });
      });
    }
  }
}

/** Validate a daemon record's binding against the installed pair. */
function checkBinding(tracker, rec) {
  if (!tracker.streamId || !rec.streamId.equals(tracker.streamId)) {
    throw new AssertFailure(
      `daemon sent ${rec.kindName} for request ${rec.id} with a stream id that does not match the OPEN-installed pair`,
    );
  }
  if (!rec.reservedZero) {
    throw new AssertFailure(`daemon sent ${rec.kindName} with nonzero reserved bytes`);
  }
}

function validateOpen(tracker, rec, session) {
  if (rec.kind !== KIND.OPEN) {
    throw new AssertFailure(`daemon sent ${rec.kindName} before OPEN on request ${rec.id}`);
  }
  if (!rec.reservedZero || rec.offset !== 0 || rec.streamId.equals(ZERO_STREAM_ID)) {
    throw new AssertFailure('daemon OPEN is malformed (nonzero offset/reserved or zero stream id)');
  }
  if (rec.payload) throw new AssertFailure('daemon OPEN carried a payload');
  // Stream ids are unique and never reused within one WebSocket connection.
  const sidHex = rec.streamId.toString('hex');
  session.seenStreamIds ||= new Set();
  if (session.seenStreamIds.has(sidHex)) {
    throw new AssertFailure(`daemon reused stream id ${sidHex} on this connection`);
  }
  session.seenStreamIds.add(sidHex);
  tracker.streamId = rec.streamId;
  tracker.openTotal = rec.value;
}

const ABORT_REASON_NAME = { 1: 'cancelled', 2: 'source_failed', 3: 'sink_failed', 4: 'protocol_error' };

/** A daemon ABORT's reason binds the terminal reply: 0x01-0x03 resolve to
 * stream_aborted with the matching reason, 0x04 to malformed_frame, and 0x05
 * (operation_error) to the authoritative typed operation error — never
 * stream_aborted. The reply's byte accounting is bound too: any
 * transferred_bytes must equal the proven receiver-accepted count and a
 * known total must equal the OPEN total. */
function checkAbortReplyCorrelation(abortRec, reply, { accepted, total } = {}) {
  if (!reply || reply.ok) return;
  const err = reply.err || {};
  const code = err.code;
  if (abortRec.value === ABORT_REASON.operation_error) {
    if (code === 'stream_aborted') {
      throw new AssertFailure('daemon ABORT operation_error must resolve to a typed operation error, got stream_aborted');
    }
  } else if (abortRec.value === ABORT_REASON.protocol_error) {
    if (code !== 'malformed_frame') {
      throw new AssertFailure(`daemon ABORT protocol_error must resolve to malformed_frame, got ${JSON.stringify(code)}`);
    }
  } else if (code !== 'stream_aborted' || err.reason !== ABORT_REASON_NAME[abortRec.value]) {
    throw new AssertFailure(
      `daemon ABORT ${ABORT_REASON_NAME[abortRec.value]} must resolve to stream_aborted with that reason, got ${JSON.stringify(reply.err)}`,
    );
  }
  if (accepted !== undefined && err.transferred_bytes !== undefined && err.transferred_bytes !== accepted) {
    throw new AssertFailure(
      `terminal transferred_bytes ${err.transferred_bytes} disagrees with the receiver-accepted ${accepted}`,
    );
  }
  if (
    total !== undefined &&
    err.total &&
    typeof err.total === 'object' &&
    err.total.state === 'known' &&
    err.total.bytes !== total
  ) {
    throw new AssertFailure(`terminal total ${err.total.bytes} disagrees with the OPEN total ${total}`);
  }
}

/** Error codes only determinable on an ADMITTED stream — a pre-OPEN reply
 * carrying one is fabricated: the daemon never observed the bytes that
 * outcome describes. */
const STREAM_ONLY_ERRORS = new Set([
  'declared_size_mismatch',
  'transfer_stalled',
  'transfer_deadline_exceeded',
  'stream_aborted',
  // A correlated malformed_frame for a byte-stream record is only reachable
  // AFTER OPEN installed the (request id, stream id) binding — the daemon has
  // to have admitted the stream and examined the injected record to produce
  // it. Treating it as pre-OPEN-legal would let a daemon fabricate the
  // raw_record outcome without ever admitting or parsing the record.
  'malformed_frame',
]);

function checkPreOpenError(reply, op) {
  if (!reply || reply.ok) return;
  const err = reply.err || {};
  if (STREAM_ONLY_ERRORS.has(err.code) || (err.code === 'file_too_large' && err.enforced_at === 'stage_stream')) {
    throw new AssertFailure(
      `${op} replied ${err.code}${err.enforced_at ? `@${err.enforced_at}` : ''} before OPEN — that outcome requires an admitted stream`,
    );
  }
}

async function settleReply(replyState) {
  if (replyState.error) {
    throw new TransportFailure(`streaming call failed to get a terminal reply: ${replyState.error.message}`);
  }
  return replyState.value;
}

/** Reasons a daemon may claim, by its role: an upload receiver has no source
 * (never 0x02); a download producer has no sink (never 0x03). */
const UPLOAD_DAEMON_ABORT_REASONS = new Set([0x01, 0x03, 0x04, 0x05]);
const DOWNLOAD_DAEMON_ABORT_REASONS = new Set([0x01, 0x02, 0x04, 0x05]);

/** A daemon ABORT must be a bare record with a closed, directionally possible
 * reason and a plausible accepted count (never beyond what this side sent,
 * for an upload). */
function validateAbort(rec, sentBound, allowedReasons) {
  if (rec.payload) throw new AssertFailure('daemon ABORT carried a payload');
  if (rec.value < 0x01 || rec.value > 0x05) {
    throw new AssertFailure(`daemon ABORT reason ${rec.value} is outside the closed 0x01..0x05 set`);
  }
  if (!allowedReasons.has(rec.value)) {
    throw new AssertFailure(
      `daemon ABORT reason ${ABORT_REASON_NAME[rec.value] || rec.value} is impossible for this stream direction`,
    );
  }
  if (sentBound !== null && rec.offset > sentBound) {
    throw new AssertFailure(`daemon ABORT accepted count ${rec.offset} exceeds the ${sentBound} bytes sent`);
  }
}

/**
 * Drive one streaming call to its terminal Text reply. `spec` is the resolved
 * fixture `stream` value: `{send_bytes: N}` or `{receive_bytes: N}`.
 * `declaredBytes` is the resolved `in.declared_bytes` (uploads; used only to
 * pick the legal-END discipline, never to bound the generator). Returns the
 * terminal reply envelope; the tracker carries the byte accounting.
 */
/** Harness patience for one streaming call: at least 60 s, extended toward
 * the served absolute budget (with headroom) when the legal budget for this
 * total is longer, and capped under the 90 s case watchdog — the watchdog is
 * the harness's deliberate anti-hang bound and reports ERROR, never a
 * conformance verdict. Deadline ENFORCEMENT belongs to deadline-designed
 * fixtures. */
export function streamWaitMs(spec, servedLimits = {}, timeoutScale = 1) {
  const allowance = servedLimits.transfer_connect_allowance_ms;
  const floor = servedLimits.transfer_floor_bits_per_second;
  // A zero (or non-integer) served floor is an invalid configuration the
  // daemon must refuse readiness over — deadline arithmetic divides by it.
  if (typeof floor !== 'number' || !Number.isInteger(floor) || floor <= 0) {
    throw new AssertFailure(
      `served transfer_floor_bits_per_second ${JSON.stringify(floor)} is an invalid configuration`,
    );
  }
  if (typeof allowance !== 'number' || !Number.isInteger(allowance) || allowance < 0) {
    throw new AssertFailure(
      `served transfer_connect_allowance_ms ${JSON.stringify(allowance)} is an invalid configuration`,
    );
  }
  const total = Number(spec.send_bytes ?? spec.receive_bytes);
  let waitMs = 60_000;
  if (Number.isFinite(total)) {
    const budgetMs = allowance + Math.ceil((total * 8 * 1000) / floor);
    waitMs = Math.max(waitMs, Math.ceil(budgetMs * 1.2));
  }
  return Math.min(waitMs, 80_000) * timeoutScale;
}

export async function runStreamingCall({ session, tracker, replyState, spec, declaredBytes, maxPayload, limitBytes, inflightBytes, concurrentLimit, stallMs, waitMs = 60_000 }) {
  if (spec.send_bytes !== undefined) {
    return runUpload({ session, tracker, replyState, sendBytes: Number(spec.send_bytes), declaredBytes, maxPayload, limitBytes, inflightBytes, concurrentLimit, stallMs, waitMs, fault: spec.fault });
  }
  if (spec.receive_bytes !== undefined) {
    // Client-originated download faults (credit-pause, receiver ABORT) are not
    // executed yet — the download producer has no executable corpus fixture at
    // all (it needs a peer-fetched file the single-subject harness cannot
    // stage), so a `fault` here would have nothing to drive. Reject it loudly
    // rather than silently ignore a fixture's intent.
    if (spec.fault !== undefined) {
      throw new Error(`stream fault ${JSON.stringify(spec.fault)} is not executable on file.read yet`);
    }
    return runDownload({ session, tracker, replyState, receiveBytes: Number(spec.receive_bytes), maxPayload, inflightBytes, concurrentLimit, stallMs, waitMs });
  }
  throw new Error(`stream spec has neither send_bytes nor receive_bytes: ${JSON.stringify(spec)}`);
}

/** The daemon reserves the OPEN total against its served aggregate inflight
 * budget before admission — an OPEN beyond the sole-active budget proves an
 * admission its own limits forbid. */
function checkInflightBudget(openTotal, inflightBytes, concurrentLimit) {
  // The limits contract serves JSON integers; a non-integer bound is a
  // nonconforming configuration, never treated as no bound.
  if (typeof inflightBytes !== 'number' || !Number.isInteger(inflightBytes) || inflightBytes < 0) {
    throw new AssertFailure(
      `served max_transfer_bytes_inflight ${JSON.stringify(inflightBytes)} is unusable for byte streaming`,
    );
  }
  if (typeof concurrentLimit !== 'number' || !Number.isInteger(concurrentLimit) || concurrentLimit < 0) {
    throw new AssertFailure(
      `served max_concurrent_transfers ${JSON.stringify(concurrentLimit)} is unusable for byte streaming`,
    );
  }
  if (concurrentLimit < 1) {
    // This sole active transfer already exceeds the advertised count budget;
    // admission should have been refused pre-OPEN.
    throw new AssertFailure(
      `daemon admitted a stream with served max_concurrent_transfers ${concurrentLimit}`,
    );
  }
  if (openTotal > inflightBytes) {
    throw new AssertFailure(
      `daemon admitted an OPEN total ${openTotal} beyond its served max_transfer_bytes_inflight ${inflightBytes}`,
    );
  }
}

/** Accepted-progress stall tracking: the daemon must abort a stream whose
 * accepted progress stops for transfer_stall_ms, so a SUCCESS after an
 * observed gap of more than twice that budget proves the timer never ran.
 * The 2x slack absorbs harness-side scheduling pauses. */
function makeStallGauge(stallMs) {
  // The served stall budget must be a positive JSON integer — a malformed
  // value is an invalid configuration, never a disabled check.
  if (typeof stallMs !== 'number' || !Number.isInteger(stallMs) || stallMs <= 0) {
    throw new AssertFailure(
      `served transfer_stall_ms ${JSON.stringify(stallMs)} is an invalid configuration`,
    );
  }
  let last = Date.now();
  let maxGap = 0;
  return {
    progress() {
      const now = Date.now();
      maxGap = Math.max(maxGap, now - last);
      last = now;
    },
    /** Measure the trailing interval at the action that ends ACTIVE, without
     * counting it as progress (FINALIZING time stays excluded). */
    mark() {
      maxGap = Math.max(maxGap, Date.now() - last);
    },
    checkOnSuccess() {
      if (maxGap > 2 * stallMs) {
        throw new AssertFailure(
          `stream succeeded after an accepted-progress gap of ${maxGap}ms, over twice the served transfer_stall_ms ${stallMs}`,
        );
      }
    },
  };
}

/** Byte-bound on the local outbound socket queue; sends pause above it so an
 * upload never enqueues unbounded data while waiting for the peer. */
const OUTBOUND_QUEUE_BOUND = 4 * 65_536;

/** Yield to the event loop after a DATA write (so interleaved CREDIT, ABORT,
 * and the reply are serviced between records) and apply socket backpressure. */
async function yieldAfterSend(session, tracker) {
  await new Promise((resolve) => setImmediate(resolve));
  while (session.ws && session.ws.bufferedAmount > OUTBOUND_QUEUE_BOUND && !tracker.failure) {
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

/** Upload: race a pre-OPEN terminal reply against OPEN, then honour CREDIT. */
async function runUpload({ session, tracker, replyState, sendBytes, declaredBytes, maxPayload, limitBytes, inflightBytes, concurrentLimit, stallMs, waitMs, fault }) {
  const first = await tracker.next(replyState, waitMs, 'OPEN or a pre-OPEN terminal reply');
  if (first.reply) {
    // Terminal before admission: zero bytes, no stream. A `stream`-carrying
    // step is a fresh execution (the corpus runs faithful replays as
    // non-streaming calls), so a pre-OPEN SUCCESS is a daemon violation —
    // a fresh execution has exactly one OPEN. Anything still queued was
    // sequenced AFTER the terminal reply and violates the settled refusal.
    const reply = await settleReply(replyState);
    if (tracker.queue.length) {
      throw new AssertFailure(`daemon sent ${tracker.queue[0].kindName} after its pre-OPEN terminal reply`);
    }
    if (reply && reply.ok) {
      throw new AssertFailure('file.share replied success before OPEN on a streamed call');
    }
    checkPreOpenError(reply, 'file.share');
    return reply;
  }
  validateOpen(tracker, first.record, session);
  if (Number.isFinite(Number(declaredBytes)) && tracker.openTotal !== Number(declaredBytes)) {
    throw new AssertFailure(
      `upload OPEN total ${tracker.openTotal} disagrees with declared_bytes ${declaredBytes}`,
    );
  }
  checkInflightBudget(tracker.openTotal, inflightBytes, concurrentLimit);

  // Client-originated fault injection (the producer misbehaving on purpose).
  // `client_abort` is a user cancel: after admission but before any DATA, the
  // client sends ABORT(cancelled); the daemon (receiver) is obliged to ACK and
  // then reply stream_aborted{cancelled} with a zero receiver-accepted count —
  // the "client ABORT wins" leg of the race table. This is the one client
  // fault the single-subject harness can execute end-to-end, because it needs
  // only an admitted upload, no peer-fetched file.
  if (fault === 'client_abort') {
    return await driveClientAbortUpload({ session, tracker, replyState, waitMs });
  }
  // `raw_record` is a client protocol violation: after admission the client
  // sends one structurally malformed Binary record on the live binding (a
  // nonzero reserved byte — a full 48-byte header the daemon can correlate,
  // so it is stream-local, not the header-unparseable 4007 class). The daemon
  // (receiver) aborts only that stream with ABORT(protocol_error) at accepted
  // offset zero, the client ACKs (0x05), and the request resolves to the
  // correlated `malformed_frame` terminal reply while the connection stays
  // usable — the "recoverable stream-local violation" leg of the framing
  // record (docs/protocol-v2.md § Malformed stream records; matched by the
  // daemon integration test websocket_data_too_large_within_frame_limit_
  // aborts_stream_not_connection in crates/jeliyad/src/serve.rs).
  if (fault === 'raw_record') {
    return await driveRawRecordUpload({ session, tracker, replyState, waitMs, limitBytes });
  }
  // `transport_drop` is a pre-END transport loss (docs/protocol-v2.md
  // § Disconnects and retries): the client streams a healthy PARTIAL upload
  // — one DATA record, acknowledged by the daemon's cumulative CREDIT — and
  // then drops the WebSocket with no close frame (the same terminate() the
  // control:{do:disconnect} verb drives; a stream step cannot use that verb
  // because a duplex call cannot return before its terminal reply, which
  // this fault deliberately never lets arrive). No byte stream survives its
  // connection: the daemon must abort the share, drop staging, release its
  // reservation, author nothing, and — under the request's op_id — record
  // stream_aborted{transport_lost} for replay. The step has NO wire reply,
  // so the driver returns a LOCAL OBSERVATION marker (never a synthetic
  // reply envelope) and the runner treats it as reply-less; the fixture's
  // reconnect + same-op_id replay asserts the recorded failure (matched by
  // the daemon integration test websocket_file_share_disconnect_replays_
  // pre_and_post_end_results in crates/jeliyad/src/serve.rs).
  if (fault === 'transport_drop') {
    return await driveTransportDropUpload({ session, tracker, replyState, sendBytes, waitMs, limitBytes, maxPayload });
  }
  if (fault !== undefined) {
    throw new Error(`unknown stream fault ${JSON.stringify(fault)} on file.share`);
  }

  const stallGauge = makeStallGauge(stallMs);

  // Producer state. `acceptedThrough`/`sendThrough` mirror the daemon's
  // cumulative CREDIT; `sent` is our contiguous DATA high-water mark. A
  // stream that matches the declaration sends END only after every sent byte
  // is CREDIT-acknowledged (the legal sequence). A deliberately SHORT stream
  // (send_bytes < declared_bytes) can never be fully acknowledged — the
  // daemon's refill policy has no partial-window acknowledgement — so END
  // rides the socket directly behind the last DATA record; per-direction
  // WebSocket ordering makes the daemon's observed_bytes deterministic.
  const declared = Number(declaredBytes);
  const shortStream = Number.isFinite(declared) && sendBytes < declared;
  // The served shared-file limit is load-bearing for the sentinel math; a
  // daemon that fails to serve it as a usable integer fails, never gets an
  // invented cap.
  if (typeof limitBytes !== 'number' || !Number.isInteger(limitBytes) || limitBytes < 0) {
    throw new AssertFailure(
      `served max_shared_file_bytes ${JSON.stringify(limitBytes)} is unusable for byte streaming`,
    );
  }
  const limit = limitBytes;
  // The daemon may not extend send_through past min(declared, limit) + 1 (the
  // observation sentinel).
  const sentinelCap = Math.min(Number.isFinite(declared) ? declared : Infinity, limit) + 1;
  let acceptedThrough = 0;
  let sendThrough = 0;
  let sent = 0;
  let creditSeen = false;
  let endSent = false;
  let abortSeen = null;
  // DATA acceptance is atomic, so a CREDIT acknowledgement may only land on
  // an emitted record boundary (or stay at zero).
  const dataEndpoints = new Set([0]);

  for (;;) {
    // One DATA record per pass, yielding after each so interleaved CREDIT,
    // ABORT, and the terminal reply are serviced between records and the
    // outbound queue stays byte-bounded — a queued record is processed
    // before the next send.
    if (!abortSeen && !replyState.done && sent < sendBytes && sendThrough > sent && tracker.queue.length === 0) {
      const len = Math.min(maxPayload, sendBytes - sent, sendThrough - sent);
      const payload = patternChunk(sent, len);
      tracker.generated += len;
      session.sendBinary(
        encodeRecord({ kind: KIND.DATA, id: tracker.id, streamId: tracker.streamId, offset: sent, payload }),
      );
      tracker.socketSent += len;
      sent += len;
      dataEndpoints.add(sent);
      await yieldAfterSend(session, tracker);
      continue;
    }
    // The exact-length path also requires the daemon's mandatory one-byte
    // probe: cumulative credit must have reached min(declared, limit) + 1
    // before END, or a daemon omitting the probe would pass unnoticed.
    const probeSatisfied = !Number.isFinite(declared) || sendThrough >= sentinelCap;
    if (
      !abortSeen &&
      !endSent &&
      !replyState.done &&
      sent === sendBytes &&
      (shortStream || (creditSeen && acceptedThrough >= sendBytes && probeSatisfied))
    ) {
      stallGauge.mark();
      session.sendBinary(
        encodeRecord({ kind: KIND.END, id: tracker.id, streamId: tracker.streamId, offset: sendBytes }),
      );
      endSent = true;
    }
    if (replyState.done && tracker.queue.length === 0) break;

    const what = endSent || abortSeen ? 'the terminal reply' : 'CREDIT, ABORT, or the terminal reply';
    const ev = await tracker.next(replyState, waitMs, what);
    if (ev.reply) break;
    const rec = ev.record;
    checkBinding(tracker, rec);
    // Per-direction ordering means nothing delivered after the daemon's ABORT
    // can be an older in-flight record — the binding is retired.
    if (abortSeen) {
      throw new AssertFailure(`daemon sent ${rec.kindName} after its ABORT retired the stream`);
    }
    switch (rec.kind) {
      case KIND.CREDIT: {
        if (rec.payload) throw new AssertFailure('daemon CREDIT carried a payload');
        if (rec.offset < acceptedThrough || rec.value < sendThrough || rec.offset > rec.value) {
          throw new AssertFailure(
            `daemon CREDIT regressed or inverted: (${rec.offset}, ${rec.value}) after (${acceptedThrough}, ${sendThrough})`,
          );
        }
        if (rec.offset > sent) {
          throw new AssertFailure(
            `daemon CREDIT acknowledged ${rec.offset} bytes beyond the ${sent} actually sent`,
          );
        }
        if (!dataEndpoints.has(rec.offset)) {
          throw new AssertFailure(
            `daemon CREDIT acknowledged ${rec.offset}, inside a DATA record rather than on a record boundary`,
          );
        }
        if (rec.offset > tracker.openTotal) {
          // The probe byte may be sent but never accepted: acknowledged
          // bytes can never exceed the admitted OPEN total.
          throw new AssertFailure(
            `daemon CREDIT acknowledged ${rec.offset} beyond the OPEN total ${tracker.openTotal}`,
          );
        }
        if (rec.value > limit + 1) {
          // The wire bound on upload credit is max_shared_file_bytes + 1;
          // min(declared, limit) + 1 is the mandatory probe TARGET, not a
          // ceiling — a conforming daemon may grant a larger bounded window.
          throw new AssertFailure(
            `daemon CREDIT send_through ${rec.value} exceeds max_shared_file_bytes + 1 (${limit + 1})`,
          );
        }
        creditSeen = true;
        if (rec.offset > acceptedThrough) stallGauge.progress();
        acceptedThrough = rec.offset;
        sendThrough = rec.value;
        tracker.accepted = acceptedThrough;
        break;
      }
      case KIND.ABORT: {
        // The daemon (the upload's receiver) aborted: its offset is its local
        // accepted count — which, with atomic DATA acceptance, must be an
        // emitted record endpoint at or above the CREDIT high-water it
        // already acknowledged. Stop producing and echo it in ACK (0x05).
        validateAbort(rec, sent, UPLOAD_DAEMON_ABORT_REASONS);
        if (!dataEndpoints.has(rec.offset)) {
          throw new AssertFailure(
            `daemon ABORT accepted count ${rec.offset} is inside a DATA record rather than on a record boundary`,
          );
        }
        if (rec.offset < acceptedThrough) {
          throw new AssertFailure(
            `daemon ABORT accepted count ${rec.offset} regressed below the credited ${acceptedThrough}`,
          );
        }
        if (rec.offset > tracker.openTotal) {
          // The probe byte may be sent but never accepted — same bound as
          // CREDIT acknowledgments.
          throw new AssertFailure(
            `daemon ABORT accepted count ${rec.offset} exceeds the OPEN total ${tracker.openTotal}`,
          );
        }
        if (tracker.replySeq !== undefined) {
          // The reply was already on the wire when we processed ABORT — the
          // daemon did not wait for our ACK (reply only after ACK).
          throw new AssertFailure('daemon sent its terminal reply before receiving the ABORT acknowledgement');
        }
        abortSeen = rec;
        tracker.accepted = rec.offset;
        session.sendBinary(
          encodeRecord({ kind: KIND.ACK, id: tracker.id, streamId: tracker.streamId, offset: rec.offset, value: 0x05 }),
        );
        break;
      }
      default:
        throw new AssertFailure(`daemon sent unexpected ${rec.kindName} on an upload stream`);
    }
  }
  if (tracker.queue.length) {
    throw new AssertFailure(`daemon sent ${tracker.queue[0].kindName} after the terminal reply`);
  }
  const reply = await settleReply(replyState);
  if (reply && reply.ok && abortSeen) {
    // ABORT is the daemon's terminal failure selection — even when our END
    // was already in flight, an ABORT means the failure path won the race.
    throw new AssertFailure('file.share replied success after the daemon ABORTed the stream');
  }
  if (reply && reply.ok && !endSent) {
    // Success is only sent after the daemon receives and finalizes END.
    throw new AssertFailure('file.share replied success before the client sent END');
  }
  if (reply && reply.ok) stallGauge.checkOnSuccess();
  if (reply && !reply.ok && !endSent && !abortSeen) {
    // While the stream is ACTIVE, a daemon terminal error must be preceded
    // by daemon ABORT and our ACK; only a post-END finalization error may
    // arrive bare.
    throw new AssertFailure(
      `file.share replied ${JSON.stringify(reply.err?.code)} on an active stream with no daemon ABORT`,
    );
  }
  if (abortSeen) {
    checkAbortReplyCorrelation(abortSeen, reply, { accepted: abortSeen.offset, total: tracker.openTotal });
  }
  return reply;
}

/**
 * Drive a client-originated upload cancel. After admission but before any DATA,
 * the client sends ABORT(cancelled) at offset 0 (its accepted high-water is
 * zero — nothing has been sent). The daemon (the receiver) must ACK (kind 0x06,
 * value 0x05, offset 0) and then reply stream_aborted{cancelled} accounting for
 * zero receiver-accepted bytes — the "client ABORT wins" leg of the race table.
 * A send window (CREDIT) the daemon granted before it observed our ABORT is
 * tolerated; it may not acknowledge bytes we never sent.
 */
async function driveClientAbortUpload({ session, tracker, replyState, waitMs }) {
  const accepted = 0;
  session.sendBinary(
    encodeRecord({ kind: KIND.ABORT, id: tracker.id, streamId: tracker.streamId, offset: accepted, value: ABORT_REASON.cancelled }),
  );
  let ackSeen = false;
  for (;;) {
    if (replyState.done && tracker.queue.length === 0) break;
    const ev = await tracker.next(replyState, waitMs, ackSeen ? 'the terminal reply' : 'the ABORT acknowledgement');
    if (ev.reply) break;
    const rec = ev.record;
    checkBinding(tracker, rec);
    if (rec.kind === KIND.CREDIT) {
      // The ACK retires the binding (protocol: "Only ACK or that timeout
      // retires the binding"; a record on a retired stream is malformed) — a
      // CREDIT emitted after the ACK certifies a daemon still speaking on a
      // dead stream.
      if (ackSeen) {
        throw new AssertFailure(
          'daemon sent CREDIT after acknowledging the ABORT — the binding retired at ACK',
        );
      }
      // A send window granted before the daemon saw our ABORT. It can never
      // acknowledge bytes we never sent.
      if (rec.offset > accepted) {
        throw new AssertFailure(
          `daemon CREDIT acknowledged ${rec.offset} bytes after a zero-progress client ABORT`,
        );
      }
      continue;
    }
    if (rec.kind === KIND.ACK) {
      if (ackSeen) throw new AssertFailure('daemon sent a second ACK for one client ABORT');
      if (rec.payload) throw new AssertFailure('daemon ACK carried a payload');
      if (rec.value !== 0x05) {
        throw new AssertFailure(`daemon ACK value ${rec.value} is not 0x05 (the ABORT it acknowledges)`);
      }
      if (rec.offset !== accepted) {
        throw new AssertFailure(
          `daemon ACK accepted count ${rec.offset} disagrees with the ${accepted} bytes it accepted`,
        );
      }
      ackSeen = true;
      tracker.accepted = accepted;
      continue;
    }
    throw new AssertFailure(`daemon sent unexpected ${rec.kindName} after a client ABORT`);
  }
  if (tracker.queue.length) {
    throw new AssertFailure(`daemon sent ${tracker.queue[0].kindName} after the terminal reply`);
  }
  if (!ackSeen) {
    throw new AssertFailure('daemon replied to a client ABORT without first acknowledging it');
  }
  const reply = await settleReply(replyState);
  if (reply && reply.ok) {
    throw new AssertFailure('file.share replied success after a client ABORT');
  }
  const err = (reply && reply.err) || {};
  if (err.code !== 'stream_aborted' || err.reason !== 'cancelled') {
    throw new AssertFailure(
      `client ABORT must resolve to stream_aborted{cancelled}, got ${JSON.stringify(reply.err)}`,
    );
  }
  if (err.transferred_bytes !== undefined && err.transferred_bytes !== accepted) {
    throw new AssertFailure(
      `terminal transferred_bytes ${err.transferred_bytes} disagrees with the ${accepted} receiver-accepted bytes`,
    );
  }
  return reply;
}

/**
 * Drive a client-originated raw-record protocol violation. After admission the
 * client sends exactly one structurally malformed DATA record on the active
 * binding: a valid 48-byte header and magic, correct id and stream id, but a
 * nonzero reserved byte (bytes 5..8 MUST be zero). Because the header is
 * correlatable to the live stream, the daemon rejects only that stream rather
 * than closing the connection: it sends ABORT(protocol_error) at accepted
 * offset zero (nothing was accepted), the client ACKs it (0x05), and the
 * request resolves to the correlated `malformed_frame` terminal reply. A
 * daemon that instead closes the connection, replies success, or answers
 * without first ABORTing is a violation. Receiver-accepted bytes are zero.
 */
async function driveRawRecordUpload({ session, tracker, replyState, waitMs, limitBytes }) {
  const accepted = 0;
  // The served shared-file bound gates CREDIT range validation; a daemon that
  // fails to serve it as a usable integer fails, never gets an invented cap.
  if (typeof limitBytes !== 'number' || !Number.isInteger(limitBytes) || limitBytes < 0) {
    throw new AssertFailure(
      `served max_shared_file_bytes ${JSON.stringify(limitBytes)} is unusable for byte streaming`,
    );
  }
  const limit = limitBytes;
  let sendThrough = 0;
  // One malformed DATA record: correct binding, corrupt reserved byte 5.
  const raw = encodeRecord({
    kind: KIND.DATA,
    id: tracker.id,
    streamId: tracker.streamId,
    offset: 0,
    payload: Buffer.from([0x00]),
  });
  raw.writeUInt8(0x01, 5); // reserved bytes MUST be zero — this makes it malformed
  session.sendBinary(raw);

  let ackSeen = false;
  for (;;) {
    if (replyState.done && tracker.queue.length === 0) break;
    const ev = await tracker.next(replyState, waitMs, ackSeen ? 'the terminal reply' : 'the daemon ABORT for the malformed record');
    if (ev.reply) break;
    const rec = ev.record;
    checkBinding(tracker, rec);
    if (rec.kind === KIND.CREDIT) {
      // A send window granted before the daemon parsed our malformed record is
      // tolerated, but its SHAPE and RANGE must satisfy the same rules the
      // normal upload path enforces: a payload, a regressing or inverted
      // window, or a send_through beyond the served bound is malformed
      // regardless of the injected fault — accepting one here would let a
      // non-conformant daemon pass this case.
      if (ackSeen) {
        throw new AssertFailure('daemon sent CREDIT after acknowledging the malformed-record ABORT — the binding retired at ACK');
      }
      if (rec.payload) throw new AssertFailure('daemon CREDIT carried a payload');
      if (rec.value < sendThrough || rec.offset > rec.value) {
        throw new AssertFailure(
          `daemon CREDIT regressed or inverted: (${rec.offset}, ${rec.value}) after (${accepted}, ${sendThrough})`,
        );
      }
      if (rec.value > limit + 1) {
        // The wire bound on upload credit is max_shared_file_bytes + 1.
        throw new AssertFailure(
          `daemon CREDIT send_through ${rec.value} exceeds max_shared_file_bytes + 1 (${limit + 1})`,
        );
      }
      // It can never acknowledge bytes we never sent.
      if (rec.offset > accepted) {
        throw new AssertFailure(`daemon CREDIT acknowledged ${rec.offset} bytes after a zero-progress malformed record`);
      }
      sendThrough = rec.value;
      continue;
    }
    if (rec.kind === KIND.ABORT) {
      // The client ACK retires the binding ("Only ACK or that timeout retires
      // the binding"; "Any later DATA, CREDIT, END, or terminal control for
      // the retired stream is malformed" — docs/protocol-v2.md §END, abort,
      // and cancellation). A SECOND daemon ABORT after our ACK is therefore
      // malformed traffic on a dead stream, never a terminal we re-acknowledge.
      if (ackSeen) {
        throw new AssertFailure(
          'daemon sent a second ABORT after the malformed-record ACK retired the binding',
        );
      }
      // The daemon aborts the stream it could correlate. Its reason is
      // protocol_error (0x04) and its accepted high-water is zero.
      validateAbort(rec, accepted, UPLOAD_DAEMON_ABORT_REASONS);
      if (rec.value !== ABORT_REASON.protocol_error) {
        throw new AssertFailure(
          `daemon ABORT reason ${rec.value} for a malformed record is not protocol_error (0x04)`,
        );
      }
      if (rec.offset !== accepted) {
        throw new AssertFailure(
          `daemon ABORT accepted count ${rec.offset} disagrees with the ${accepted} bytes it accepted from a malformed record`,
        );
      }
      if (tracker.replySeq !== undefined) {
        throw new AssertFailure('daemon sent its terminal reply before receiving the malformed-record ABORT acknowledgement');
      }
      tracker.accepted = accepted;
      session.sendBinary(
        encodeRecord({ kind: KIND.ACK, id: tracker.id, streamId: tracker.streamId, offset: accepted, value: 0x05 }),
      );
      ackSeen = true;
      continue;
    }
    throw new AssertFailure(`daemon sent unexpected ${rec.kindName} after a client malformed record`);
  }
  if (tracker.queue.length) {
    throw new AssertFailure(`daemon sent ${tracker.queue[0].kindName} after the terminal reply`);
  }
  if (!ackSeen) {
    throw new AssertFailure('daemon replied to a malformed record without first ABORTing the stream');
  }
  const reply = await settleReply(replyState);
  if (reply && reply.ok) {
    throw new AssertFailure('file.share replied success after a client malformed record');
  }
  const err = (reply && reply.err) || {};
  if (err.code !== 'malformed_frame') {
    throw new AssertFailure(
      `a stream-local malformed record must resolve to malformed_frame, got ${JSON.stringify(reply.err)}`,
    );
  }
  // A recoverable stream-local violation must leave the WebSocket usable —
  // "Other requests remain usable only on the request-local paths above"
  // (docs/protocol-v2.md §Malformed stream records). Prove it: send an
  // unrelated, idempotent typed request on the SAME session after the terminal
  // reply. A daemon that runs the correct ABORT→ACK→malformed_frame exchange
  // and THEN closes the connection (the 4005/4007 class this case exists to
  // catch) fails here instead of passing. subject.ensure carries no tracker,
  // so it never enters callRecords and cannot perturb a later bytes_streamed
  // observation, which reads only the runner's per-step records.
  const liveness = await session.call('subject.ensure', {}, { timeoutMs: Math.min(waitMs, 10_000) });
  if (!liveness || !liveness.ok) {
    throw new AssertFailure(
      `connection was not usable after a stream-local malformed_frame: ${JSON.stringify(liveness)}`,
    );
  }
  // Re-examine the retired stream after the liveness await: a daemon that sent
  // a late DATA/CREDIT/END/ABORT while subject.ensure was pending had the
  // record queued into this still-registered tracker without failing the
  // liveness call, and any traffic on a binding retired by the ACK is
  // malformed. (Anything arriving after this check, once the runner retires
  // the tracker, becomes the connection-fatal unbindable-record class and is
  // surfaced by the case-end sticky-violation scan.)
  if (tracker.queue.length) {
    throw new AssertFailure(
      `daemon sent ${tracker.queue[0].kindName} for the retired stream after the terminal reply`,
    );
  }
  if (tracker.failure) throw tracker.failure;
  return reply;
}

/** The marker a transport_drop step returns: this fault deliberately ends
 * with NO terminal reply on the wire (the socket is dropped pre-END), so the
 * driver reports a local observation — never a synthetic reply envelope. The
 * runner treats a step carrying this marker as reply-less: nothing to match,
 * capture, or expose under out/err/frame. */
export function isTransportDroppedMarker(value) {
  return value !== null && typeof value === 'object' && value.transportDropped === true;
}

/**
 * Drive a pre-END transport loss on a healthy partial upload. The daemon's
 * cumulative CREDIT is the only forward-progress signal, and its refill
 * policy acknowledges a DATA record only when the record consumed the whole
 * granted window (or completed the file) — so the healthy partial here is
 * exactly ONE FULL WINDOW: the declared total must exceed one window (the
 * served max DATA payload), the client sends a single DATA record of that
 * size, and the daemon's cumulative CREDIT must acknowledge exactly those
 * bytes. That acknowledgement pins what the daemon must later record. The
 * client then terminates the socket with no close frame. No byte stream
 * survives its connection: the daemon aborts the share, removes staging,
 * releases its reservation, authors no event, and (under the request's
 * op_id) records stream_aborted{transport_lost, transferred_bytes: the
 * acknowledged count} — asserted by the fixture's reconnect + same-op_id
 * replay, not here, because there is no reply on this connection to assert
 * against.
 *
 * Guards before the drop: the declared total exceeds one window (else the
 * fault would be a whole-file upload, not a partial — a fixture/sizing
 * error, never a daemon failure), the initial CREDIT shape and bound, the
 * acknowledgement landing exactly on the DATA record's end boundary, no
 * credit beyond what was sent, no daemon ABORT of the healthy stream, and
 * no terminal reply arriving before the drop (a reply here would mean the
 * daemon settled the request while the producer was still active).
 */
async function driveTransportDropUpload({ session, tracker, replyState, sendBytes, waitMs, limitBytes, maxPayload }) {
  if (typeof limitBytes !== 'number' || !Number.isInteger(limitBytes) || limitBytes < 0) {
    throw new AssertFailure(
      `served max_shared_file_bytes ${JSON.stringify(limitBytes)} is unusable for byte streaming`,
    );
  }
  const limit = limitBytes;
  // One full window is the only healthy partial the daemon's refill policy
  // acknowledges mid-file; a declared total within one window would make the
  // acknowledgement arrive only with the whole file, which is not a partial.
  // Both quantities gate the design: the OPEN total (already validated equal
  // to declared_bytes by the caller) must exceed one window so the record is
  // a genuine partial of the declaration, and send_bytes must cover the
  // window so the record stays within the bytes the fixture declares it
  // streams. send_bytes alone is not enough — it is deliberately independent
  // of declared_bytes in the DSL, so a fixture declaring less than it sends
  // would otherwise slip a whole-file or over-declaration record through and
  // misreport the daemon's refusal as a conformance failure.
  if (!Number.isInteger(sendBytes) || sendBytes < maxPayload || tracker.openTotal <= maxPayload) {
    throw new Error(
      `transport_drop needs a declared total (OPEN ${tracker.openTotal}) above one DATA window (${maxPayload} bytes) and a send_bytes (${sendBytes}) that covers the window, so a single full-window record is a healthy partial — a fixture/executor sizing mismatch, not a daemon conformance failure`,
    );
  }
  const chunkLen = maxPayload;
  const sent = chunkLen;
  let creditWindow = null;
  let acknowledged = null;
  let dataSent = false;
  let dropped = false;
  // The previous CREDIT's (offset, value), for the monotonicity contract.
  let prevCredit = null;

  const drop = () => {
    // The same primitive the control:{do:disconnect} verb drives — no close
    // frame, no drain. Outstanding waiters on this connection fail; the
    // marker below is returned before any such failure can be misread as
    // this step's reply.
    session.disconnect();
    dropped = true;
  };

  for (;;) {
    const what = dropped
      ? 'the local drop'
      : acknowledged === null
        ? (creditWindow === null ? 'the initial CREDIT' : `the cumulative CREDIT acknowledging ${sent}`)
        : 'the drop';
    if (dropped) break;
    if (creditWindow !== null && !dataSent && acknowledged === null && tracker.queue.length === 0 && !replyState.done) {
      // Send the one full-window DATA record as soon as the initial window
      // exists — exactly once (a second record at the same offset would be
      // client malformation, not this fault).
      const payload = patternChunk(0, chunkLen);
      tracker.generated += chunkLen;
      session.sendBinary(
        encodeRecord({ kind: KIND.DATA, id: tracker.id, streamId: tracker.streamId, offset: 0, payload }),
      );
      tracker.socketSent += chunkLen;
      dataSent = true;
      await yieldAfterSend(session, tracker);
      continue;
    }
    const ev = await tracker.next(replyState, waitMs, what);
    if (ev.reply) {
      throw new AssertFailure(
        `file.share settled its terminal reply while the producer was still active — the transport had not dropped yet`,
      );
    }
    const rec = ev.record;
    checkBinding(tracker, rec);
    if (rec.kind === KIND.CREDIT) {
      if (rec.payload) throw new AssertFailure('daemon CREDIT carried a payload');
      // The spec's credit contract is monotonic and non-inverted
      // (accepted_through <= send_through, both only ever advance) — the
      // same rule the normal upload path enforces on every window.
      if (rec.offset > rec.value) {
        throw new AssertFailure(
          `daemon CREDIT inverted: accepted ${rec.offset} through a send_through of ${rec.value}`,
        );
      }
      if (prevCredit && (rec.offset < prevCredit.offset || rec.value < prevCredit.value)) {
        throw new AssertFailure(
          `daemon CREDIT regressed: (${rec.offset}, ${rec.value}) after (${prevCredit.offset}, ${prevCredit.value})`,
        );
      }
      if (rec.offset > sent) {
        throw new AssertFailure(
          `daemon CREDIT acknowledged ${rec.offset} bytes beyond the ${sent} actually sent`,
        );
      }
      if (rec.value > limit + 1) {
        throw new AssertFailure(
          `daemon CREDIT send_through ${rec.value} exceeds max_shared_file_bytes + 1 (${limit + 1})`,
        );
      }
      if (creditWindow === null) {
        if (rec.offset !== 0) {
          throw new AssertFailure(
            `daemon CREDIT acknowledged ${rec.offset} bytes when the client had sent none`,
          );
        }
        if (rec.value < chunkLen) {
          // A window smaller than the design's one full record is not a
          // daemon violation — the record makes maxPayload a per-record
          // maximum only, and the receiver's byte-bounded capacity may be
          // any smaller window. But this fixture's one-full-window partial
          // (and its exact byte-count assertions) are sized to the canonical
          // window, so a divergent flow-control window is a fixture/limits
          // mismatch: an executor error, never a conformance failure blamed
          // on the daemon. Truly honoring arbitrary windows needs the
          // fixture's transferred_bytes to be dynamic — recorded as a
          // follow-up, not silently accepted here.
          throw new Error(
            `transport_drop fixture is mis-sized for this daemon's flow control: the initial CREDIT window is ${rec.value} bytes, not the one full window (${chunkLen}) this fixture's partial and its byte-count assertions are built on — a fixture/limits mismatch, not a daemon conformance failure`,
          );
        }
        if (rec.value > chunkLen) {
          // Larger windows break the design the other way: one record would
          // not consume the window, so the refill policy would never
          // acknowledge mid-file and the drop precondition could not be
          // established honestly. Same mismatch class.
          throw new Error(
            `transport_drop fixture is mis-sized for this daemon's flow control: the initial CREDIT window is ${rec.value} bytes, larger than the one full window (${chunkLen}) the single-record partial is built on, so no mid-file acknowledgement would ever arrive — a fixture/limits mismatch, not a daemon conformance failure`,
          );
        }
        creditWindow = rec.value;
        prevCredit = { offset: rec.offset, value: rec.value };
        continue;
      }
      // Cumulative credit may only advance on the accepted record boundary —
      // the daemon has seen exactly one DATA record, so the only legal
      // positive acknowledgement is exactly the record's end.
      if (rec.offset !== 0 && rec.offset !== sent) {
        throw new AssertFailure(
          `daemon CREDIT acknowledged ${rec.offset}, inside the single ${sent}-byte DATA record rather than on its boundary`,
        );
      }
      if (rec.offset === sent) {
        if (!dataSent) {
          // A cumulative acknowledgement of bytes that were never put on the
          // wire — e.g. a daemon batching initial + cumulative CREDIT before
          // the client could send its one record — is fabricated progress:
          // the daemon cannot have accepted bytes it never received.
          throw new AssertFailure(
            `daemon CREDIT acknowledged ${rec.offset} bytes before the client's DATA record was on the wire`,
          );
        }
        acknowledged = rec.offset;
        tracker.accepted = acknowledged;
        prevCredit = { offset: rec.offset, value: rec.value };
        // The transport has NOT dropped yet: if the daemon already settled a
        // terminal reply (batched with this CREDIT — Session stamps
        // replySeq synchronously on delivery), it fabricated an outcome
        // while the connection was still live. The drop must never bury that
        // violation; the same wire-order stamp the sibling drivers check is
        // authoritative here too.
        if (tracker.replySeq !== undefined) {
          throw new AssertFailure(
            'daemon settled its terminal reply before the transport dropped — the request was still live on an open connection',
          );
        }
        // The precondition is proven: the daemon has accepted exactly the
        // partial bytes. Drop the transport now, pre-END.
        drop();
        break;
      }
      prevCredit = { offset: rec.offset, value: rec.value };
      continue;
    }
    if (rec.kind === KIND.ABORT) {
      throw new AssertFailure(
        `daemon ABORTed a healthy partial upload (accepted ${rec.offset}) while the client awaited the cumulative CREDIT — a receiver must acknowledge the bytes it accepted`,
      );
    }
    throw new AssertFailure(`daemon sent unexpected ${rec.kindName} before the transport drop`);
  }
  if (!dropped) {
    throw new AssertFailure('transport_drop failed to drop the connection');
  }
  if (acknowledged !== sent) {
    throw new AssertFailure(
      `transport_drop dropped the connection before the daemon acknowledged the ${sent} accepted bytes`,
    );
  }
  // No terminal reply exists for this step; report the local observation.
  return { transportDropped: true, acceptedBytes: acknowledged };
}

/** Download: grant a bounded window, accept DATA, advance cumulative CREDIT
 * on sink acceptance, validate END, and ACK a daemon ABORT with the final
 * accepted count. Requires OPEN.total, END.offset, the terminal `out.bytes`,
 * and the fixture's N to agree. */
async function runDownload({ session, tracker, replyState, receiveBytes, maxPayload, inflightBytes, concurrentLimit, stallMs, waitMs }) {
  const first = await tracker.next(replyState, waitMs, 'OPEN or a pre-OPEN terminal reply');
  if (first.reply) {
    const reply = await settleReply(replyState);
    if (tracker.queue.length) {
      throw new AssertFailure(`daemon sent ${tracker.queue[0].kindName} after its pre-OPEN terminal reply`);
    }
    // file.read has no op_id dedup, so there is no legal streamless success.
    if (reply && reply.ok) {
      throw new AssertFailure('file.read replied success without OPEN or any streamed byte');
    }
    checkPreOpenError(reply, 'file.read');
    return reply;
  }
  validateOpen(tracker, first.record, session);
  const total = tracker.openTotal;
  if (total !== receiveBytes) {
    throw new AssertFailure(`file.read OPEN total ${total} disagrees with the fixture's receive_bytes ${receiveBytes}`);
  }
  checkInflightBudget(total, inflightBytes, concurrentLimit);
  const stallGauge = makeStallGauge(stallMs);

  // Bounded window; the sink is a counter (the harness never collects the
  // whole file). Client credit MUST NOT exceed the OPEN total. Every
  // accepted_through we issue is remembered: a producer ABORT offset must
  // equal one of them.
  const window = maxPayload;
  const issuedAccepted = new Set();
  const grant = (accepted) => {
    const send = Math.min(accepted + window, total);
    issuedAccepted.add(accepted);
    session.sendBinary(
      encodeRecord({ kind: KIND.CREDIT, id: tracker.id, streamId: tracker.streamId, offset: accepted, value: send }),
    );
    return send;
  };
  let sendThrough = grant(0);
  let endSeen = false;

  while (!endSeen) {
    // Drain queued records before honouring the settled reply: END and the
    // Text reply can land in one batched delivery, and honouring the reply
    // first would skip the spec-mandated END validation.
    if (replyState.done && tracker.queue.length === 0) break;
    const ev = await tracker.next(replyState, waitMs, 'DATA, END, ABORT, or the terminal reply');
    if (ev.reply) break;
    const rec = ev.record;
    checkBinding(tracker, rec);
    switch (rec.kind) {
      case KIND.DATA: {
        const len = rec.payload ? rec.payload.length : 0;
        tracker.received += len;
        if (len === 0) throw new AssertFailure('daemon sent an empty DATA record');
        if (len > maxPayload) throw new AssertFailure(`daemon DATA payload of ${len} bytes exceeds the ${maxPayload}-byte bound`);
        if (rec.offset !== tracker.accepted) {
          throw new AssertFailure(`daemon DATA offset ${rec.offset} is not contiguous with accepted ${tracker.accepted}`);
        }
        if (rec.offset + len > sendThrough) {
          throw new AssertFailure(`daemon DATA through ${rec.offset + len} exceeds granted credit ${sendThrough}`);
        }
        if (rec.value !== 0) throw new AssertFailure('daemon DATA carried a nonzero value field');
        tracker.accepted += len;
        stallGauge.progress();
        // An END already queued at grant time was sent before this CREDIT
        // could possibly arrive — the producer did not wait for every DATA
        // byte to be acknowledged.
        if (tracker.queue.some((r) => r.kind === KIND.END)) {
          throw new AssertFailure('daemon sent END before receiving the final cumulative CREDIT');
        }
        // An ABORT already on the wire ends crediting: the recipient stops
        // granting, and the new accepted offset is NOT issued — the producer
        // cannot have observed it, so an ABORT echoing it stays invalid.
        if (tracker.queue.some((r) => r.kind === KIND.ABORT)) {
          break;
        }
        sendThrough = grant(tracker.accepted);
        break;
      }
      case KIND.END: {
        if (rec.payload) throw new AssertFailure('daemon END carried a payload');
        if (rec.value !== 0) throw new AssertFailure('daemon END carried a nonzero value field');
        if (rec.offset !== total || tracker.accepted !== total) {
          throw new AssertFailure(
            `daemon END at ${rec.offset} disagrees with OPEN total ${total} / accepted ${tracker.accepted}`,
          );
        }
        stallGauge.mark();
        endSeen = true;
        break;
      }
      case KIND.ABORT: {
        // We are the receiver: ACK carries OUR final accepted count. The
        // producer's ABORT offset must echo an accepted_through we issued.
        validateAbort(rec, null, DOWNLOAD_DAEMON_ABORT_REASONS);
        if (!issuedAccepted.has(rec.offset)) {
          throw new AssertFailure(
            `daemon ABORT offset ${rec.offset} matches no accepted_through this receiver issued`,
          );
        }
        if (tracker.replySeq !== undefined) {
          throw new AssertFailure('daemon sent its terminal reply before receiving the ABORT acknowledgement');
        }
        session.sendBinary(
          encodeRecord({ kind: KIND.ACK, id: tracker.id, streamId: tracker.streamId, offset: tracker.accepted, value: 0x05 }),
        );
        const reply = await settleReplyAfter(tracker, replyState, waitMs);
        if (reply && reply.ok) {
          throw new AssertFailure('download stream was ABORTed but the request replied success');
        }
        checkAbortReplyCorrelation(rec, reply, { accepted: tracker.accepted, total });
        return reply;
      }
      default:
        throw new AssertFailure(`daemon sent unexpected ${rec.kindName} on a download stream`);
    }
  }

  const reply = endSeen ? await settleReplyAfter(tracker, replyState, waitMs) : await settleReply(replyState);
  if (reply && reply.ok) {
    // The spec's executor contract: OPEN.total, END, terminal out.bytes, and
    // the fixture's N must agree — a success with a missing or disagreeing
    // leg is a conformance failure, never a pass.
    if (!endSeen) throw new AssertFailure('file.read replied success without a daemon END');
    stallGauge.checkOnSuccess();
    if (!reply.out || reply.out.bytes !== total) {
      throw new AssertFailure(
        `terminal out.bytes ${reply.out?.bytes} disagrees with the streamed total ${total}`,
      );
    }
  } else if (reply && !reply.ok) {
    if (endSeen) {
      // A valid END commits the completed download; only success may follow.
      throw new AssertFailure('file.read completed its byte stream with END but replied an error');
    }
    // While the stream is ACTIVE, a daemon terminal error must be preceded
    // by daemon ABORT and our ACK.
    throw new AssertFailure(
      `file.read replied ${JSON.stringify(reply.err?.code)} on an active stream with no daemon ABORT`,
    );
  }
  return reply;
}

/** Wait for the reply to settle after the stream's binary side concluded.
 * `next` prefers queued records over the settled reply, so a record that
 * arrived batched with (or after) the reply is still examined — any record
 * after END/ACK is a daemon violation of the retired binding. */
async function settleReplyAfter(tracker, replyState, waitMs) {
  const ev = await tracker.next(replyState, waitMs, 'the terminal reply');
  if (ev.record) {
    throw new AssertFailure(`daemon sent ${ev.record.kindName} after the stream concluded`);
  }
  if (tracker.queue.length) {
    throw new AssertFailure(`daemon sent ${tracker.queue[0].kindName} after the terminal reply`);
  }
  return settleReply(replyState);
}
