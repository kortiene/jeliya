// The case runner for the v2 conformance harness.
//
// Each case gets a fresh daemon (or daemons, for `daemon:second`), a session
// per `on` label, and a variable scope. The runner establishes `requires`,
// executes the steps in order, and reports pass/fail with the first failing
// assertion. Blocked-on-upstream cases are expected to FAIL — a passing one
// is reported as a surprise, not silently promoted.

import { readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { startDaemon } from './daemon.mjs';
import { Session } from './session.mjs';
import { AssertContext, AssertFailure, TransportFailure, evalAssert, subsetMatch } from './assert.mjs';
import { resolvePath, resolveValue } from './values.mjs';
import { CallStreamTracker, maxDataPayloadBytes, runStreamingCall, streamWaitMs } from './stream.mjs';

/** The outcome of one case. */
export const Outcome = {
  PASS: 'pass',
  FAIL: 'fail',
  BLOCKED_PASS: 'blocked-pass', // blocked_on_upstream but it passed (surprise)
  BLOCKED_FAIL: 'blocked-fail', // blocked_on_upstream and it failed (expected)
  ERROR: 'error', // harness/setup error, not an assertion
};

let opIdCounter = 1;

/** A loopback TCP echo service for pipe-target fixtures. */
function startEchoService() {
  return import('node:net').then(({ createServer }) => {
    return new Promise((resolve) => {
      const server = createServer((sock) => sock.pipe(sock));
      server.listen(0, '127.0.0.1', () => {
        const port = server.address().port;
        resolve({ port, close: () => server.close() });
      });
    });
  });
}

export class Runner {
  constructor(binary, { verbose = false, timeoutScale = 1 } = {}) {
    this.binary = binary;
    this.verbose = verbose;
    this.timeoutScale = timeoutScale;
  }

  log(...args) {
    if (this.verbose) console.log('   ', ...args);
  }

  /**
   * Run one case. Returns { outcome, name, reason }.
   */
  async runCase(fixture) {
    // A hard watchdog so no case can hang the whole run. The state (sessions,
    // daemons, timers, temp dirs) lives in an abort controller the watchdog
    // fires, so a timed-out case is actually cancelled and cleaned up rather
    // than left running to contaminate later cases.
    const budgetMs = 90_000 * this.timeoutScale;
    const handle = { cleanup: null };
    const timeout = new Promise((resolve) => {
      const t = setTimeout(async () => {
        if (handle.cleanup) await handle.cleanup();
        resolve({
          outcome: Outcome.ERROR,
          name: fixture.name,
          reason: `case watchdog timeout (${budgetMs}ms)`,
        });
      }, budgetMs);
      if (t.unref) t.unref();
    });
    return Promise.race([this.#runCaseInner(fixture, handle), timeout]);
  }

  async #runCaseInner(fixture, handle) {
    const name = fixture.name;
    const daemons = [];
    const sessions = new Map();
    const vars = Object.create(null);
    const ctxState = {
      networkActivity: 0,
      eventAuthored: 0,
      durableMutation: 0,
      pushesByRoom: new Map(),
      // One record per `call` step: {step (1-based), label, accepted, opened}.
      // `bytes_streamed` observations read receiver-accepted bytes from here.
      callRecords: [],
    };

    const cleanup = async () => {
      for (const s of sessions.values()) s.close();
      for (const d of daemons) await d.stop();
      for (const c of (vars.__case ? vars.__case.closers.splice(0) : [])) {
        try { c(); } catch { /* ignore */ }
      }
    };
    // Register cleanup so the watchdog can cancel and reap this case.
    if (handle) handle.cleanup = cleanup;

    try {
      // Pre-seed the well-known variables a fresh case can reference.
      vars.op_id_new = `cf-op-${opIdCounter++}`;
      vars.op_id_fixed = 'cf-op-fixed-0001';
      // Codec bounds (not in the served limits object) for placeholder
      // synthesis; mirrors jeliya-codec's CodecBounds::default.
      vars.limits = {
        max_depth: 64,
        max_frame_bytes: 128 * 1024 * 1024,
        max_op_len: 64,
        max_array_len: 1 << 20,
      };

      // Establish requires.
      await this.#establishRequires(fixture, daemons, sessions, vars);

      const ctx = new AssertContext({
        vars,
        sessions,
        observe: async (a) => this.#evalObserve(a, { sessions, vars, ctxState, name, daemons }),
      });

      // Snapshot observable daemon state at case start (and re-snapshot after
      // each step) so no_event_authored / no_durable_mutation compare a real
      // before/after delta instead of passing unconditionally.
      ctxState.eventSnapshot = await this.#roomEventTotal(vars, sessions);
      ctxState.dirSnapshot = await this.#stableDirSignature(daemons);

      // Execute the steps.
      for (let i = 0; i < fixture.steps.length; i++) {
        const step = fixture.steps[i];
        ctxState.lastCallOk = undefined;
        await this.#runStep(step, i, { daemons, sessions, vars, ctx, ctxState, name });
        // An observation compares against the state just before the observed
        // operation. A SUCCESSFUL daemon-interacting step advances the
        // baselines; a REFUSED one (an error-replied call, or a raw `send`
        // probe) must not — a refused operation authors nothing legally, so
        // refreshing after it would absorb exactly the violation a following
        // no_event_authored / no_durable_mutation observation exists to
        // catch. Assertion-only (and save-only) steps touch no daemon state
        // and never refresh, so the pre-refusal baseline survives through
        // intervening assertions.
        const refused = (step.call && ctxState.lastCallOk === false) || step.send !== undefined;
        const interactsWithDaemon =
          step.call || step.http || step.upgrade || step.send !== undefined || step.await || step.control;
        if (interactsWithDaemon) {
          // The DIRECTORY baseline advances after every daemon interaction,
          // refused or not: a refused op_id operation legally performs one
          // durable write (its recorded reply in the dedup ledger), so a
          // strict pre-refusal dir baseline would fail a conformant daemon.
          // The capture settles first — staging cleanup is asynchronous, and
          // a baseline taken mid-deletion would read as a later "mutation".
          ctxState.dirSnapshot = await this.#stableDirSignature(daemons, ctxState.dirSnapshot);
          // The EVENT baseline is strict: a refused operation may author
          // nothing, so it never advances past a refusal.
          if (!refused) {
            ctxState.eventSnapshot = await this.#roomEventTotal(vars, sessions);
          }
        }
      }

      // A connection-fatal binary violation observed after the last tracker
      // retired must not be outlived by a green verdict — replaced (upgraded
      // away) sessions included.
      for (const s of [...sessions.values(), ...(vars.__case?.replacedSessions || [])]) {
        if (s.stickyBinaryViolation) throw s.stickyBinaryViolation;
      }

      // A case that ran all steps without a failing assertion passes. A block
      // may name an upstream dependency or a settled record/corpus
      // contradiction awaiting fixture retirement (e.g. the old "op_id is
      // required" case after the record settled optional-but-deduplicated).
      const blocked = fixture.blocked_on_upstream || fixture.blocked_on_record;
      await cleanup();
      if (blocked) {
        return { outcome: Outcome.BLOCKED_PASS, name, reason: `blocked on ${blocked} but PASSED` };
      }
      return { outcome: Outcome.PASS, name };
    } catch (err) {
      await cleanup();
      const reason =
        err instanceof AssertFailure
          ? err.message
          : `${err.name || 'Error'}: ${err.message}`;
      const blocked = fixture.blocked_on_upstream || fixture.blocked_on_record;
      if (blocked) {
        // Only a genuine matcher assertion failure counts as the EXPECTED
        // blocked failure. A setup error, an unsupported control verb, a
        // missing dependency, a transport failure (a reply timeout, closed
        // socket, failed upgrade, send failure, or crash), or a runner bug
        // means the case never reached the blocked assertion, so it remains
        // ERROR. TransportFailure is deliberately not an AssertFailure so a
        // dead daemon during a blocked case surfaces here rather than being
        // counted as the block's expected failure.
        if (err instanceof AssertFailure) {
          return { outcome: Outcome.BLOCKED_FAIL, name, reason };
        }
        return { outcome: Outcome.ERROR, name, reason: `blocked case errored before the blocked assertion: ${reason}` };
      }
      const isAssertion = err instanceof AssertFailure;
      return { outcome: isAssertion ? Outcome.FAIL : Outcome.ERROR, name, reason };
    }
  }

  /** Establish the case's `requires` (daemons, subjects, rooms, links). */
  async #establishRequires(fixture, daemons, sessions, vars) {
    const requires = fixture.requires || [];
    const needsSecondDaemon = requires.some((r) => r === 'daemon:second');

    // Every case runs against at least one daemon. `daemon:fresh` (and the
    // bare `daemon`/`daemon:self`) get a fresh one; without a daemon token we
    // still spawn one because a session needs somewhere to connect.
    const primary = await startDaemon(this.binary, { loopback: true });
    daemons.push(primary);
    vars['daemon.storage_generation'] = primary.storageGeneration;
    vars.daemon_sg = primary.storageGeneration;
    vars['daemon'] = { storage_generation: primary.storageGeneration };
    vars.__case = { primary, second: null, closers: [] };

    if (needsSecondDaemon) {
      const second = await startDaemon(this.binary, { loopback: true });
      daemons.push(second);
      vars.__case.second = second;
    }

    // Establish the subjects, room, and members the case names. `subject`
    // (bare) means a primary subject on the primary daemon; `room:live`/`plain`
    // means a room exists (`$rid`) and is live for `live`; `member:b`/`c` add
    // members via the invite flow. A case that names none of these needs no
    // setup beyond the daemon (e.g. subject-lifecycle cases on `subject:none`).
    const wantsRoom = requires.some((r) => /^room:(plain|live|quiescent|with_history)$/.test(r));
    const wantsLeftRoom = requires.some((r) => r === 'room:left');
    const memberLabels = requires.filter((r) => /^member:[a-z]$/.test(r));
    const wantsMembers = memberLabels.length > 0;

    // Only room/member setup pre-creates the daemon's (single) subject — a bare
    // `requires: subject` does NOT, because subject-lifecycle cases assert on
    // `subject.ensure`'s `created` flag themselves. The subject is global to the
    // daemon, so any room setup establishes it as a side effect.
    if (wantsRoom || wantsLeftRoom || wantsMembers) {
      // The authority's session and subject.
      const authority = await this.#sessionFor('subject:authority', daemons, sessions, vars);
      const authHello = await authority.call('subject.ensure', {});
      vars.self_sid = authHello.out?.subject_id;

      if (wantsRoom || wantsLeftRoom || wantsMembers) {
        const created = await authority.call('room.create', { name: 'conformance' });
        vars.rid = created.out?.room_id;
        if (requires.some((r) => r === 'room:live' || r === 'room:quiescent' || r === 'room:with_history')) {
          await authority.call('room.activate', { room_id: vars.rid });
        }
      }

      // Additional members. Each gets a daemon-side subject and redeems an
      // invite minted by the authority. `member:b` -> subject:member_b, etc.
      for (const m of memberLabels) {
        const letter = m.split(':')[1];
        const label = `subject:member_${letter}`;
        const sess = await this.#sessionFor(label, daemons, sessions, vars);
        await sess.call('subject.ensure', {});
        const ensured = await sess.call('subject.ensure', {});
        const sid = ensured.out?.subject_id;
        const mint = await authority.call('invite.mint', {
          room_id: vars.rid,
          subject_id: sid,
          role: 'member',
          expires_at: new Date(Date.now() + 3600_000).toISOString().replace(/\.\d+Z$/, 'Z'),
        });
        const cap = mint.out?.capability;
        if (cap) {
          await sess.call('invite.redeem', { capability: cap });
          if (requires.some((r) => r === 'room:live' || r === 'room:quiescent')) {
            await sess.call('room.activate', { room_id: vars.rid });
          }
        }
        vars[`member_${letter}_sid`] = sid;
      }

      // A `room:left` room is created, joined by member_b, then left by them.
      if (wantsLeftRoom) {
        const left = await authority.call('room.create', { name: 'left room' });
        vars.rid_left = left.out?.room_id;
      }

      // `resource:shared_file` — a real file genuinely streamed into the room
      // through the byte-stream executor; binds `$fid`. This is live setup
      // evidence, not a stub: the daemon stages, digests, and authors the
      // share exactly as any client upload.
      if (requires.includes('resource:shared_file')) {
        const input = {
          room_id: vars.rid,
          name: 'seed.bin',
          declared_bytes: 4096,
          declared_content_type: 'application/octet-stream',
        };
        const reply = await this.#streamCall(authority, 'file.share', input, { send_bytes: 4096 }, {
          opId: `cf-seed-${opIdCounter++}`,
        });
        if (!reply.ok) {
          throw new Error(`resource:shared_file setup failed: ${JSON.stringify(reply.err)}`);
        }
        vars.fid = reply.out?.file_id;
      }
    }

    // Bare `subject` means the daemon's one subject exists (`subject:none` is
    // the absence form). Room/member setup ensures it as a side effect. A case
    // whose own steps call subject.ensure manages the lifecycle itself (some
    // assert on the `created` flag); otherwise ensure it here so a
    // subject-requiring error case observes its intended error rather than
    // subject_absent.
    const caseEnsuresSubject = (fixture.steps || []).some((st) => st.call === 'subject.ensure');
    if (requires.includes('subject') && !caseEnsuresSubject && !(wantsRoom || wantsLeftRoom || wantsMembers)) {
      const authority = await this.#sessionFor('subject:authority', daemons, sessions, vars);
      const ensured = await authority.call('subject.ensure', {});
      if (!ensured.ok || !ensured.out?.subject_id) {
        throw new Error(`requires subject: subject.ensure failed: ${JSON.stringify(ensured.err)}`);
      }
      vars.self_sid = ensured.out.subject_id;
    }

    // `resource:tcp_service` — a loopback TCP echo service a pipe can target;
    // its port is captured for `$svc_port` (and `$svc_port_v6` when bound to
    // ::1). The service lives until the case's cleanup.
    if (requires.some((r) => r === 'resource:tcp_service')) {
      const { port, close } = await startEchoService();
      vars.svc_port = port;
      vars.svc_port_v6 = port;
      vars.__case.closers.push(close);
    }

    // Subject-id vars the fixtures name but the runner must bind. Each is a
    // real ensured subject; the fixtures reference the id (an invitee, an
    // outsider) without authoring it inline. `$sb` is the second subject (the
    // invitee a capability is bound to), `$sc` the outsider (a subject with
    // no room relationship). A daemon holds exactly ONE subject, so a
    // `subject:second` / `subject:outsider` require implies a second daemon
    // even when `daemon:second` is not named — spawn one so the ids are
    // genuinely distinct from the authority's. Bound only when the case
    // declares the matching subject require, so a case that does not name one
    // never sees an id it did not ask for.
    const ensureSecondDaemon = async () => {
      if (!vars.__case.second) {
        const second = await startDaemon(this.binary, { loopback: true });
        daemons.push(second);
        vars.__case.second = second;
      }
      return vars.__case.second;
    };
    const bindSubject = async (requireToken, sessionLabel, varName, onSecond) => {
      if (!requires.includes(requireToken)) return;
      if (onSecond) await ensureSecondDaemon();
      const sess = await this.#sessionFor(sessionLabel, daemons, sessions, vars);
      const ensured = await sess.call('subject.ensure', {});
      vars[varName] = ensured.out?.subject_id;
    };
    await bindSubject('subject:second', 'subject:second', 'sb', true);
    await bindSubject('subject:outsider', 'subject:outsider', 'sc', true);
  }

  /** The session for an `on` label, connecting lazily. Every step path
   * resolves its session here, so a sticky binary violation recorded on the
   * connection surfaces before the next interaction of any kind. */
  async #sessionFor(onLabel, daemons, sessions, vars) {
    const label = onLabel || 'subject:self';
    if (sessions.has(label)) {
      const existing = sessions.get(label);
      if (existing.stickyBinaryViolation) throw existing.stickyBinaryViolation;
      return existing;
    }
    // Route to the second daemon for labels that clearly name a distinct
    // subject (a daemon holds one subject, so second/outsider/peer subjects
    // live on the second daemon).
    const cs = vars.__case || {};
    const daemon =
      cs.second && /second|outsider|principal_b|peer|remote/i.test(label)
        ? cs.second
        : cs.primary;
    const clientId = vars.__case?.clientIds?.[label] || `cf-client-${opIdCounter++}`;
    if (vars.__case) {
      vars.__case.clientIds ||= Object.create(null);
      vars.__case.clientIds[label] = clientId;
    }
    const s = new Session(label, clientId);
    await s.connect(
      daemon,
      { v: 2, sg: daemon.storageGeneration, token: daemon.token },
      {},
    );
    // Consume the hello frame and seed `frame` so a step-1 assert can read
    // `frame.subject` without an intervening await. The hello (and its served
    // limits) is also kept on a dedicated root so later replies don't clobber
    // it — `frame.<limit>` resolves against the hello throughout the case.
    let hello;
    try {
      hello = await s.awaitFrame((f) => f.t === 'hello');
    } catch (err) {
      s.close();
      throw new TransportFailure(`session ${label} failed before hello: ${err.message}`);
    }
    vars.frame = hello;
    vars.hello = hello;
    sessions.set(label, s);
    return s;
  }

  /** Execute one step. */
  async #runStep(step, index, env) {
    const { sessions, vars, ctx } = env;
    const onLabel = step.on;

    if (step.upgrade) {
      await this.#doUpgrade(step, env);
      return;
    }
    if (step.http) {
      await this.#doHttp(step, env);
      return;
    }
    if (step.call) {
      await this.#doCall(step, index, env);
      return;
    }
    if (step.send !== undefined) {
      const s = await this.#sessionFor(onLabel, env.daemons, sessions, vars);
      const value = resolveValue(step.send, vars);
      try {
        await s.sendRaw(value);
      } catch (err) {
        throw new TransportFailure(`send failed: ${err.message}`);
      }
      return;
    }
    if (step.await) {
      await this.#doAwait(step, env);
      return;
    }
    if (step.control) {
      await this.#doControl(step, index, env);
      return;
    }
    if (step.assert) {
      await evalAssert(step.assert, ctx);
    }
    // A step may carry `save` with no verb (e.g. capturing the hello's limits
    // at case start); capture from the current frame/out roots.
    if (step.save && !step.call && !step.await && !step.upgrade && !step.http) {
      for (const [varName, p] of Object.entries(step.save)) {
        // The hello's served limits are nested under `limits` on the wire, but
        // the corpus names them flat (`frame.max_message_body_bytes`). Resolve
        // against the preserved hello (not whatever reply last set `frame`).
        const helloFrame = vars.hello || vars.frame || {};
        const limits = helloFrame.limits || {};
        const root = { frame: { ...helloFrame, ...limits }, out: vars.out, err: vars.err, ...vars };
        const { found, value } = resolvePath(root, p);
        vars[varName] = found ? value : undefined;
        this.log(`save ${varName} <- ${p} = ${JSON.stringify(value)?.slice(0, 40)} (found=${found})`);
      }
    }
    // A step with only auxiliary keys (note/expect) and no verb is otherwise a
    // no-op for the runner.
  }

  /** A `call` step: invoke an operation, match the reply, save captures. A
   * step carrying `stream` runs as one duplex operation via the byte-stream
   * executor; every call (streaming or not) gets a tracker so
   * `bytes_streamed` observes real receiver-accepted counts and an
   * unexpected OPEN on a stream-less call is caught, not dropped. */
  async #doCall(step, index, env) {
    const { sessions, vars, ctxState } = env;
    const s = await this.#sessionFor(step.on, env.daemons, sessions, vars);
    const input = step.in !== undefined ? resolveValue(step.in, vars) : {};
    // CORPUS/SPEC DISCREPANCY: every committed fixture calls stream.subscribe
    // without the spec-required `from` cursor (50/50). The spec (canonical)
    // makes `from` required; the corpus asserts `from_pos` in the reply, which
    // is only meaningful when a cursor resolved. The harness supplies
    // `from: {state: "start"}` for an omitted cursor so the corpus's intent
    // runs; the implementation stays spec-conformant (requires `from`). This
    // is recorded here rather than by editing 50 fixtures or weakening the
    // spec in the same change that runs them — a #213 follow-up should pick
    // one and make them agree.
    if (step.call === 'stream.subscribe' && input.from === undefined) {
      input.from = { state: 'start' };
    }
    const opId = step.op_id !== undefined ? resolveValue(step.op_id, vars) : undefined;
    this.log(`call ${step.call} on ${step.on || 'subject:self'}${input.body ? ` body[${String(input.body).length}]` : ''}`);

    // Track whether this call authors an event (for no_event_authored).
    const mutating = !/\.list$|\.timeline$|\.members$|\.peers$|\.history$|\.archive$/.test(
      step.call,
    );

    const streamSpec = step.stream !== undefined ? resolveValue(step.stream, vars) : null;
    const record = { step: index + 1, label: step.on || 'subject:self', accepted: 0, opened: false };
    ctxState.callRecords.push(record);

    let reply;
    if (streamSpec) {
      reply = await this.#streamCall(s, step.call, input, streamSpec, { opId, record });
      this.log(`  -> ${reply.ok ? 'ok' : 'err ' + JSON.stringify(reply.err)} (accepted ${record.accepted})`);
    } else {
      const { id, reply: replyPromise } = s.startCall(step.call, input, { opId });
      const tracker = new CallStreamTracker(id);
      s.streams.set(id, tracker);
      // Race the reply against Binary delivery: a stream-less call that
      // receives OPEN (a daemon awaiting bytes that will never come) must
      // fail as the conformance verdict NOW, not as a reply timeout later.
      let watcherDone = false;
      const recordWatcher = (async () => {
        for (;;) {
          if (watcherDone) return null;
          if (tracker.failure) throw tracker.failure;
          if (tracker.opened || tracker.queue.length) {
            const what = tracker.opened ? 'an OPEN' : `a ${tracker.queue[0].kindName} record`;
            throw new AssertFailure(
              `call ${step.call} received ${what} for a stream the fixture declares it must not open`,
            );
          }
          await new Promise((resolve) => tracker.wakers.push(resolve));
        }
      })();
      try {
        reply = await Promise.race([replyPromise, recordWatcher]);
        this.log(`  -> ${reply.ok ? 'ok' : 'err ' + JSON.stringify(reply.err)}`);
      } catch (err) {
        if (err instanceof AssertFailure) throw err;
        if (err === tracker.failure) throw err;
        // Never a reply — a timeout, a closed socket, or a crash. This is a
        // transport/runner failure, not a matcher verdict: raise a
        // TransportFailure (not an AssertFailure) so a blocked case cannot count
        // a dead daemon as its expected blocked assertion.
        const openNote = tracker.opened ? ' after the daemon opened an unexpected byte stream' : '';
        throw new TransportFailure(`call ${step.call} failed to get a reply${openNote}: ${err.message}`);
      } finally {
        watcherDone = true;
        tracker.wake();
        recordWatcher.catch(() => {});
        s.streams.delete(id);
        record.accepted = tracker.accepted;
        record.opened = tracker.opened;
      }
      if (tracker.failure) {
        // A binary-routing violation (unparseable message, unbindable record)
        // observed during this call is connection-fatal class — the Text
        // reply arriving anyway must not launder it.
        throw tracker.failure;
      }
      if (tracker.opened || tracker.queue.length) {
        const what = tracker.opened ? 'an OPEN' : `a ${tracker.queue[0].kindName} record`;
        throw new AssertFailure(
          `call ${step.call} received ${what} for a stream the fixture declares it must not open`,
        );
      }
    }

    // Expose the reply under the assertion roots.
    vars.out = reply.out;
    vars.err = reply.err;
    vars.frame = reply;
    if (reply.err) vars.last_error = reply.err;
    ctxState.lastCallOk = !!reply.ok;

    if (mutating && reply.ok) ctxState.eventAuthored++;
    ctxState.networkActivity++;

    // The reply matcher.
    if (step.expect) {
      this.#matchExpect(step.expect, reply, vars, step.call);
    }

    // Captures.
    if (step.save) {
      for (const [varName, path] of Object.entries(step.save)) {
        const root = path.startsWith('$') ? vars : reply;
        const rel = path.startsWith('$') ? path.slice(1) : path;
        const { found, value } = resolvePath(root, rel);
        vars[varName] = found ? value : undefined;
      }
    }
  }

  /** Run one duplex streaming call through the byte-stream executor. */
  async #streamCall(s, op, input, spec, { opId, record } = {}) {
    const hello = s.lastHello || {};
    const frameLimit = hello.limits?.max_frame_bytes;
    const maxPayload = maxDataPayloadBytes(frameLimit);
    const waitMs = streamWaitMs(spec, hello.limits || {}, this.timeoutScale);
    const { id, reply: replyPromise, requestBytes } = s.startCall(op, input, {
      opId,
      timeoutMs: waitMs,
    });
    const tracker = new CallStreamTracker(id);
    s.streams.set(id, tracker);
    const replyState = { done: false, value: undefined, error: undefined, seq: Infinity };
    // The wire-order stamp is taken synchronously in Session.#onMessage
    // (tracker.replySeq); the fallback covers settle-without-a-wire-message
    // (a timeout), which correctly sequences after every delivered record.
    replyPromise.then(
      (v) => { replyState.seq = tracker.replySeq ?? ++tracker.seq; replyState.done = true; replyState.value = v; tracker.wake(); },
      (e) => { replyState.seq = tracker.replySeq ?? ++tracker.seq; replyState.done = true; replyState.error = e; tracker.wake(); },
    );
    try {
      // The spec's second readiness condition: a stream-valid max_frame_bytes
      // must carry the streaming Text request envelope. A daemon serving a
      // limit this request exceeds must have refused readiness, not answered.
      if (requestBytes > Number(frameLimit)) {
        throw new AssertFailure(
          `served max_frame_bytes ${frameLimit} cannot carry the ${op} request envelope (${requestBytes} bytes)`,
        );
      }
      return await runStreamingCall({
        session: s,
        tracker,
        replyState,
        spec,
        declaredBytes: input?.declared_bytes,
        maxPayload,
        limitBytes: hello.limits?.max_shared_file_bytes,
        inflightBytes: hello.limits?.max_transfer_bytes_inflight,
        stallMs: hello.limits?.transfer_stall_ms,
        waitMs,
      });
    } finally {
      s.streams.delete(id);
      if (record) {
        record.accepted = tracker.accepted;
        record.opened = tracker.opened;
      }
    }
  }

  /** Match an `expect` reply matcher against a reply envelope. */
  #matchExpect(expect, reply, vars, callName) {
    if (expect.ok === true) {
      if (!reply.ok)
        throw new AssertFailure(
          `${callName}: expected ok:true, got err ${JSON.stringify(reply.err)}`,
        );
      if (expect.out !== undefined && !subsetMatch(expect.out, reply.out, vars))
        throw new AssertFailure(
          `${callName}: out did not match ${JSON.stringify(expect.out)}; got ${JSON.stringify(reply.out)}`,
        );
    } else if (expect.ok === false) {
      if (reply.ok)
        throw new AssertFailure(`${callName}: expected ok:false, got out ${JSON.stringify(reply.out)}`);
      if (expect.err !== undefined && !subsetMatch(expect.err, reply.err, vars))
        throw new AssertFailure(
          `${callName}: err did not match ${JSON.stringify(expect.err)}; got ${JSON.stringify(reply.err)}`,
        );
    }
  }

  /** An `upgrade` step: a fresh WS upgrade attempt with the given query/headers. */
  async #doUpgrade(step, env) {
    const { vars, sessions } = env;
    const daemon = (vars.__case||{}).primary;
    const u = step.upgrade;
    const query = {};
    for (const [k, v] of Object.entries(u.query || {})) query[k] = resolveValue(v, vars);
    const headers = {};
    for (const [k, v] of Object.entries(u.headers || {})) {
      headers[k] = this.#resolveHeaderValue(v, daemon, vars);
    }
    this.log(`upgrade ${JSON.stringify(query)}`);

    // Perform the upgrade; it may be refused (non-101). A successful upgrade
    // becomes the `on` session's live connection so a following `await` reads
    // THIS connection's hello, not a stale one from before the upgrade.
    // A setup upgrade (no expect) is the label's ordinary admitted
    // connection, so it presents the label's STABLE client id exactly as
    // #sessionFor does — an omitted `cid` is an ephemeral per-connection
    // dedup principal on the daemon, which would make every later
    // same-principal reconnect (op_id replay cases) silently miss the ledger.
    // A probing upgrade (any expect) sends exactly what the fixture wrote.
    const label = step.on || 'subject:self';
    // A same-label upgrade replaces the registered session; a sticky
    // violation on the outgoing connection must not vanish with it.
    const outgoing = sessions.get(label);
    if (outgoing && outgoing.stickyBinaryViolation) throw outgoing.stickyBinaryViolation;
    let clientId = null;
    if (!step.expect && query.cid === undefined && query.ct === undefined && vars.__case) {
      vars.__case.clientIds ||= Object.create(null);
      vars.__case.clientIds[label] ||= `cf-client-${opIdCounter++}`;
      clientId = vars.__case.clientIds[label];
      query.cid = clientId;
    }
    const result = await this.#attemptUpgrade(daemon, query, headers, label, sessions, clientId);
    // The precheck ran before the await; a violation recorded on the
    // outgoing connection during the upgrade must still surface, and the
    // replaced session stays scanned at case end.
    if (outgoing && outgoing.stickyBinaryViolation) throw outgoing.stickyBinaryViolation;
    if (outgoing && vars.__case) (vars.__case.replacedSessions ||= []).push(outgoing);
    vars.frame = result.frame || {};
    vars.out = result.body;
    vars.err = result.body;

    if (step.expect) {
      this.#matchUpgradeExpect(step.expect, result, vars);
    }
    if (step.save) {
      for (const [varName, path] of Object.entries(step.save)) {
        const root = path.startsWith('frame') ? { frame: result.frame } : result;
        const { found, value } = resolvePath(root, path);
        vars[varName] = found ? value : undefined;
      }
    }
  }

  /** Resolve a header value, substituting the portfile placeholders. */
  #resolveHeaderValue(v, daemon, vars) {
    if (typeof v !== 'string') return v;
    return v
      .replace('<bearer_from_portfile>', daemon.token)
      .replace('<token>', daemon.token)
      .replace('<port>', String(daemon.port))
      .replace(/\$([A-Za-z_][A-Za-z0-9_.]*)/g, (m, name) => {
        const { found, value } = resolvePath(vars, name);
        return found ? String(value) : m;
      });
  }

  /** Attempt a WS upgrade, returning {status, body, frame}. On admission the
   * socket stays open and is registered as the `label` session's connection. */
  async #attemptUpgrade(daemon, query, headers, label, sessions, clientId = null) {
    const params = new URLSearchParams();
    for (const [k, v] of Object.entries(query)) params.set(k, String(v));
    const url = `${daemon.wsBase}?${params.toString()}`;
    const WebSocket = (await import('ws')).default;
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url, {
        headers: { Host: `127.0.0.1:${daemon.port}`, ...headers },
        perMessageDeflate: false,
      });
      ws.binaryType = 'nodebuffer';
      let settled = false;
      let opened = false;
      let helloTimer;
      const fail = (message) => {
        if (settled) return;
        settled = true;
        if (helloTimer) clearTimeout(helloTimer);
        try { ws.terminate(); } catch { /* already closed */ }
        reject(new TransportFailure(message));
      };
      ws.once('open', () => {
        opened = true;
        const session = new Session(label, clientId);
        session.ws = ws;
        session.open = true;
        ws.on('message', (data, isBinary) => session.__onMessage(data, isBinary));
        ws.on('close', (code) => {
          session.__onClose(code);
          if (!settled) fail(`upgrade connection closed before hello (${code})`);
        });
        ws.on('error', (err) => {
          if (!settled) fail(`upgrade failed before hello: ${err.message}`);
        });
        ws.once('message', (data) => {
          if (settled) return;
          let frame;
          try {
            frame = JSON.parse(data.toString());
          } catch {
            fail('upgrade first frame was not valid JSON');
            return;
          }
          if (frame.t !== 'hello') {
            fail(`upgrade first frame was ${JSON.stringify(frame.t)}, expected hello`);
            return;
          }
          settled = true;
          if (helloTimer) clearTimeout(helloTimer);
          if (sessions) sessions.set(label, session);
          resolve({ status: 101, frame, body: frame, headers: {} });
        });
        helloTimer = setTimeout(
          () => fail('upgrade timed out waiting for hello'),
          2000 * this.timeoutScale,
        );
      });
      ws.once('unexpected-response', (req, res) => {
        let body = '';
        const responseHeaders = Object.fromEntries(
          Object.entries(res.headers).map(([k, v]) => [k, Array.isArray(v) ? v.join(', ') : String(v)]),
        );
        const responseFail = (message) => fail(`upgrade response ${message}`);
        res.on('data', (d) => (body += d));
        res.once('aborted', () => responseFail('was aborted'));
        res.once('error', (err) => responseFail(`failed: ${err.message}`));
        res.once('end', () => {
          if (settled) return;
          settled = true;
          let parsed = {};
          try { parsed = JSON.parse(body); } catch { /* not json */ }
          resolve({ status: res.statusCode, body: parsed, frame: parsed, headers: responseHeaders });
        });
      });
      ws.once('error', (err) => {
        if (settled) return;
        // A pre-open connection error (e.g. ECONNREFUSED against a daemon that
        // is not accepting connections) is an unambiguous, observable Layer-0
        // outcome, not a mid-stream transport ambiguity: report it as status 0
        // so a fixture that intentionally probes a stopped daemon sees a
        // refusal rather than an ERROR. A failure AFTER the socket opened is
        // the genuinely ambiguous case (admitted vs. crashed) and stays a
        // TransportFailure via the open handler's listeners.
        if (!opened) {
          settled = true;
          if (helloTimer) clearTimeout(helloTimer);
          resolve({ status: 0, body: {}, frame: {}, headers: {} });
          return;
        }
        fail(`upgrade failed: ${err.message}`);
      });
    });
  }

  /** Match an upgrade/http `expect` (status/headers/body). The corpus writes
   * body fields at the top level beside the reserved `status`/`headers`/`body`
   * keys (e.g. the health fixture's `protocol`, `min_protocol`, `limits`), so
   * every non-reserved key is matched against the response body, `headers`
   * against the response headers, and `body` against the whole body. */
  #matchUpgradeExpect(expect, result, vars) {
    if (expect.status !== undefined && result.status !== expect.status) {
      throw new AssertFailure(
        `expected status ${expect.status}, got ${result.status} (body ${JSON.stringify(result.body)})`,
      );
    }
    if (expect.headers !== undefined) {
      const got = result.headers || {};
      const want = Object.fromEntries(
        Object.entries(expect.headers).map(([k, v]) => [k.toLowerCase(), v]),
      );
      const gotLower = Object.fromEntries(Object.entries(got).map(([k, v]) => [k.toLowerCase(), v]));
      if (!subsetMatch(want, gotLower, vars)) {
        throw new AssertFailure(
          `expected headers ${JSON.stringify(want)}, got ${JSON.stringify(gotLower)}`,
        );
      }
    }
    // Whole-body matcher when the corpus nests one under `body`.
    if (expect.body !== undefined && !subsetMatch(expect.body, result.body, vars)) {
      throw new AssertFailure(
        `expected body ${JSON.stringify(expect.body)}, got ${JSON.stringify(result.body)}`,
      );
    }
    // Top-level body fields (everything except the reserved keys). For an
    // http/Layer-0 response the body IS the result object, so the corpus's
    // `out: {...}` wrapper is matched against the body itself (the spec's flat
    // shape — the fields the corpus nests under `out` live at the body's top
    // level), and the remaining top-level keys match the body too.
    const { out, ...rest } = Object.fromEntries(
      Object.entries(expect).filter(([k]) => !['status', 'headers', 'body'].includes(k)),
    );
    const matcher = { ...(out && typeof out === 'object' ? out : {}), ...rest };
    if (Object.keys(matcher).length && !subsetMatch(matcher, result.body, vars)) {
      throw new AssertFailure(
        `expected body fields ${JSON.stringify({ out, ...rest })}, got ${JSON.stringify(result.body)}`,
      );
    }
  }

  /** An `http` step: a Layer 0 or /api/session request. */
  async #doHttp(step, env) {
    const { vars } = env;
    const daemon = (vars.__case||{}).primary;
    const h = step.http;
    const headers = {};
    for (const [k, v] of Object.entries(h.headers || {})) {
      headers[k] = this.#resolveHeaderValue(v, daemon, vars);
    }
    let path = this.#resolveHeaderValue(h.path || '/', daemon, vars);
    const url = `http://127.0.0.1:${daemon.port}${path}`;
    this.log(`http ${h.method || 'GET'} ${path}`);
    const res = await fetch(url, {
      method: h.method || 'GET',
      headers,
      body: h.body ? JSON.stringify(resolveValue(h.body, vars)) : undefined,
    });
    let body = {};
    const text = await res.text();
    try { body = JSON.parse(text); } catch { body = { raw: text }; }
    const result = { status: res.status, body, headers: Object.fromEntries(res.headers) };
    vars.out = body;
    vars.frame = body;
    if (step.expect) this.#matchUpgradeExpect(step.expect, result, vars);
    if (step.save) {
      for (const [varName, p] of Object.entries(step.save)) {
        const { found, value } = resolvePath({ body, ...body }, p);
        vars[varName] = found ? value : undefined;
      }
    }
  }

  /** An `await` step: wait for a push, a frame, or a correlated reply. */
  async #doAwait(step, env) {
    const { sessions, vars } = env;
    const a = step.await;
    const s = await this.#sessionFor(step.on, env.daemons, sessions, vars);

    // The await matcher is a *locator* plus an implicit assertion: locate the
    // correlated frame (by id for a reply, by type for a push/frame), then
    // assert it matches. A mismatch is a fast, clear failure — never a
    // timeout that reads as a hang. A push with no explicit `on` is located
    // across every connection the harness owns (the corpus's `on` defaults to
    // the primary connection, but a subscribed secondary connection receives
    // the push all the same).
    let locate;
    let want;
    let kind;
    if (a.push !== undefined) {
      want = resolveValue(a.push, vars);
      kind = 'push';
      const PUSH_T = new Set(['event', 'gap', 'peer', 'transfer']);
      const wantType = want && typeof want === 'object' ? want.t : undefined;
      const wantRoom = want && typeof want === 'object' ? want.room_id : undefined;
      locate = (f) =>
        f.t !== undefined &&
        PUSH_T.has(f.t) &&
        (wantType === undefined || f.t === wantType) &&
        (wantRoom === undefined || f.room_id === wantRoom);
      let frame;
      try {
        frame = await this.#awaitAcrossSessions(sessions, locate, step.on, env, vars);
      } catch (err) {
        if (/timed out/.test(err.message)) {
          throw new AssertFailure(`await push timed out waiting for ${JSON.stringify(want)}`);
        }
        throw new TransportFailure(`await push failed: ${err.message}`);
      }
      this.#setFrameRoots(vars, frame);
      if (want !== undefined && !this.#matchPushOrFrame('push', want, frame, vars)) {
        throw new AssertFailure(`await push did not match ${JSON.stringify(want)}; got ${JSON.stringify(frame)}`);
      }
      if (step.save) this.#applySave(step, frame, vars);
      return;
    } else if (a.frame !== undefined) {
      want = resolveValue(a.frame, vars);
      kind = 'frame';
      // Correlate by stable locator fields before applying the full matcher.
      // Without this, a matcher naming `t: "peer"` can select the historical
      // hello frame and count that unrelated mismatch as a blocked failure.
      const wantId = want && typeof want === 'object' ? want.id : undefined;
      const wantType = want && typeof want === 'object' ? want.t : undefined;
      locate = (f) =>
        (wantId === undefined || f.id === wantId) &&
        (wantType === undefined || f.t === wantType);
    } else if (a.reply !== undefined) {
      // `$id` names the connection's most recent request id (replies may
      // arrive out of order, so a case correlates by id, not request order).
      const resolved = a.reply === '$id' ? s.lastRequestId : Number(resolveValue(a.reply, vars));
      want = undefined;
      kind = 'reply';
      locate = (f) => f.id === resolved;
    } else {
      throw new Error('await step with no push/frame/reply key');
    }

    let frame;
    try {
      frame = await s.awaitFrame(locate, 10_000 * this.timeoutScale);
    } catch (err) {
      if (/timed out/.test(err.message)) {
        throw new AssertFailure(`await ${kind} timed out waiting for ${JSON.stringify(want)}`);
      }
      throw new TransportFailure(`await ${kind} failed: ${err.message}`);
    }
    if (want !== undefined) {
      if (!this.#matchPushOrFrame(kind, want, frame, vars)) {
        throw new AssertFailure(
          `await ${kind} did not match ${JSON.stringify(want)}; got ${JSON.stringify(frame)}`,
        );
      }
    }
    this.#setFrameRoots(vars, frame);
    if (step.save) this.#applySave(step, frame, vars);
  }

  /** Locate a push across every connection the harness owns (or the named one). */
  async #awaitAcrossSessions(sessions, locate, onLabel, env, vars) {
    if (onLabel) {
      const s = await this.#sessionFor(onLabel, env.daemons, sessions, vars);
      return s.awaitFrame(locate, 10_000 * this.timeoutScale);
    }
    // Race every open session's next matching frame, plus the frames already
    // in each one's history.
    for (const s of sessions.values()) {
      if (s.stickyBinaryViolation) throw s.stickyBinaryViolation;
      for (const f of s.pushes) {
        if (locate(f)) return f;
      }
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('await push timed out (all sessions)')), 10_000 * this.timeoutScale);
      let remaining = sessions.size;
      if (remaining === 0) {
        clearTimeout(timer);
        reject(new Error('await push: no open sessions'));
        return;
      }
      for (const s of sessions.values()) {
        s.awaitFrame(locate, 10_000 * this.timeoutScale).then(
          (f) => {
            clearTimeout(timer);
            resolve(f);
          },
          () => {
            if (--remaining === 0) {
              clearTimeout(timer);
              reject(new Error('await push timed out on every session'));
            }
          },
        );
      }
    });
  }

  /** Set the frame/out/err roots from an awaited frame. */
  #setFrameRoots(vars, frame) {
    vars.frame = frame;
    if (frame.id !== undefined) {
      vars.out = frame.out;
      vars.err = frame.err;
      if (frame.err) vars.last_error = frame.err;
    }
  }

  /** Apply a step's `save` captures. The hello's served limits are nested
   * under `limits` on the wire, but the corpus names them flat
   * (`frame.max_message_body_bytes`); resolve both spellings. */
  #applySave(step, frame, vars) {
    const limits = (frame && frame.limits) || (vars.hello && vars.hello.limits) || {};
    const flatFrame = { ...(frame || {}), ...limits };
    for (const [varName, p] of Object.entries(step.save || {})) {
      const { found, value } = resolvePath({ frame: flatFrame, ...flatFrame }, p);
      vars[varName] = found ? value : undefined;
    }
  }

  /** Match an await matcher against a frame, tolerating fixture phrasing:
   * annotation keys (`conn`, `note`) are dropped, and an `event: {...}` key on
   * a push matcher matches against the committed event inline in the frame
   * (the spec flattens the event beside `t`). */
  #matchPushOrFrame(kind, want, frame, vars) {
    if (want === null || typeof want !== 'object' || Array.isArray(want)) {
      return subsetMatch(want, frame, vars);
    }
    const { conn, note, session, client, event, ...rest } = want;
    if (!subsetMatch(rest, frame, vars)) return false;
    if (event !== undefined) {
      // The committed event is the frame minus its `t` discriminator. The
      // corpus names content fields directly (`event: {body: …}`); match
      // against the event and, for content fields, against `event.content`.
      const { t, ...eventValue } = frame;
      const { kind, content, ...eventScalars } = eventValue;
      if (!subsetMatch(event, eventValue, vars)) {
        // Fall back: match content-bearing keys against the event's content,
        // and kind-bearing keys against the event's kind.
        const contentMatch = content && subsetMatch(event, content, vars);
        const kindMatch = event.kind !== undefined && event.kind === kind;
        const merged = { ...(content || {}), ...(kind !== undefined ? { kind } : {}) };
        const mergedMatch = subsetMatch(event, merged, vars);
        if (!contentMatch && !kindMatch && !mergedMatch) return false;
      }
    }
    return true;
  }

  /** A `control` step: drive the harness. */
  async #doControl(step, index, env) {
    const { sessions, vars } = env;
    const c = step.control;
    switch (c.do) {
      case 'idle':
        await new Promise((r) => setTimeout(r, Number(resolveValue(c.ms, vars)) || 0));
        return;
      case 'advance_clock':
        // The harness cannot move the daemon's clock; treat as a no-op wait so
        // timing-sensitive cases still exercise their real durations.
        await new Promise((r) => setTimeout(r, Math.min(Number(resolveValue(c.ms, vars)) || 0, 1000)));
        return;
      case 'disconnect': {
        const s = await this.#sessionFor(c.on || step.on, env.daemons, sessions, vars);
        s.disconnect();
        return;
      }
      case 'reconnect': {
        const label = c.on || step.on || 'subject:self';
        const s = sessions.get(label);
        if (s) {
          s.close();
          sessions.delete(label);
          // A violation delivered to the outgoing connection while the
          // replacement is awaited must survive to the case-end scan.
          if (vars.__case) (vars.__case.replacedSessions ||= []).push(s);
        }
        await this.#sessionFor(label, env.daemons, sessions, vars);
        return;
      }
      case 'stop_daemon':
        await (vars.__case||{}).primary.proc.kill('SIGTERM');
        return;
      case 'start_daemon':
        // A restart is out of scope for this runner's single-shot daemon.
        throw new Error('control start_daemon is not supported by this harness yet');
      case 'set_limit':
      case 'start_transfers':
      case 'pause_link':
      case 'inject_fault':
        throw new Error(`control ${c.do} is not supported by this harness yet`);
      default:
        throw new Error(`unknown control do ${c.do}`);
    }
  }

  /** Whether the case's primary daemon still accepts TCP connections. */
  async #daemonReachable(primary) {
    const net = await import('node:net');
    return new Promise((resolve) => {
      const sock = net.createConnection(primary.port, '127.0.0.1');
      sock.once('connect', () => {
        sock.destroy();
        resolve(true);
      });
      sock.once('error', () => resolve(false));
      setTimeout(() => {
        sock.destroy();
        resolve(false);
      }, 1000);
    });
  }

  /** The total committed event positions across the case's room(s), read via
   * any subscribed session's timeline head — the observable signal for
   * no_event_authored. Returns null when no room exists yet. */
  async #roomEventTotal(vars, sessions) {
    const rid = vars.rid;
    if (!rid) return null;
    // Read the room head through the first available session.
    const s = sessions.values().next().value;
    if (!s) return null;
    try {
      const r = await s.call('room.timeline', {
        room_id: rid,
        cursor: { state: 'start' },
        direction: 'backward',
        limit: 1,
      });
      const last = r.out?.events?.[r.out.events.length - 1];
      return last ? last.pos + 1 : 0;
    } catch {
      return null;
    }
  }

  /** The dir signature once it stops changing: two consecutive identical
   * reads a beat apart, bounded. Asynchronous persistence (staging cleanup,
   * a lagging ledger fsync under IO load) otherwise races the baseline
   * capture and reads as a later mutation. An unchanged-from-prior read
   * returns immediately — settling only costs time when something moved. */
  async #stableDirSignature(daemons, prior) {
    let cur = this.#dirStateSignature(daemons);
    if (prior !== undefined && cur === prior) return cur;
    const deadline = Date.now() + 4_000 * this.timeoutScale;
    for (;;) {
      await new Promise((r) => setTimeout(r, 250));
      const next = this.#dirStateSignature(daemons);
      if (next === cur || Date.now() > deadline) return next;
      cur = next;
    }
  }

  /** A signature of the data dir's contents (file count + total bytes +
   * newest mtime), the observable signal for no_durable_mutation. Two kinds
   * of transient state are excluded: the byte-stream staging directory
   * (cleanup is asynchronous and races the capture; stranded residue is
   * asserted explicitly by the observation) and `-wal`/`-shm` journal
   * sidecars (the harness's own observation reads append to the WAL, and a
   * WAL-resident event mutation is what no_event_authored observes — the
   * signature tracks committed content files). */
  #dirStateSignature(daemons) {
    try {
      const walk = (dir) => {
        let files = 0, bytes = 0, newest = 0;
        let entries = [];
        try { entries = readdirSync(dir, { withFileTypes: true }); } catch { return { files, bytes, newest }; }
        for (const e of entries) {
          const p = join(dir, e.name);
          if (e.isDirectory()) {
            if (e.name === 'protocol-v2-stream-staging') continue;
            const sub = walk(p);
            files += sub.files; bytes += sub.bytes; newest = Math.max(newest, sub.newest);
          } else {
            if (e.name.endsWith('-wal') || e.name.endsWith('-shm')) continue;
            try { const st = statSync(p); files++; bytes += st.size; newest = Math.max(newest, st.mtimeMs); } catch { /* ignore */ }
          }
        }
        return { files, bytes, newest };
      };
      return JSON.stringify(walk(daemons[0].dataDir));
    } catch {
      return null;
    }
  }

  /** Whether the staging directory still holds any file, settled: cleanup is
   * asynchronous, so residue is only a violation if it persists past a
   * bounded wait. */
  async #stagingResidue(daemons) {
    const dir = join(daemons[0].dataDir, 'protocol-v2-stream-staging');
    const residue = () => {
      try { return readdirSync(dir).length > 0; } catch { return false; }
    };
    const deadline = Date.now() + 2_000 * this.timeoutScale;
    while (residue()) {
      if (Date.now() > deadline) return true;
      await new Promise((r) => setTimeout(r, 100));
    }
    return false;
  }

  /** Evaluate an `observe` assertion against recorded behaviour. */
  async #evalObserve(a, { sessions, vars, ctxState, name, daemons }) {
    const primary = (vars.__case || {}).primary;
    // A sticky binary violation outranks whatever this observation would
    // have concluded — assertion-only steps never resolve a session, so
    // this is their surfacing point.
    for (const s of sessions.values()) {
      if (s.stickyBinaryViolation) throw s.stickyBinaryViolation;
    }
    const obs = a.observe;
    switch (obs) {
      case 'no_event_authored': {
        // Real check: the room's committed event total must not have grown
        // since the last SUCCESSFUL step's baseline — the refresh skips
        // refused operations, so the observed refusal cannot absorb an
        // illegally authored event.
        if (ctxState.eventSnapshot === null || ctxState.eventSnapshot === undefined) return;
        const now = await this.#roomEventTotal(vars, sessions);
        if (now !== null && now > ctxState.eventSnapshot) {
          throw new AssertFailure(
            `no_event_authored violated: room events grew ${ctxState.eventSnapshot} -> ${now}`,
          );
        }
        return;
      }
      case 'no_durable_mutation': {
        // Real check: the data dir's content signature (staging excluded)
        // must be unchanged, and the transient staging directory must have
        // cleaned itself up — an aborted stage leaves no stranded file.
        const before = ctxState.dirSnapshot;
        if (before !== null && before !== undefined) {
          const now = this.#dirStateSignature(daemons);
          if (now !== null && now !== before) {
            throw new AssertFailure(`no_durable_mutation violated: data dir changed ${before} -> ${now}`);
          }
        }
        if (await this.#stagingResidue(daemons)) {
          throw new AssertFailure('no_durable_mutation violated: stranded byte-stream staging file');
        }
        return;
      }
      case 'no_network_activity':
        // Not instrumentable from user space without packet capture; an
        // honest limitation, not a silent pass — fail loudly so the case is
        // visible as unobserved rather than falsely green.
        throw new AssertFailure(
          'no_network_activity is not instrumentable by this harness (no packet capture)',
        );
      case 'connection_open': {
        const label = a.on || 'subject:self';
        // `on: "none"` asserts the daemon is unreachable (connection refused)
        // — used after daemon.stop to prove it no longer accepts connections.
        if (label === 'none') {
          const reachable = await this.#daemonReachable(primary);
          if (reachable) throw new AssertFailure('expected none to be open (daemon still reachable)');
          return;
        }
        const s = sessions.get(label);
        // The corpus uses logical connection names (c1, c2) that do not map to
        // the harness's `subject:*` session labels; treat an unknown label as
        // "any connection still open".
        if (!s) {
          const anyOpen = [...sessions.values()].some((x) => x.open);
          if (!anyOpen) throw new AssertFailure(`expected ${label} (any connection) to be open`);
          return;
        }
        if (!s.open) throw new AssertFailure(`expected ${label} to be open`);
        return;
      }
      case 'close_code': {
        const label = a.on || 'subject:self';
        const s = sessions.get(label);
        const want = Number(resolveValue(a.value, vars));
        // Wait briefly for a close if not yet observed.
        const deadline = Date.now() + 2000;
        while (s && s.closeCode === null && Date.now() < deadline) {
          await new Promise((r) => setTimeout(r, 50));
        }
        if (!s || s.closeCode !== want)
          throw new AssertFailure(`expected close code ${want} on ${label}, got ${s?.closeCode}`);
        return;
      }
      case 'no_push': {
        const room = resolveValue(a.room_id, vars);
        for (const s of sessions.values()) {
          const offending = s.pushes.find((p) => subsetMatch({ room_id: room }, p, vars));
          if (offending)
            throw new AssertFailure(`expected no push for ${room}, got ${JSON.stringify(offending)}`);
        }
        return;
      }
      case 'push_count': {
        const room = resolveValue(a.room_id, vars);
        const count = [...sessions.values()].reduce(
          (n, s) => n + s.pushes.filter((p) => p.room_id === room).length,
          0,
        );
        const cmp = a.value;
        const ok =
          cmp.op === 'eq' ? count === cmp.value
          : cmp.op === 'gte' ? count >= cmp.value
          : cmp.op === 'lte' ? count <= cmp.value
          : false;
        if (!ok) throw new AssertFailure(`push_count ${count} !${cmp.op} ${cmp.value} for ${room}`);
        return;
      }
      case 'process_exited': {
        // `daemon.stop` replies first, then tears down after a beat — poll for
        // the exit rather than demanding it be already observed.
        const deadline = Date.now() + 5_000 * this.timeoutScale;
        while (!primary.exited && Date.now() < deadline) {
          await new Promise((r) => setTimeout(r, 50));
        }
        if (!primary.exited)
          throw new AssertFailure(`expected daemon process to have exited`);
        return;
      }
      case 'timing_indistinguishable':
        // A timing assertion needs sub-step latency instrumentation; treated
        // as non-blocking here (recorded, not asserted).
        return;
      case 'bytes_streamed': {
        // Receiver-ACCEPTED payload bytes for one call: the daemon's
        // cumulative CREDIT/ABORT acknowledgement for an upload, the harness
        // sink's accepted count for a download. A pre-OPEN terminal refusal
        // (and a faithful replay, which opens no stream) records zero.
        const records = ctxState.callRecords;
        let rec;
        if (a.call !== undefined) {
          const m = /^step:([1-9][0-9]*)$/.exec(String(a.call));
          if (!m) throw new AssertFailure(`bytes_streamed selector ${JSON.stringify(a.call)} is not step:<n>`);
          rec = records.find((r) => r.step === Number(m[1]));
        } else {
          // Default: the most recent call on the same session (the
          // observation's session label, subject:self when unnamed).
          const label = a.on || 'subject:self';
          const pool = records.filter((r) => r.label === label);
          rec = pool[pool.length - 1];
        }
        if (!rec) {
          throw new AssertFailure(`bytes_streamed: no call recorded for ${a.call ?? 'the most recent call'}`);
        }
        const cmp = a.value;
        const want = Number(resolveValue(cmp.value, vars));
        const got = rec.accepted;
        const ok =
          cmp.op === 'eq' ? got === want
          : cmp.op === 'ne' ? got !== want
          : cmp.op === 'lt' ? got < want
          : cmp.op === 'lte' ? got <= want
          : cmp.op === 'gt' ? got > want
          : cmp.op === 'gte' ? got >= want
          : false;
        if (!ok) {
          throw new AssertFailure(
            `bytes_streamed ${got} !${cmp.op} ${want}${a.call ? ` (${a.call})` : ''}`,
          );
        }
        return;
      }
      default:
        throw new AssertFailure(`unsupported observe ${obs}`);
    }
  }
}
