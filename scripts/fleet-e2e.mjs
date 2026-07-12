#!/usr/bin/env node
// End-to-end proof of Jeliya AGENT-FLEET orchestration (echo workers,
// loopback only — no LLM, no network beyond 127.0.0.1). Node 22+, no npm deps.
// Reuses the realnet-lib plumbing (Client / startRealDaemon / sleep).
//
// Topology (ports 7482-7487):
//   - human daemon : 7482, spawned here (loopback) — creates & owns the room
//   - agent 1      : 7483, its own daemon+data-dir, echo worker runner
//   - agent 2      : 7484, its own daemon+data-dir, echo worker runner
//
// Both agents are pre-joined as role=agent members before workload content (a
// temporary daemon per agent does identity.create + room.join, then dies), and
// are then run in rejoin mode (--room). Late joins after content are supported,
// but staging membership first removes transient bootstrap timing variance while
// each runner immediately announces with agent_status. Pre-join while quiet,
// rejoin to serve.
//
// HARD assertions (any failure → teardown + exit 1):
//   1. two echo-worker runners BOTH join the one room and both show as active
//      role=agent members.
//   2. COLLISION: N trials of a single "@agent build the thing" trigger; the
//      claim protocol must let EXACTLY ONE agent execute — proven by exactly one
//      distinct "done"-status author per trial (echo output is byte-identical
//      across agents, so file_ids dedup and cannot count executors; the done
//      status, authored per-identity, is the honest executor signal). Worst case
//      (max executors in any trial) is reported.
//   3. ADDRESSED: "@agent:<prefix-of-agent-2> ping" — only agent 2 executes;
//      agent 1 provably receives it and ignores it (logs the addressed-ignore
//      line, emits no done).
//   4. FLEET API: agents.fleet → total=2, active/working/room-coverage match the
//      real moment; agent.history for one agent → points equal that agent's real
//      agent_status events in the room.
//   5. LIVENESS / stale-working fix: put agent 1 into a real "working" status
//      (posted through its OWN daemon, so it is a genuine signed event by that
//      identity), SIGKILL the runner + its orphaned daemon, then assert
//      agents.fleet reports that agent OFFLINE/STALE — never a live "working"
//      badge for a dead process.
//
// Usage: node scripts/fleet-e2e.mjs [--trials 5] [--scratch <dir>]

import { execFileSync } from "node:child_process";
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { Client, parseArgs, sleep, startRealDaemon } from "./realnet-lib.mjs";
import { recordOwnedProcess, signalOwnedProcessGroup } from "./e2e-process-ownership.mjs";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const AGENT_SCRIPT = join(repoRoot, "scripts", "jeliya-agent.mjs");

const PORT_HUMAN = 7482;
const PORT_AGENT1 = 7483;
const PORT_AGENT2 = 7484;
const TRIGGER = "@agent";

const args = parseArgs(process.argv.slice(2));
const TRIALS = Number.isInteger(Number(args.trials)) && Number(args.trials) > 0 ? Number(args.trials) : 5;
const SCRATCH = typeof args.scratch === "string"
  ? resolve(args.scratch)
  : mkdtempSync(join(tmpdir(), "jeliya-fleet-e2e-"));

// The double-run guard: after the first executor finishes a trial, wait this
// long for a would-be SECOND executor to also surface before counting.
const DOUBLE_GUARD_MS = 8_000;

// ---------------------------------------------------------------------------
// Teardown discipline — everything spawned is registered and reaped on ANY
// exit. Runner daemon listener PIDs are matched to jeliyad + the run-owned data
// dirs before they may be killed; unrelated fixed-port listeners fail closed.
// ---------------------------------------------------------------------------
const daemons = [];
const clients = [];
const runners = []; // { proc, exited }
const scratchDirs = [];
const ownedPorts = new Set();
let tearingDown = false;
const cleanupErrors = [];

function portListenerPids(port) {
  try {
    const out = execFileSync(
      "lsof",
      ["-nP", `-iTCP:${port}`, "-sTCP:LISTEN", "-t"],
      { encoding: "utf8" },
    );
    return [...new Set(out.split("\n")
      .filter(Boolean)
      .map(Number)
      .filter((pid) => Number.isInteger(pid) && pid > 0 && pid !== process.pid))];
  } catch (error) {
    // lsof exits nonzero when nothing listens, which is expected. A missing
    // binary would make cleanup best-effort and can create false/flaky passes.
    if (error?.code === "ENOENT") {
      throw new Error("fleet E2E requires lsof for deterministic cleanup");
    }
    if (error?.status === 1 && !String(error?.stdout ?? "").trim()) return [];
    throw new Error(`fleet E2E could not inspect port ${port} with lsof`);
  }
}

function assertPortAvailable(port) {
  const listeners = portListenerPids(port);
  if (listeners.length > 0) {
    throw new Error(
      `fleet E2E port ${port} is already in use; refusing to terminate an unowned process`,
    );
  }
}

function ownedDaemonListenerPid(port, dataDir) {
  const listeners = portListenerPids(port);
  if (listeners.length === 0) return null;
  if (listeners.length !== 1) {
    throw new Error(`fleet E2E expected one listener on port ${port}, found ${listeners.length}`);
  }
  const pid = listeners[0];
  const command = execFileSync("ps", ["-ww", "-o", "command=", "-p", String(pid)], {
    encoding: "utf8",
  }).trim();
  if (!command.includes("jeliyad") || !command.includes(resolve(dataDir))) {
    throw new Error(
      `fleet E2E port ${port} was claimed by a process that is not the run-owned daemon`,
    );
  }
  return pid;
}

function waitForPortsReleased(ports, timeoutMs = 5_000) {
  const sleeper = new Int32Array(new SharedArrayBuffer(4));
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const occupied = ports.filter((port) => portListenerPids(port).length > 0);
    if (occupied.length === 0) return [];
    if (Date.now() >= deadline) return occupied;
    Atomics.wait(sleeper, 0, 0, 100);
  }
}

function teardown() {
  if (tearingDown) return cleanupErrors;
  tearingDown = true;
  for (const c of clients) {
    try {
      c.close();
    } catch {}
  }
  for (const r of runners) {
    if (r.group) {
      try {
        signalOwnedProcessGroup(r.group, "SIGKILL");
      } catch (error) {
        cleanupErrors.push(error.message);
      }
    } else if (!r.exited) {
      try {
        if (!r.proc.kill("SIGKILL")) {
          cleanupErrors.push(`could not signal unregistered run-owned runner ${r.proc.pid}`);
        }
      } catch (error) {
        cleanupErrors.push(error.message);
      }
    }
  }
  for (const d of daemons) {
    try {
      if (d.exitCode === null && d.signalCode === null && !d.kill("SIGKILL")) {
        cleanupErrors.push(`could not signal run-owned daemon ${d.pid}`);
      }
    } catch (error) {
      cleanupErrors.push(`could not signal run-owned daemon ${d.pid}: ${error?.code ?? error}`);
    }
  }
  try {
    const occupied = waitForPortsReleased([...ownedPorts]);
    if (occupied.length > 0) {
      cleanupErrors.push(`run-owned ports did not close: ${occupied.join(", ")}`);
    }
  } catch (error) {
    cleanupErrors.push(`could not verify port release: ${error.message}`);
  }
  if (cleanupErrors.length === 0) {
    for (const dir of scratchDirs) {
      try {
        rmSync(dir, { recursive: true, force: true });
      } catch (error) {
        cleanupErrors.push(`could not remove run-owned scratch directory: ${error?.code ?? error}`);
      }
    }
  }
  for (const error of cleanupErrors) console.error(`fleet-e2e: cleanup failure — ${error}`);
  return cleanupErrors;
}
process.on("exit", teardown);
for (const sig of ["SIGINT", "SIGTERM"]) {
  process.on(sig, () => {
    teardown();
    process.exit(1);
  });
}

let assertions = 0;
function fail(msg) {
  console.error(`fleet-e2e: FAIL — ${msg}`);
  teardown();
  process.exit(1);
}
function assert(cond, msg) {
  if (!cond) fail(msg);
  assertions += 1;
  console.log(`fleet-e2e: ok — ${msg}`);
}

async function pollUntil(fn, timeoutMs, what, intervalMs = 400) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() > deadline) fail(`timed out after ${timeoutMs}ms waiting for ${what}`);
    await sleep(intervalMs);
  }
}

function scratchDir(name) {
  const dir = join(SCRATCH, name);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  scratchDirs.push(dir);
  return dir;
}

function startHumanDaemon(label, port, dataDir) {
  const proc = startRealDaemon({
    port,
    dataDir,
    label,
    loopback: true,
    onExit: (code, signal) => {
      if (!tearingDown) fail(`${label} daemon exited early (code=${code} signal=${signal})`);
    },
  });
  daemons.push(proc);
  ownedPorts.add(port);
  return proc;
}

// A short-lived daemon used only to create an identity and pre-join; we kill it
// ourselves, so its exit is expected (no fail guard).
function startTempDaemon(label, port, dataDir) {
  const proc = startRealDaemon({ port, dataDir, label, loopback: true, onExit: () => {} });
  daemons.push(proc);
  ownedPorts.add(port);
  return proc;
}

// ---------------------------------------------------------------------------
// The flow
// ---------------------------------------------------------------------------

console.log(`fleet-e2e: scratch = ${SCRATCH}, trials = ${TRIALS}`);
for (const port of [PORT_HUMAN, PORT_AGENT1, PORT_AGENT2]) assertPortAvailable(port);

const humanData = scratchDir("human-data");
const agent1Data = scratchDir("agent1-data");
const agent2Data = scratchDir("agent2-data");
const work1 = scratchDir("work1");
const work2 = scratchDir("work2");

// results tracked as structured strings so the StructuredOutput caller (the
// orchestrator) can read the collision + liveness verdicts from stdout.
let collisionResult = "not-run";
let livenessResult = "not-run";

try {
  // ---- 1. human daemon: identity + room + open -----------------------------
  startHumanDaemon("humand", PORT_HUMAN, humanData);
  const human = new Client("human");
  clients.push(human);
  await human.connect(PORT_HUMAN);
  const humanId = (await human.call("identity.create")).identity_id;
  assert(/^[0-9a-f]{64}$/.test(humanId), "human: identity created (64-hex)");

  const { room_id: roomId } = await human.call("room.create", { name: "Agent Fleet" });
  const opened = await human.call("room.open", { room_id: roomId });
  const humanAddr = opened.endpoint?.addr;
  assert(
    typeof humanAddr === "string" && humanAddr.includes("@"),
    `human: room 'Agent Fleet' open with a dialable addr (${humanAddr})`,
  );

  // ---- pre-join both agents before workload content -------------------------
  // Each agent: temp daemon → identity.create → owner mints an agent invite →
  // room.join (persists membership) → kill temp daemon. No status/message is
  // posted here, which keeps this collision fixture deterministic.
  async function prejoinAgent(label, port, dataDir) {
    const d = startTempDaemon(`${label}-tmp`, port, dataDir);
    const c = new Client(`${label}-tmp`);
    await c.connect(port);
    const id = (await c.call("identity.create")).identity_id;
    const { ticket } = await human.call("invite.create", {
      room_id: roomId,
      identity_id: id,
      role: "agent",
    });
    let joined = null;
    let lastErr = null;
    for (let attempt = 1; attempt <= 5 && !joined; attempt += 1) {
      try {
        joined = await c.call("room.join", { ticket, peers: [humanAddr] }, 90_000);
      } catch (err) {
        lastErr = err;
        console.log(`fleet-e2e: ${label} pre-join attempt ${attempt} failed (${err.message}); retrying`);
        await sleep(2_000);
      }
    }
    if (!joined) fail(`${label} could not pre-join: ${lastErr?.message}`);
    if (joined.room_id !== roomId) fail(`${label} joined the wrong room (${joined.room_id})`);
    c.closedByUs = true; // its daemon is about to die — suppress the onclose exit
    c.close();
    d.kill("SIGKILL");
    await pollUntil(
      () => portListenerPids(port).length === 0,
      5_000,
      `${label}'s temporary daemon to release port ${port}`,
      100,
    );
    return id;
  }

  const agent1Id = await prejoinAgent("agent1", PORT_AGENT1, agent1Data);
  assert(/^[0-9a-f]{64}$/.test(agent1Id), `agent1 pre-joined as an agent member (${agent1Id.slice(0, 12)}…)`);
  const agent2Id = await prejoinAgent("agent2", PORT_AGENT2, agent2Data);
  assert(/^[0-9a-f]{64}$/.test(agent2Id), `agent2 pre-joined as an agent member (${agent2Id.slice(0, 12)}…)`);
  assert(agent1Id !== agent2Id, "the two agents have distinct identities");

  const agentIds = new Set([agent1Id, agent2Id]);

  // human sees both as active role=agent members (membership synced).
  await pollUntil(
    async () => {
      const { members } = await human.call("room.members", { room_id: roomId });
      const agents = members.filter((m) => m.role === "agent" && m.status === "active");
      return agents.length === 2 ? agents : null;
    },
    30_000,
    "both agents to show as active role=agent members on the human",
  );
  assert(true, "human: room.members shows exactly two active role=agent members");

  // ---- start both runners in rejoin mode (echo worker) ----------------------
  function startRunner(label, port, dataDir, workspace) {
    const proc = spawn(
      "node",
      [
        AGENT_SCRIPT,
        "--room", roomId,
        "--peer", humanAddr,
        "--port", String(port),
        "--data-dir", dataDir,
        "--worker", "echo",
        "--workspace", workspace,
        "--trigger", TRIGGER,
        "--allow-sender", humanId,
        "--loopback",
      ],
      { detached: true, stdio: ["ignore", "pipe", "pipe"] },
    );
    const rec = { proc, group: null, exited: false, log: "" };
    runners.push(rec);
    rec.group = recordOwnedProcess(proc.pid);
    ownedPorts.add(port);
    proc.stdout.on("data", (d) => {
      rec.log += d;
      process.stdout.write(`[${label}] ${d}`);
    });
    proc.stderr.on("data", (d) => {
      rec.log += d;
      process.stderr.write(`[${label}] ${d}`);
    });
    proc.on("exit", (code, signal) => {
      rec.exited = true;
      if (!tearingDown && !rec.expectExit) {
        fail(`${label} runner exited early (code=${code} signal=${signal})`);
      }
    });
    return rec;
  }

  const runner1 = startRunner("agent1", PORT_AGENT1, agent1Data, work1);
  const runner2 = startRunner("agent2", PORT_AGENT2, agent2Data, work2);

  for (const [port, dataDir, label] of [
    [PORT_AGENT1, agent1Data, "agent1"],
    [PORT_AGENT2, agent2Data, "agent2"],
  ]) {
    const pid = await pollUntil(
      () => ownedDaemonListenerPid(port, dataDir),
      30_000,
      `${label}'s run-owned daemon listener`,
      100,
    );
    assert(Number.isInteger(pid), `${label}'s daemon exposes one owned listener PID`);
  }
  assert(true, "both runner daemon listeners are identified as run-owned before cleanup");

  // Both announce "online" once their daemon reopens the room.
  const timeline = async () =>
    (await human.call("room.timeline", { room_id: roomId, limit: 1_000_000 })).events;
  const agentStatuses = async (id) =>
    (await timeline()).filter((e) => e.kind === "agent_status" && e.sender?.identity_id === id);

  for (const [id, name] of [[agent1Id, "agent1"], [agent2Id, "agent2"]]) {
    await pollUntil(
      async () => (await agentStatuses(id)).some((e) => e.label === "online"),
      60_000,
      `${name}'s online status`,
    );
  }
  assert(true, "both runners rejoined, opened the room, and posted an online status");

  // Both agent daemons connect back to the human as peers (needed for the
  // fleet 'connected' primary signal). Wait for two connected peers.
  await pollUntil(
    async () => {
      const { peers } = await human.call("peers.status", { room_id: roomId });
      return peers.filter((p) => p.state === "connected").length >= 2 ? peers : null;
    },
    60_000,
    "two connected agent peers on the human session",
  );
  assert(true, "human: both agent daemons are connected peers");

  // Helpers over the human's authoritative timeline.
  const doneEvents = async () =>
    (await timeline()).filter(
      (e) => e.kind === "agent_status" && e.label === "done" && agentIds.has(e.sender?.identity_id),
    );
  const resultFiles = async () => {
    const { files } = await human.call("file.list", { room_id: roomId });
    return files.filter((f) => f.name === "result.txt" && agentIds.has(f.sender_id) && f.available);
  };

  // ---- 2. COLLISION TEST ----------------------------------------------------
  console.log(`fleet-e2e: === COLLISION TEST (${TRIALS} trials) ===`);
  let worstExecutors = 0;
  for (let trial = 1; trial <= TRIALS; trial += 1) {
    const baseDone = new Set((await doneEvents()).map((e) => e.event_id));
    const baseFiles = new Set((await resultFiles()).map((f) => f.file_id));

    await human.call("message.send", { room_id: roomId, body: `${TRIGGER} build the thing #${trial}` });

    // Wait for at least one executor to finish this trial's task…
    await pollUntil(
      async () => (await doneEvents()).some((e) => !baseDone.has(e.event_id)),
      60_000,
      `trial ${trial}: a "done" status from a winning agent`,
    );
    // …then hold, giving a would-be SECOND executor time to also surface.
    await sleep(DOUBLE_GUARD_MS);

    const newDone = (await doneEvents()).filter((e) => !baseDone.has(e.event_id));
    const executors = new Set(newDone.map((e) => e.sender?.identity_id));
    const newFiles = (await resultFiles()).filter((f) => !baseFiles.has(f.file_id));
    const fileSenders = new Set(newFiles.map((f) => f.sender_id));
    worstExecutors = Math.max(worstExecutors, executors.size);
    console.log(
      `fleet-e2e: trial ${trial}: executors=${executors.size} (done events=${newDone.length}), ` +
        `result.txt authors=${fileSenders.size}`,
    );
    assert(
      executors.size === 1,
      `trial ${trial}: EXACTLY ONE agent executed (${executors.size} distinct done-status author(s)) — claim protocol held`,
    );
    assert(
      fileSenders.size === 1 && executors.has([...fileSenders][0]),
      `trial ${trial}: the single executor shared result.txt (and nobody else did)`,
    );
    await sleep(1_500); // let both agents settle back to idle before the next trigger
  }
  collisionResult = `PASS — ${TRIALS} trials, EXACTLY ONE executor every trial (worst case = ${worstExecutors} executor(s) in any single trial)`;
  assert(worstExecutors === 1, `collision: worst case across ${TRIALS} trials was a single executor (no double-run)`);

  // ---- 3. ADDRESSED TRIGGER -------------------------------------------------
  console.log("fleet-e2e: === ADDRESSED TRIGGER ===");
  const prefix2 = agent2Id.slice(0, 16);
  assert(
    !agent1Id.startsWith(prefix2),
    `agent1 does NOT share agent2's 16-hex address prefix (${prefix2}) — the address is unambiguous`,
  );
  const baseDoneA = new Set((await doneEvents()).map((e) => e.event_id));
  const runner1LogMark = runner1.log.length;
  await human.call("message.send", { room_id: roomId, body: `${TRIGGER}:${prefix2} ping` });

  await pollUntil(
    async () => (await doneEvents()).some((e) => !baseDoneA.has(e.event_id)),
    60_000,
    "a done status for the addressed trigger",
  );
  await sleep(DOUBLE_GUARD_MS);
  const addrDone = (await doneEvents()).filter((e) => !baseDoneA.has(e.event_id));
  const addrExecutors = new Set(addrDone.map((e) => e.sender?.identity_id));
  assert(
    addrExecutors.size === 1 && addrExecutors.has(agent2Id) && !addrExecutors.has(agent1Id),
    `addressed trigger executed on agent2 ONLY (executors: ${[...addrExecutors].map((s) => s.slice(0, 8)).join(",")})`,
  );
  await pollUntil(
    () => runner1.log.slice(runner1LogMark).includes(`ignored trigger addressed to ${prefix2}`),
    30_000,
    "agent1 to log that it ignored the addressed trigger",
  );
  assert(true, "agent1 received the addressed trigger and ignored it silently (logged the addressed-ignore line)");

  // ---- 4. FLEET API ---------------------------------------------------------
  console.log("fleet-e2e: === FLEET API ===");
  // Both agents idle + connected right now → each aggregates to online-idle.
  const fleet = await pollUntil(
    async () => {
      const f = await human.call("agents.fleet");
      return f.total === 2 && f.active === 2 && f.working === 0 ? f : null;
    },
    60_000,
    "agents.fleet to report total=2, active=2, working=0 (both idle+connected)",
  );
  assert(fleet.total === 2, `agents.fleet total = 2 (both agent identities present)`);
  assert(fleet.active === 2, `agents.fleet active = 2 (both online-idle right now)`);
  assert(fleet.working === 0, `agents.fleet working = 0 (no task in flight)`);
  assert(
    fleet.working <= fleet.active && fleet.active <= fleet.total,
    `agents.fleet invariant working ≤ active ≤ total holds (${fleet.working} ≤ ${fleet.active} ≤ ${fleet.total})`,
  );
  // rooms coverage must match reality: the human's store knows exactly the one
  // Agent Fleet room, and it has agent members.
  const knownRooms = (await human.call("room.list")).rooms?.length ?? null;
  assert(
    fleet.rooms_total === knownRooms && fleet.rooms_total >= 1,
    `agents.fleet rooms_total (${fleet.rooms_total}) equals the human's real room.list count (${knownRooms})`,
  );
  assert(
    fleet.rooms_covered === 1 && fleet.rooms_covered <= fleet.rooms_total,
    `agents.fleet rooms_covered = 1 (the one room with agent members), ≤ rooms_total`,
  );
  const fleetIds = new Set(fleet.agents.map((a) => a.identity_id));
  assert(
    fleetIds.has(agent1Id) && fleetIds.has(agent2Id),
    "agents.fleet lists both agent identities",
  );
  for (const a of fleet.agents) {
    assert(
      a.liveness === "online-idle",
      `fleet agent ${a.identity_id.slice(0, 8)}… liveness = online-idle (connected + idle-class latest)`,
    );
  }

  // agent.history for agent1 must equal its real agent_status events in the room.
  const realStatuses = await agentStatuses(agent1Id);
  const history = await human.call("agent.history", { room_id: roomId, identity_id: agent1Id });
  assert(
    Array.isArray(history.points) && history.points.length === realStatuses.length,
    `agent.history returns one point per real agent_status event (${history.points.length} == ${realStatuses.length})`,
  );
  // chronological, and every point ts matches a real status ts.
  const realTs = new Set(realStatuses.map((e) => e.ts));
  assert(
    history.points.every((p) => realTs.has(p.ts)) &&
      history.points.every((p, i) => i === 0 || p.ts >= history.points[i - 1].ts),
    "agent.history points are real (each ts matches a stored status) and chronological",
  );
  const histLabels = new Set(history.points.map((p) => p.label));
  assert(
    histLabels.has("online"),
    `agent.history reflects agent1's posted status labels (saw: ${[...histLabels].join(", ")})`,
  );

  // ---- 5. LIVENESS / stale-working fix --------------------------------------
  console.log("fleet-e2e: === LIVENESS / stale-working fix ===");
  // Put agent1 into a genuine "working" status by posting through ITS OWN
  // daemon (a real signed agent_status event by agent1's identity — the same
  // event a long task would produce; echo tasks finish too fast to hold).
  const side = new Client("agent1-side");
  await side.connect(PORT_AGENT1);
  await side.call("status.post", {
    room_id: roomId,
    label: "working",
    message: "long task in progress (liveness probe)",
  });
  // Human must see agent1 as genuinely "working" FIRST — proving the working
  // path reports working while the peer is live.
  await pollUntil(
    async () => {
      const f = await human.call("agents.fleet");
      const a = f.agents.find((x) => x.identity_id === agent1Id);
      return a && a.liveness === "working" && a.latest?.label === "working" ? a : null;
    },
    30_000,
    "agents.fleet to report agent1 as working while its peer is live",
  );
  assert(true, "agent1 reported liveness=working while connected with a fresh working status");

  // Now SIGKILL the runner (and its orphaned daemon): the working process dies
  // WITHOUT posting a terminal status — exactly the "crashed mid-task" case.
  side.closedByUs = true; // agent1's daemon is about to die
  side.close();
  runner1.expectExit = true;
  signalOwnedProcessGroup(runner1.group, "SIGKILL");
  await pollUntil(
    () => portListenerPids(PORT_AGENT1).length === 0,
    5_000,
    "the deliberately killed agent1 process group to release its port",
    100,
  );
  console.log("fleet-e2e: SIGKILLed agent1 runner + daemon while it was 'working'");

  // Wait for the human to detect the dead peer, then assert the derivation
  // reports agent1 as stale/offline — NEVER a live "working" badge.
  const deadAgent = await pollUntil(
    async () => {
      const f = await human.call("agents.fleet");
      const a = f.agents.find((x) => x.identity_id === agent1Id);
      // success once it is no longer reported as working (must land on stale
      // per §1.2 row 2: disconnected + working-class latest → stale).
      return a && a.liveness !== "working" ? a : null;
    },
    120_000,
    "agents.fleet to stop reporting the SIGKILLed 'working' agent as working",
    1_000,
  );
  assert(
    deadAgent.liveness === "stale",
    `SIGKILLed working agent is reported STALE, not working (liveness=${deadAgent.liveness}) — the stale-working fix holds`,
  );
  assert(
    deadAgent.latest?.label === "working",
    "the dead agent's latest posted label is still 'working' — proving peer state (not the label) drove the stale verdict",
  );
  const fleetAfter = await human.call("agents.fleet");
  assert(
    fleetAfter.working === 0,
    `agents.fleet working count is 0 after the crash (no dead process counted as working)`,
  );
  livenessResult = `PASS — a SIGKILLed agent whose latest status is "working" is reported liveness="stale" (never "working"); fleet working count fell to 0`;

  console.log(`fleet-e2e: PASS — ${assertions} assertions green`);
  console.log(`fleet-e2e: COLLISION RESULT: ${collisionResult}`);
  console.log(`fleet-e2e: LIVENESS RESULT: ${livenessResult}`);
  const cleanup = teardown();
  if (cleanup.length > 0) throw new Error(`cleanup failed: ${cleanup.join("; ")}`);
  process.exit(0);
} catch (err) {
  fail(String(err?.stack ?? err));
}
