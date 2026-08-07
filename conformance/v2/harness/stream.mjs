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
    return runUpload({ session, tracker, replyState, sendBytes: Number(spec.send_bytes), declaredBytes, maxPayload, limitBytes, inflightBytes, concurrentLimit, stallMs, waitMs });
  }
  if (spec.receive_bytes !== undefined) {
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
async function runUpload({ session, tracker, replyState, sendBytes, declaredBytes, maxPayload, limitBytes, inflightBytes, concurrentLimit, stallMs, waitMs }) {
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
