// A WebSocket session to one daemon, speaking protocol v2.
//
// One Session is one connection for one principal. The harness keys sessions
// by the `on` label (`subject:self`, `subject:principal_b`, `subject:self#2`,
// `session:cX`, …); a `#2` suffix names a second connection for the same
// principal. Frames are matched by the DSL's `await` verb; replies are
// correlated by envelope `id`, and because replies may arrive out of order,
// each in-flight request has its own waiter.

import WebSocket from 'ws';

/** Serialize a value, splicing any `{__rawJson}` subtrees in verbatim. */
function serializeWithRaw(value) {
  const raws = [];
  const placeholder = (v) => {
    if (v !== null && typeof v === 'object' && v.__rawJson !== undefined) {
      const idx = raws.length;
      raws.push(v.__rawJson);
      return `RAW${idx}RAW`;
    }
    return v;
  };
  let text = JSON.stringify(value, (k, v) => placeholder(v));
  // Replace the quoted placeholders with the raw JSON text.
  text = text.replace(/"RAW(\d+)RAW"/g, (_, i) => raws[Number(i)]);
  return text;
}

let nextRequestId = 1;

/** Allocate a process-unique envelope id. */
export function requestId() {
  return nextRequestId++;
}

export class Session {
  constructor(label) {
    this.label = label;
    this.ws = null;
    this.pending = new Map(); // id -> {resolve, reject}
    this.frameWaiters = []; // {predicate, resolve, reject, timer}
    this.pushes = []; // every push frame received, in order
    this.frames = []; // every non-reply frame received (hello, pushes)
    this.closeCode = null;
    this.lastHello = null;
    this.open = false;
    // How many received pushes have been consumed by `await push` steps.
    this.pushCursor = 0;
  }

  /** Connect and wait for the hello frame. `query` is the v/sg/token map. */
  async connect(daemon, query, headers = {}) {
    const params = new URLSearchParams();
    for (const [k, v] of Object.entries(query)) params.set(k, String(v));
    const url = `${daemon.wsBase}?${params.toString()}`;
    const hdrs = { Host: `127.0.0.1:${daemon.port}`, ...headers };
    this.ws = new WebSocket(url, { headers: hdrs });
    this.ws.binaryType = 'nodebuffer';
    this.ws.on('message', (data) => this.#onMessage(data));
    this.ws.on('close', (code) => {
      this.open = false;
      this.closeCode = code;
      this.#failAll(new Error(`connection closed (${code})`));
    });
    this.ws.on('error', () => {});
    await new Promise((resolve, reject) => {
      this.ws.once('open', () => {
        this.open = true;
        resolve();
      });
      this.ws.once('error', (err) => reject(err));
    });
  }

  /** Public wrapper so the upgrade path can attach an externally-opened socket. */
  __onMessage(data) {
    this.#onMessage(data);
  }

  #onMessage(data) {
    let frame;
    try {
      frame = JSON.parse(data.toString());
    } catch {
      return; // undeliverable frame; nothing to correlate
    }
    if (frame.t === 'hello') {
      this.lastHello = frame;
      this.frames.push(frame);
      this.#notifyFrame(frame);
      return;
    }
    if (frame.t !== undefined) {
      // A push.
      this.pushes.push(frame);
      this.frames.push(frame);
      this.#notifyFrame(frame);
      return;
    }
    if (frame.id !== undefined) {
      // A reply.
      const waiter = this.pending.get(frame.id);
      if (waiter) {
        this.pending.delete(frame.id);
        clearTimeout(waiter.timer);
        waiter.resolve(frame);
      }
      this.#notifyFrame(frame);
      return;
    }
  }

  #notifyFrame(frame) {
    for (let i = this.frameWaiters.length - 1; i >= 0; i--) {
      const w = this.frameWaiters[i];
      let matched = false;
      try {
        matched = w.predicate(frame);
      } catch {
        matched = false;
      }
      if (matched) {
        clearTimeout(w.timer);
        this.frameWaiters.splice(i, 1);
        w.resolve(frame);
      }
    }
  }

  #failAll(err) {
    for (const w of this.frameWaiters) {
      clearTimeout(w.timer);
      w.reject(err);
    }
    this.frameWaiters = [];
    for (const [, p] of this.pending) {
      clearTimeout(p.timer);
      p.reject(err);
    }
    this.pending.clear();
  }

  /** Send one request envelope and wait for its correlated reply. */
  async call(op, input, { opId, timeoutMs = 10_000 } = {}) {
    const id = requestId();
    this.lastRequestId = id;
    const env = { id, op, in: input };
    if (opId !== undefined) env.op_id = opId;
    const replyPromise = new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`reply for ${op} (id ${id}) timed out`)),
        timeoutMs,
      );
      this.pending.set(id, { resolve, reject, timer });
    });
    this.ws.send(JSON.stringify(env));
    return replyPromise;
  }

  /** Send raw bytes/text that may not be a valid frame. A `{__rawJson}` marker
   * anywhere in the value is spliced in as pre-serialized JSON text (for
   * deep-nesting fixtures that cannot be JSON-serialized as objects). */
  sendRaw(value) {
    if (typeof value === 'string') {
      this.ws.send(value);
      return;
    }
    this.ws.send(serializeWithRaw(value));
  }

  /** Wait for a frame matching `predicate`. Replies (correlated by id) and the
   * hello may be re-scanned from history, but a PUSH is consumed once — two
   * sequential `await push` steps must see distinct pushes, or a fixture
   * needing two deliveries could be satisfied by one. */
  awaitFrame(predicate, timeoutMs = 10_000) {
    // Re-scan replies/hello in history (a frame that arrived before the await
    // counts), but start push scanning from the consumed cursor.
    for (const f of this.frames) {
      if (f.t !== undefined && f.t !== 'hello') continue; // pushes handled below
      let m = false;
      try {
        m = predicate(f);
      } catch {
        m = false;
      }
      if (m) return Promise.resolve(f);
    }
    for (let i = this.pushCursor; i < this.pushes.length; i++) {
      const f = this.pushes[i];
      let m = false;
      try {
        m = predicate(f);
      } catch {
        m = false;
      }
      if (m) {
        this.pushCursor = i + 1; // consume this push
        return Promise.resolve(f);
      }
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`await frame timed out on ${this.label}`)),
        timeoutMs,
      );
      this.frameWaiters.push({
        predicate: (f) => {
          // Newly arriving pushes are matched (and consumed on match); replies
          // and hello are matched without consuming.
          if (f.t !== undefined && f.t !== 'hello') {
            const isPush = predicate(f);
            if (isPush) this.pushCursor = this.pushes.length;
            return isPush;
          }
          return predicate(f);
        },
        resolve,
        reject,
        timer,
      });
    });
  }

  /** Drop the transport without a close frame. */
  disconnect() {
    if (this.ws) {
      try {
        this.ws.terminate();
      } catch {
        /* already gone */
      }
    }
  }

  close() {
    if (this.ws) {
      try {
        this.ws.close();
      } catch {
        /* already gone */
      }
    }
  }
}
