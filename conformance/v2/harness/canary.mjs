// The run-level RealNetwork canary (175c0b).
//
// When a selected case uses `link:up`, the daemons run WITHOUT `--loopback`
// (RealNetwork mode: the invited join dials the minter via discovery, which
// loopback mode cannot do). Before any such case runs — before the whole
// case list, in fact — this bounded, disposable, two-daemon canary proves
// the RealNetwork path itself works on this host: distinct subjects, a room
// created and activated, a minted invite redeemed cross-daemon, the joiner
// timeline converged, and total cleanup.
//
// A canary failure is a HARNESS SETUP ERROR (never an assertion FAIL, never
// a fallback to loopback): the run aborts before the scenarios, naming the
// step that failed. The canary is also never a substitute for the per-case
// link witness — every link:up case re-verifies its own staged topology
// before its authored steps (runner.mjs #verifyLinkTopology).
//
// The witness deliberately avoids room.peers: provider/consumer reachability
// observability is #50's oracle (175c1a), and using it here would pre-empt
// that scenario's evidence. Convergence is observed where the corpus's own
// cases observe it: the timeline the joiner reads.

import { startDaemon } from './daemon.mjs';
import { Session } from './session.mjs';
import { pollUntil } from './topology.mjs';

/** How long the canary waits for the redeem dial and timeline convergence. */
export const CANARY_DEADLINE_MS = 45_000;

async function connect(daemon, label, clientId) {
  const s = new Session(label, clientId);
  await s.connect(daemon, { v: 2, sg: daemon.storageGeneration, token: daemon.token }, {});
  await s.awaitFrame((f) => f.t === 'hello');
  return s;
}

/**
 * Run the canary. Returns timing witnesses on success; throws on any step.
 * `onLog` receives progress lines (main.mjs prints them; tests stay quiet).
 */
export async function runRealNetworkCanary(binary, { timeoutScale = 1, deadlineMs, onLog = () => {} } = {}) {
  const deadline = (deadlineMs ?? CANARY_DEADLINE_MS) * timeoutScale;
  const t0 = Date.now();
  const daemons = [];
  const log = (m) => onLog(`[realnetwork-canary +${Date.now() - t0}ms] ${m}`);
  let sa = null;
  let sb = null;
  try {
    log('spawning two RealNetwork daemons (no --loopback)');
    const a = await startDaemon(binary, { loopback: false });
    daemons.push(a);
    const b = await startDaemon(binary, { loopback: false });
    daemons.push(b);
    log(`daemons up (A :${a.port}, B :${b.port})`);

    const sessA = await connect(a, 'canary-A', 'cf-canary-A');
    const sessB = await connect(b, 'canary-B', 'cf-canary-B');
    const ea = await sessA.call('subject.ensure', {});
    const eb = await sessB.call('subject.ensure', {});
    sa = ea.out?.subject_id;
    sb = eb.out?.subject_id;
    if (!sa || !sb || sa === sb) {
      throw new Error(`canary subjects not distinct: A=${sa} B=${sb}`);
    }

    const room = await sessA.call('room.create', { name: 'realnetwork-canary' });
    const rid = room.out?.room_id;
    if (!rid) throw new Error(`canary room.create failed: ${JSON.stringify(room.err)}`);
    await sessA.call('room.activate', { room_id: rid });

    const mint = await sessA.call('invite.mint', {
      room_id: rid,
      subject_id: sb,
      role: 'member',
      expires_at: new Date(Date.now() + 3600_000).toISOString().replace(/\.\d+Z$/, 'Z'),
    });
    if (!mint.ok) throw new Error(`canary invite.mint failed: ${JSON.stringify(mint.err)}`);

    // The redeem is the discovery dial: the joiner finds the minter with no
    // address hint through the ticket. This is the step loopback mode cannot
    // do, so it gets the bounded wait — with the CONFIGURED deadline as the
    // call timeout (Session.call defaults to 10 s; a healthy-but-slow relay
    // between 10 s and the deadline would otherwise abort the run early —
    // review-caught on PR #312).
    const tRedeem = Date.now();
    const redeem = await sessB.call('invite.redeem', { capability: mint.out.capability }, {
      timeoutMs: deadline,
    });
    if (!redeem.ok) {
      throw new Error(
        `canary invite.redeem failed after ${Date.now() - tRedeem}ms: ${JSON.stringify(redeem.err)}`,
      );
    }
    const redeemMs = Date.now() - tRedeem;
    log(`invite redeemed cross-daemon in ${redeemMs}ms`);

    const activate = await sessB.call('room.activate', { room_id: rid });
    if (!activate.ok) {
      throw new Error(`canary joiner room.activate failed: ${JSON.stringify(activate.err)}`);
    }

    // Joiner timeline: the joiner's forward timeline must show an event
    // authored by the AUTHORITY — cross-daemon sync through the live room
    // session, the same evidence a late-join case reads.
    const tConv = Date.now();
    await pollUntil({
      deadlineMs: deadline,
      describe: 'joiner timeline shows an authority-authored event',
      probe: async () => {
        const tl = await sessB.call('room.timeline', {
          room_id: rid, cursor: { state: 'start' }, direction: 'forward', limit: 100,
        });
        const events = tl.out?.events || [];
        const ok = events.some((e) => e.author?.subject_id === sa);
        return { done: ok, detail: `${events.length} events, authority-authored=${ok}` };
      },
    });
    const convergeMs = Date.now() - tConv;
    log(`joiner timeline converged in ${convergeMs}ms`);

    // Authority side: the authority's own timeline must show the join — the
    // membership is genuinely bidirectionally synced, not a local illusion.
    await pollUntil({
      deadlineMs: deadline,
      describe: 'authority timeline shows the member_joined event',
      probe: async () => {
        const tl = await sessA.call('room.timeline', {
          room_id: rid, cursor: { state: 'start' }, direction: 'forward', limit: 100,
        });
        const events = tl.out?.events || [];
        const ok = events.some((e) => e.kind === 'member_joined');
        return { done: ok, detail: events.map((e) => e.kind).join(',') || 'none' };
      },
    });

    return { redeemMs, convergeMs, aPort: a.port, bPort: b.port, totalMs: Date.now() - t0 };
  } finally {
    // Total cleanup: both daemons stopped gracefully and their temp data
    // dirs removed. Cleanup failures are loud — a leaked canary daemon would
    // contaminate the case run that follows.
    for (const d of daemons) await d.stop();
    log(`cleanup complete (${daemons.length} daemons stopped, data dirs removed)`);
  }
}
