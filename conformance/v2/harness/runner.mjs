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
import { Session, isPaddedEnvelope, buildPaddedEnvelope, isBareEnvelope } from './session.mjs';
import { AssertContext, AssertFailure, TransportFailure, evalAssert, subsetMatch } from './assert.mjs';
import { resolvePath, resolveValue } from './values.mjs';
import { CallStreamTracker, maxDataPayloadBytes, runStreamingCall, streamWaitMs, isTransportDroppedMarker } from './stream.mjs';
import { CONTROL_CAPABILITIES, unimplementedRequire } from './capabilities.mjs';
import { resolveActor, planCaseTopology, CaseTopology, pollUntil } from './topology.mjs';

/** The outcome of one case. */
export const Outcome = {
  PASS: 'pass',
  FAIL: 'fail',
  BLOCKED_PASS: 'blocked-pass', // blocked_on_upstream but it passed (surprise)
  BLOCKED_FAIL: 'blocked-fail', // blocked_on_upstream and it failed (expected)
  ERROR: 'error', // harness/setup error, not an assertion
};

let opIdCounter = 1;

/** Bounded wait for cross-daemon timeline convergence (the link witness).
 * Generous against the observed ~2.5s redeem dial: a deadline that expires
 * is a harness setup ERROR, so it must never false-red a healthy topology
 * under CI load. */
const LINK_WITNESS_DEADLINE_MS = 45_000;

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
      // Raw-send write aborts, recorded not thrown (see #runStep's send
      // branch and the case-end honesty guard).
      sendWriteErrors: [],
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

      // Send-write honesty guard: a raw `send` whose write was aborted may
      // only end green when a LATER step observed the connection state
      // (close_code / connection_open / process_exited). Otherwise a failed
      // probe would read as a passed one.
      for (const sendErr of ctxState.sendWriteErrors) {
        const observed = fixture.steps
          .slice(sendErr.step)
          .some((later) => Array.isArray(later?.assert) && later.assert.some((a) =>
            a && (a.observe === 'close_code' || a.observe === 'connection_open'
              || a.observe === 'process_exited')));
        if (!observed) {
          throw new AssertFailure(
            `step ${sendErr.step}'s raw send failed to write (${sendErr.message}) and no later step observed the connection state — the probe's outcome is unobserved`,
          );
        }
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
    // Requires honesty, BEFORE any staging: a well-formed token this runner
    // does not establish must fail the case loudly, never run half-staged.
    // The checker owns token SHAPE (its vocabulary is closed); this guard
    // owns token EXECUTION. It runs before the first daemon spawns so a
    // refused case costs no process and cannot leave partial state.
    const unimplemented = unimplementedRequire(requires);
    if (unimplemented) throw unimplemented;
    // Actor honesty, also pre-spawn: every label the case uses must be a
    // declared catalog role (four-axis resolution; no regex, no guessing).
    this.#validateActorLabels(fixture);

    // Topology plan: which slots the requires stage, and whether the
    // daemons must run RealNetwork (cross-daemon discovery). Non-link cases
    // stay loopback — there is NO fallback either way.
    const plan = planCaseTopology(requires);
    const topology = new CaseTopology(fixture._file || '');
    vars.__case = { topology, domain: fixture._file || '', primary: null, closers: [], joins: [] };

    // Every case runs against at least one daemon (the authority slot).
    const primary = await startDaemon(this.binary, { loopback: !plan.realNetwork });
    daemons.push(primary);
    topology.setDaemon('authority', primary);
    vars.__case.primary = primary;
    vars['daemon.storage_generation'] = primary.storageGeneration;
    vars.daemon_sg = primary.storageGeneration;
    vars['daemon'] = { storage_generation: primary.storageGeneration };

    // The other planned slots: one daemon per distinct subject (a daemon
    // holds exactly one).
    for (const slot of plan.slots) {
      if (slot === 'authority') continue;
      const d = await startDaemon(this.binary, { loopback: !plan.realNetwork });
      daemons.push(d);
      topology.setDaemon(slot, d);
    }

    // Establish the subjects, room, and members the case names. `subject`
    // (bare) means the authority subject exists; `room:live`/`plain` means a
    // room exists (`$rid`) and is live for `live`; `member:b`/`member:c`
    // stage a REAL additional member on its own daemon: subject.ensure,
    // invite.mint by the authority, invite.redeem over the RealNetwork link,
    // room.activate — the joiner timeline.
    const wantsRoom = requires.some((r) => r === 'room:plain' || r === 'room:live');
    const wantsLeftRoom = requires.includes('room:left');
    const wantsMembers = requires.includes('member:b') || requires.includes('member:c');
    const roomLive = requires.includes('room:live');

    if (wantsRoom || wantsLeftRoom || wantsMembers) {
      // The authority's session and subject ($sa / $self_sid).
      const authority = await this.#sessionFor('subject:authority', daemons, sessions, vars);
      const authHello = await authority.call('subject.ensure', {});
      vars.self_sid = authHello.out?.subject_id;
      vars.sa = vars.self_sid;

      const created = await authority.call('room.create', { name: 'conformance' });
      vars.rid = created.out?.room_id;
      if (roomLive) {
        await authority.call('room.activate', { room_id: vars.rid });
      }

      // Additional members, each on its own daemon: a real invite flow, not
      // a same-daemon alias. `member:b` → the member slot ($sd), `member:c`
      // → the member2 slot.
      if (requires.includes('member:b')) {
        await this.#stageMemberJoin({
          slot: 'member', label: 'subject:member_b', roomVar: 'rid', activate: roomLive,
          bind: { sd: true, member_b_sid: true }, daemons, sessions, vars,
        });
      }
      if (requires.includes('member:c')) {
        await this.#stageMemberJoin({
          slot: 'member2', label: 'subject:member_c', roomVar: 'rid', activate: roomLive,
          bind: { member_c_sid: true }, daemons, sessions, vars,
        });
      }

      // A `room:left` room is created, joined by the member, and LEFT by
      // them — a departed member, not an empty room id.
      if (wantsLeftRoom) {
        const left = await authority.call('room.create', { name: 'left room' });
        vars.rid_left = left.out?.room_id;
        await this.#stageMemberJoin({
          slot: 'member', label: 'subject:member_b', roomVar: 'rid_left', activate: true,
          bind: {}, depart: true, daemons, sessions, vars,
        });
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
        if (!reply.ok || !reply.out?.file_id) {
          throw new Error(
            `resource:shared_file setup failed: ${JSON.stringify(reply.ok ? reply.out : reply.err)}`,
          );
        }
        vars.fid = reply.out.file_id;
      }
    }

    // `link:up` with no other staged peer: the member slot is the link peer
    // (the files-domain provider/consumer shape) and joins the room so the
    // link is genuinely UP, not merely dialable. Cases that already staged a
    // second subject use it as the peer.
    if (plan.linkUp && plan.slots.includes('member')
      && !requires.includes('member:b') && !requires.includes('room:left')) {
      if (!vars.rid) {
        // No room to join: the peer daemon exists as a RealNetwork endpoint
        // only. (resource:* tokens that would stage the room themselves stay
        // unimplemented; this shape is recorded, not silently skipped.)
        this.log('link:up peer daemon staged without a room join (no room require present)');
      } else {
        await this.#stageMemberJoin({
          slot: 'member', label: 'subject:B', roomVar: 'rid', activate: roomLive,
          bind: { sd: true }, daemons, sessions, vars,
        });
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
      vars.sa = vars.self_sid;
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
    // real ensured subject on its own daemon — one daemon, one subject, so
    // these ids are genuinely distinct from the authority's. `$sb` is the
    // invitee (the subject a capability is bound to; the invitee slot stages
    // with `daemon:second` or `subject:second`), `$sc` the outsider (a
    // subject with no room relationship, staged by `subject:outsider`). A
    // case that stages neither slot never sees a var it did not ask for.
    const ensureSlotSubject = async (label, varName) => {
      const sess = await this.#sessionFor(label, daemons, sessions, vars);
      const ensured = await sess.call('subject.ensure', {});
      vars[varName] = ensured.out?.subject_id;
    };
    if (vars.__case.topology.daemonFor('invitee')) {
      await ensureSlotSubject('subject:second', 'sb');
    }
    if (vars.__case.topology.daemonFor('outsider')) {
      await ensureSlotSubject('subject:outsider', 'sc');
    }

    // The per-case link witness: a RealNetwork case re-verifies its staged
    // topology BEFORE the authored steps (the run-level canary is never a
    // substitute). Joins converge on the timeline; a case whose link:up
    // staged no join (the invites shape: the case authors its own redeem)
    // witnesses the staged sessions are live.
    if (plan.realNetwork) {
      await this.#verifyLinkTopology({ plan, vars, sessions });
    }
  }

  /**
   * Stage one additional member on its own daemon: subject.ensure, mint by
   * the authority, redeem over the link, activate (when the room is live),
   * and optionally depart (room:left). Records the join for the witness.
   *
   * The redeem DIALS the authority's live room session — an invite into a
   * room the authority never serves cannot be redeemed (verified live: the
   * join times out with no reply). The staging therefore activates the
   * target room on the authority before minting (idempotent — the corpus
   * itself pins `room_activate_is_naturally_idempotent`), so a case that
   * wants a non-live room authors its OWN room.deactivate afterwards
   * (room_peers_on_a_room_that_is_not_live does exactly that).
   */
  async #stageMemberJoin({ slot, label, roomVar, activate, bind, depart = false, daemons, sessions, vars }) {
    const authority = await this.#sessionFor('subject:authority', daemons, sessions, vars);
    const sess = await this.#sessionFor(label, daemons, sessions, vars);
    const ensured = await sess.call('subject.ensure', {});
    const sid = ensured.out?.subject_id;
    if (!sid) throw new Error(`${label} staging: subject.ensure failed: ${JSON.stringify(ensured.err)}`);
    if (bind?.sd) vars.sd = sid;
    if (bind?.member_b_sid) vars.member_b_sid = sid;
    if (bind?.member_c_sid) vars.member_c_sid = sid;
    const liveUp = await authority.call('room.activate', { room_id: vars[roomVar] });
    if (!liveUp.ok) {
      throw new Error(`${label} staging: authority room.activate failed: ${JSON.stringify(liveUp.err)}`);
    }
    const mint = await authority.call('invite.mint', {
      room_id: vars[roomVar],
      subject_id: sid,
      role: 'member',
      expires_at: new Date(Date.now() + 3600_000).toISOString().replace(/\.\d+Z$/, 'Z'),
    });
    if (!mint.ok || !mint.out?.capability) {
      throw new Error(`${label} staging: invite.mint failed: ${JSON.stringify(mint.err)}`);
    }
    const redeem = await sess.call('invite.redeem', { capability: mint.out.capability });
    if (!redeem.ok) {
      throw new Error(
        `${label} staging: invite.redeem failed (the cross-daemon dial): ${JSON.stringify(redeem.err)}`,
      );
    }
    if (activate) {
      const act = await sess.call('room.activate', { room_id: vars[roomVar] });
      if (!act.ok) {
        throw new Error(`${label} staging: room.activate failed: ${JSON.stringify(act.err)}`);
      }
    }
    if (depart) {
      const left = await sess.call('room.leave', { room_id: vars[roomVar] });
      if (!left.ok) {
        throw new Error(`${label} staging: room.leave failed: ${JSON.stringify(left.err)}`);
      }
      return;
    }
    vars.__case.joins.push({ slot, label, roomVar });
  }

  /**
   * The per-case link witness: repeats the actual topology checks before the
   * authored steps. Every staged join must be visible on both ends — the
   * joiner's timeline shows authority-authored events (cross-daemon sync),
   * and the authority's timeline shows the member's join. A link:up case
   * with no staged join (its steps author the cross-daemon work themselves)
   * witnesses the staged sessions are still open. Never uses room.peers:
   * provider/consumer reachability observability is #50's oracle.
   */
  async #verifyLinkTopology({ plan, vars, sessions }) {
    const deadlineMs = LINK_WITNESS_DEADLINE_MS * this.timeoutScale;
    const authority = sessions.get(vars.__case.topology.canonicalLabelFor('authority'));
    if (!authority || !authority.open) {
      throw new Error('link witness: the authority session is not open');
    }
    for (const join of vars.__case.joins) {
      // Resolve the join's session exactly as #sessionFor keys it: canonical
      // family labels (subject:B / subject:member_b) share the member slot's
      // ONE canonical connection.
      const joinActor = resolveActor(vars.__case.domain, join.label);
      const joinLookup = joinActor.connectionKind === 'canonical'
        ? (vars.__case.topology.canonicalLabelFor(joinActor.slot) ?? join.label)
        : join.label;
      const sess = sessions.get(joinLookup);
      if (!sess || !sess.open) {
        throw new Error(`link witness: the staged join ${join.label} has no open session`);
      }
      const rid = vars[join.roomVar];
      const sa = vars.sa;
      await pollUntil({
        deadlineMs,
        describe: `joiner ${join.label} timeline shows authority-authored events for ${join.roomVar}`,
        probe: async () => {
          const tl = await sess.call('room.timeline', {
            room_id: rid, cursor: { state: 'start' }, direction: 'forward', limit: 100,
          });
          const events = tl.out?.events || [];
          const ok = events.some((e) => e.author?.subject_id === sa);
          return { done: ok, detail: `${events.length} events, authority-authored=${ok}` };
        },
      });
    }
    if (vars.__case.joins.length > 0) {
      const rids = [...new Set(vars.__case.joins.map((j) => vars[j.roomVar]))];
      for (const rid of rids) {
        await pollUntil({
          deadlineMs,
          describe: `authority timeline shows the member join for room`,
          probe: async () => {
            const tl = await authority.call('room.timeline', {
              room_id: rid, cursor: { state: 'start' }, direction: 'forward', limit: 100,
            });
            const events = tl.out?.events || [];
            const ok = events.some((e) => e.kind === 'member_joined');
            return { done: ok, detail: events.map((e) => e.kind).join(',') || 'none' };
          },
        });
      }
    } else if (plan.linkUp) {
      // No staged join: the case stages a second subject (the invites
      // shape) but never joins it to anything, so there is no join to
      // re-verify. Two open client sockets prove nothing about
      // daemon-to-daemon connectivity — the run-level canary runs on
      // DIFFERENT disposable daemons and is explicitly not a per-case
      // substitute. The witness therefore performs a REAL cross-daemon
      // round on the case's own daemons: a dedicated WITNESS room (never
      // the case's `$rid`, whose membership/positions the case asserts
      // on) that the link peer joins via mint/redeem — the same checks
      // the canary makes, per-case (review-caught on PR #312: the
      // sockets-open check could pass while the two case daemons could
      // not communicate).
      const topology = vars.__case.topology;
      const peerSlot = ['invitee', 'member', 'member2', 'outsider']
        .find((slot) => topology.daemonFor(slot));
      if (!peerSlot) {
        throw new Error('link witness: link:up with no join and no peer slot staged');
      }
      const peerLabel = topology.canonicalLabelFor(peerSlot);
      const peer = sessions.get(peerLabel);
      if (!peer || !peer.open) {
        throw new Error(`link witness: the ${peerSlot} peer session (${peerLabel}) is not open`);
      }
      const peerSid = peerSlot === 'invitee' ? vars.sb
        : peerSlot === 'outsider' ? vars.sc
        : peerSlot === 'member' ? (vars.sd ?? vars.member_b_sid)
        : vars.member_c_sid;
      if (!peerSid) {
        throw new Error(`link witness: no bound subject id for the ${peerSlot} peer slot`);
      }
      const witnessRoom = await authority.call('room.create', { name: 'link-witness' });
      if (!witnessRoom.ok) {
        throw new Error(`link witness: room.create failed: ${JSON.stringify(witnessRoom.err)}`);
      }
      const wrid = witnessRoom.out.room_id;
      const wAct = await authority.call('room.activate', { room_id: wrid });
      if (!wAct.ok) {
        throw new Error(`link witness: room.activate failed: ${JSON.stringify(wAct.err)}`);
      }
      const wMint = await authority.call('invite.mint', {
        room_id: wrid,
        subject_id: peerSid,
        role: 'member',
        expires_at: new Date(Date.now() + 3600_000).toISOString().replace(/\.\d+Z$/, 'Z'),
      });
      if (!wMint.ok || !wMint.out?.capability) {
        throw new Error(`link witness: invite.mint failed: ${JSON.stringify(wMint.err)}`);
      }
      const wRedeem = await peer.call('invite.redeem', { capability: wMint.out.capability }, { timeoutMs: deadlineMs });
      if (!wRedeem.ok) {
        throw new Error(
          `link witness: the peer could not redeem over the link (daemon-to-daemon discovery): ${JSON.stringify(wRedeem.err)}`,
        );
      }
      const wLive = await peer.call('room.activate', { room_id: wrid });
      if (!wLive.ok) {
        throw new Error(`link witness: peer room.activate failed: ${JSON.stringify(wLive.err)}`);
      }
      await pollUntil({
        deadlineMs,
        describe: `witness joiner timeline shows authority-authored events (cross-daemon link)`,
        probe: async () => {
          const tl = await peer.call('room.timeline', {
            room_id: wrid, cursor: { state: 'start' }, direction: 'forward', limit: 100,
          });
          const events = tl.out?.events || [];
          const ok = events.some((e) => e.author?.subject_id === vars.sa);
          return { done: ok, detail: `${events.length} events, authority-authored=${ok}` };
        },
      });
      await pollUntil({
        deadlineMs,
        describe: `authority timeline shows the witness member join`,
        probe: async () => {
          const tl = await authority.call('room.timeline', {
            room_id: wrid, cursor: { state: 'start' }, direction: 'forward', limit: 100,
          });
          const events = tl.out?.events || [];
          const ok = events.some((e) => e.kind === 'member_joined');
          return { done: ok, detail: events.map((e) => e.kind).join(',') || 'none' };
        },
      });
      this.log(`link:up witnessed via a dedicated witness room joined by ${peerLabel} (real cross-daemon round on the case's daemons)`);
    }
  }

  /** Pre-spawn actor-label validation: every label the case uses (step `on`,
   * control target, observe target) must be declared in the catalog. */
  #validateActorLabels(fixture) {
    const labels = new Set();
    for (const step of fixture.steps || []) {
      if (typeof step.on === 'string') labels.add(step.on);
      if (step.control && typeof step.control.on === 'string') labels.add(step.control.on);
      for (const a of step.assert || []) {
        if (a && typeof a.on === 'string') labels.add(a.on);
      }
    }
    labels.delete('none'); // observe-only: probes daemon unreachability
    for (const label of labels) {
      resolveActor(fixture._file || '', label); // throws ActorResolutionError
    }
  }

  /** The session for an `on` label, connecting lazily. Every step path
   * resolves its session here, so a sticky binary violation recorded on the
   * connection surfaces before the next interaction of any kind. */
  async #sessionFor(onLabel, daemons, sessions, vars) {
    const rawLabel = onLabel || 'subject:self';
    const topology = vars.__case?.topology;
    const actor = resolveActor(vars.__case?.domain || '', rawLabel);
    const daemon = topology?.daemonFor(actor.slot);
    if (!daemon) {
      // Missing role: the label is declared, but the case's requires staged
      // no daemon for its slot. A setup ERROR (never an assertion FAIL, and
      // never a silent default to the primary daemon).
      throw new Error(
        `missing role: actor label "${rawLabel}" resolves to topology slot "${actor.slot}", ` +
          `but the case's requires staged no daemon there — stage it with the matching require ` +
          `(member:b / member:c / subject:second / daemon:second / subject:outsider)`,
      );
    }
    // Canonical-family labels (subject:A / subject:authority / subject:self)
    // share the slot's ONE canonical connection; `same_principal` reconnects
    // replace it. Extra connections (`#N`) and own-principal labels get
    // their own session entries.
    const lookupLabel = actor.connectionKind === 'canonical'
      ? (topology.canonicalLabelFor(actor.slot) ?? rawLabel)
      : rawLabel;
    if (sessions.has(lookupLabel)) {
      const existing = sessions.get(lookupLabel);
      if (existing.stickyBinaryViolation) throw existing.stickyBinaryViolation;
      return existing;
    }
    // The case authored an upgrade for this label and the daemon refused it;
    // auto-connecting here would silently substitute a different admission.
    const refused = vars.__case?.refusedUpgrades?.[rawLabel]
      ?? vars.__case?.refusedUpgrades?.[lookupLabel];
    if (refused !== undefined) {
      throw new AssertFailure(
        `the authored upgrade for ${rawLabel} was refused (status ${refused}); refusing to substitute an auto-connected session`,
      );
    }
    const clientId = topology.principalIdFor(actor.slot, actor.principalKey, actor.principalKind);
    let registerAs = lookupLabel;
    if (actor.connectionKind === 'replace') {
      // `same_principal`: same cid, and the new connection REPLACES the
      // canonical one — the outgoing session is closed (and kept for the
      // case-end sticky-violation scan), the replacement registers under
      // this label, and later canonical-label steps resolve to it.
      const outgoingLabel = topology.canonicalLabelFor(actor.slot);
      const outgoing = outgoingLabel ? sessions.get(outgoingLabel) : undefined;
      if (outgoing) {
        if (outgoing.stickyBinaryViolation) throw outgoing.stickyBinaryViolation;
        outgoing.close();
        sessions.delete(outgoingLabel);
        (vars.__case.replacedSessions ||= []).push(outgoing);
      }
      topology.setCanonicalLabel(actor.slot, rawLabel);
      registerAs = rawLabel;
    }
    const s = new Session(rawLabel, clientId);
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
      throw new TransportFailure(`session ${rawLabel} failed before hello: ${err.message}`);
    }
    vars.frame = hello;
    vars.hello = hello;
    sessions.set(registerAs, s);
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
      let value = resolveValue(step.send, vars);
      // The padded-envelope form builds one frame whose total serialized
      // length hits an exact byte target (the max_frame_bytes boundary
      // fixtures); the bare-envelope form sends the envelope itself raw
      // (malformed-input fixtures). Resolve computed nodes first, then build.
      // A padded frame is delivered as a TRICKLED write: an oversize frame
      // is decided by the daemon from its header while the write is still
      // in flight, and a one-shot 128 MiB write races the TCP RST (provoked
      // by the daemon-side unread backlog) against our reading of the close
      // frame — the close loses often enough to flake CI. A write the
      // daemon aborts is recorded, not fatal: the case's later assertions
      // observe the reaction, and the case-end guard refuses a green
      // verdict when nothing observed the connection state after it.
      try {
        if (isPaddedEnvelope(value)) {
          value = buildPaddedEnvelope(value);
          this.log(`send padded frame (trickled): ${String(value).length} chars`);
          await s.sendPaddedTrickled(value);
        } else {
          if (isBareEnvelope(value)) {
            value = value.envelope;
            this.log(`send raw envelope id=${value?.id}`);
          }
          await s.sendRaw(value);
        }
      } catch (err) {
        env.ctxState.sendWriteErrors.push({ step: index + 1, message: err.message });
        this.log(`send write failed (recorded; later assertions must observe the reaction): ${err.message}`);
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
      if (isTransportDroppedMarker(reply)) {
        // A transport_drop step ends with NO terminal reply on the wire —
        // the socket is deliberately dropped pre-END. Treat the step as
        // reply-less: nothing is exposed under out/err/frame (there is no
        // reply to expose, and fabricating one is forbidden), the call
        // counts as not-ok for the refused-baseline semantics, and the
        // validator forbids expect/save on this shape so no matcher ever
        // runs. The reply roots are CLEARED, not left holding the previous
        // step's reply — a later assert or save-only step reading out/err/
        // frame must find nothing rather than silently match stale values
        // from before the drop. The tracker's accepted count (already
        // recorded) is what a later bytes_streamed observation reads.
        vars.out = undefined;
        vars.err = undefined;
        vars.frame = undefined;
        ctxState.lastCallOk = false;
        ctxState.networkActivity++;
        this.log(`  -> transport dropped (accepted ${record.accepted})`);
        return reply;
      }
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
        if (this.verbose) {
          let shown;
          try { shown = JSON.stringify(value)?.slice(0, 60); } catch { shown = '[unserializable]'; }
          this.log(`save ${varName} <- ${path} = ${shown} (found=${found})`);
        }
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
        concurrentLimit: hello.limits?.max_concurrent_transfers,
        stallMs: hello.limits?.transfer_stall_ms,
        allowanceMs: hello.limits?.transfer_connect_allowance_ms,
        floorBps: hello.limits?.transfer_floor_bits_per_second,
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
    const u = step.upgrade;
    // Resolve the actor FIRST: the upgrade targets the label's topology slot
    // (daemon), and its placeholders (<port>, <token>) substitute against
    // THAT daemon — never an implicit primary.
    const label = step.on || 'subject:self';
    const actor = resolveActor(vars.__case?.domain || '', label);
    const lookupLabel = actor.connectionKind === 'canonical'
      ? (vars.__case?.topology?.canonicalLabelFor(actor.slot) ?? label)
      : label;
    // A same-label upgrade replaces the registered session; a sticky
    // violation on the outgoing connection must not vanish with it.
    const outgoing = sessions.get(lookupLabel);
    if (outgoing && outgoing.stickyBinaryViolation) throw outgoing.stickyBinaryViolation;
    const daemon = vars.__case?.topology?.daemonFor(actor.slot);
    if (!daemon) {
      // No latent default-to-primary here either: an upgrade against a slot
      // the case never staged is a missing-role setup error, exactly like
      // #sessionFor. (No corpus upgrade names a non-authority label today;
      // this guard keeps it that way loudly.)
      throw new Error(
        `missing role: upgrade for "${label}" resolves to topology slot "${actor.slot}", ` +
          `but the case's requires staged no daemon there`,
      );
    }
    const query = {};
    for (const [k, v] of Object.entries(u.query || {})) {
      const resolved = resolveValue(v, vars);
      // Query values carry the same portfile placeholders headers do
      // (<daemon_sg>, <token>, …) — substitute them, or an authored admission
      // silently becomes a refusal.
      query[k] = typeof resolved === 'string' ? this.#resolveHeaderValue(resolved, daemon, vars) : resolved;
    }
    const headers = {};
    for (const [k, v] of Object.entries(u.headers || {})) {
      headers[k] = this.#resolveHeaderValue(v, daemon, vars);
    }
    this.log(`upgrade ${JSON.stringify(query)}`);

    // Perform the upgrade; it may be refused (non-101). A successful upgrade
    // becomes the `on` session's live connection so a following `await` reads
    // THIS connection's hello, not a stale one from before the upgrade.
    // A setup upgrade (no expect) is the label's ordinary admitted
    // connection, so it presents the label's STABLE principal exactly as
    // #sessionFor does — an omitted `cid` is an ephemeral per-connection
    // dedup principal on the daemon, which would make every later
    // same-principal reconnect (op_id replay cases) silently miss the ledger.
    // A probing upgrade (any expect) sends exactly what the fixture wrote.
    let clientId = null;
    if (!step.expect && query.cid === undefined && query.ct === undefined && vars.__case) {
      clientId = vars.__case.topology.principalIdFor(actor.slot, actor.principalKey, actor.principalKind);
      query.cid = clientId;
    }
    const result = await this.#attemptUpgrade(daemon, query, headers, label, sessions, clientId, lookupLabel);
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
    } else if (vars.__case) {
      // A refused no-expect upgrade must not silently degrade into
      // #sessionFor's auto-connection: mark the label, and the next step
      // that DEPENDS on this session fails with the refusal. Probe upgrades
      // and post-stop reachability checks (which never use the session
      // afterwards) stay expressible.
      vars.__case.refusedUpgrades ||= Object.create(null);
      if (result.status === 101) {
        delete vars.__case.refusedUpgrades[label];
      } else {
        vars.__case.refusedUpgrades[label] = result.status;
      }
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
      .replace('<daemon_sg>', String(daemon.storageGeneration))
      .replace('<port>', String(daemon.port))
      .replace(/\$([A-Za-z_][A-Za-z0-9_.]*)/g, (m, name) => {
        const { found, value } = resolvePath(vars, name);
        return found ? String(value) : m;
      });
  }

  /** Attempt a WS upgrade, returning {status, body, frame}. On admission the
   * socket stays open and is registered as the `label` session's connection. */
  async #attemptUpgrade(daemon, query, headers, label, sessions, clientId = null, registerAs = null) {
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
          if (sessions) sessions.set(registerAs ?? label, session);
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

  /** An `http` step: a Layer 0 or /api/session request. An http step
   * carries no actor label — it targets the case's daemon, which IS the
   * authority slot's (the primary), resolved through the topology rather
   * than a bare field read so no unstaged-slot path can exist here. */
  async #doHttp(step, env) {
    const { vars } = env;
    const daemon = vars.__case?.topology?.daemonFor('authority');
    if (!daemon) throw new Error('http step: no authority daemon staged for this case');
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
        // The corpus roots http captures at `out` (and sometimes `body`);
        // both alias the response body.
        const { found, value } = resolvePath({ body, out: body, ...body }, p);
        vars[varName] = found ? value : undefined;
        if (this.verbose) {
          let shown;
          try { shown = JSON.stringify(value)?.slice(0, 60); } catch { shown = '[unserializable]'; }
          this.log(`save ${varName} <- ${p} = ${shown} (found=${found})`);
        }
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
    // Capability honesty: a verb the DSL documents but this harness does not
    // execute fails loudly here — never a silent no-op. The classification
    // lives in capabilities.mjs and is shared with the checker and README.
    if (CONTROL_CAPABILITIES[c?.do] === 'documented_only') {
      throw new Error(
        `control ${c.do} is documented in the DSL but not executed by this harness ` +
          `(capabilities.mjs classifies it documented_only) — the case cannot run honestly`,
      );
    }
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
        // Close whatever connection currently serves the label's canonical
        // family (subject:A and subject:self are one actor), then reconnect
        // presenting the SAME principal id — the per-(slot, principal) map
        // survives, so an op_id replay after reconnect hits the same dedup
        // ledger.
        const actor = resolveActor(vars.__case?.domain || '', label);
        const lookupLabel = actor.connectionKind === 'canonical'
          ? (vars.__case?.topology?.canonicalLabelFor(actor.slot) ?? label)
          : label;
        const s = sessions.get(lookupLabel);
        if (s) {
          s.close();
          sessions.delete(lookupLabel);
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
      default:
        // Unreachable for classified verbs: every documented_only verb is
        // refused before the switch, so this names a verb outside the
        // checker's closed set reaching the runner anyway.
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
      // EVERY staged daemon's store, not just the primary's: since 175c0b a
      // member runs on its OWN daemon, so a no_durable_mutation observation
      // after a member operation must watch the store the operation ran on —
      // an illegal mutation or stranded file on a secondary daemon would
      // otherwise pass unnoticed (review-caught on PR #312).
      return JSON.stringify(daemons.map((d) => walk(d.dataDir)));
    } catch {
      return null;
    }
  }

  /** Whether the staging directory still holds any file, settled: cleanup is
   * asynchronous, so residue is only a violation if it persists past a
   * bounded wait. Checked on EVERY staged daemon, for the same reason as
   * #dirStateSignature. */
  async #stagingResidue(daemons) {
    const residueAnywhere = () => daemons.some((d) => {
      const dir = join(d.dataDir, 'protocol-v2-stream-staging');
      try { return readdirSync(dir).length > 0; } catch { return false; }
    });
    const deadline = Date.now() + 2_000 * this.timeoutScale;
    while (residueAnywhere()) {
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
        // Named connections resolve through the actor catalog. The session
        // KEY follows how #sessionFor registers connections: canonical-family
        // labels share the slot's canonical connection, an observe alias
        // (`c1` → subject:self#2) resolves onto the label that OPENED it
        // (the alias is never a step label, so its key is the alias target),
        // and every other extra/own connection registers under its own raw
        // label. An undeclared label is an error — the old "unknown label
        // means any connection" fallback could pass on a connection the
        // fixture never named. Observing never OPENS a connection: asserting
        // one that was never opened is a failure.
        const actor = resolveActor(vars.__case?.domain || '', label);
        const lookupLabel = actor.connectionKind === 'canonical'
          ? (vars.__case?.topology?.canonicalLabelFor(actor.slot) ?? label)
          : (actor.aliasOf ?? label);
        const s = sessions.get(lookupLabel);
        if (!s) {
          throw new AssertFailure(`expected ${label} to be open, but no session was ever opened for it`);
        }
        if (!s.open) throw new AssertFailure(`expected ${label} to be open`);
        return;
      }
      case 'close_code': {
        const label = a.on || 'subject:self';
        // Canonical-family labels (subject:A / subject:self) observe the one
        // canonical connection; observe aliases (c1) resolve onto the label
        // that opened the connection (the same key rule as connection_open).
        const closeActor = resolveActor(vars.__case?.domain || '', label);
        const closeLookup = closeActor.connectionKind === 'canonical'
          ? (vars.__case?.topology?.canonicalLabelFor(closeActor.slot) ?? label)
          : (closeActor.aliasOf ?? label);
        const s = sessions.get(closeLookup);
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
