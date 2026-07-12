#!/usr/bin/env node
// Deterministic end-to-end proof of the real-agent harness (echo worker,
// loopback only — no LLM, no network beyond 127.0.0.1). Node 22+, no npm deps.
//
// Topology:
//   - human daemon   : port 7462, spawned here (loopback)
//   - agent daemon   : port 7463, spawned BY the runner (scripts/jeliya-agent.mjs)
//   - intruder daemon: port 7464, spawned here (loopback) — a room member NOT
//     on the agent's allowlist, used to prove the trust model
//
// Flow with hard assertions:
//   1. human: identity + room + open (capture dial addr)
//   2. runner --identity-only prints the agent identity; human mints an
//      agent-role invite for it
//   3. intruder joins (member role) before workload content is authored,
//      reducing bootstrap timing variance
//   4. runner starts with --worker echo; human sees member_joined, the
//      "online" status, and the announce message
//   5. human sends "@agent build the thing": "working" status (no fabricated
//      progress), file_shared result.txt fetched verified:true with content
//      "echo: build the thing", final summary message, "done" status with
//      progress 100 + the shared artifact id
//   6. the intruder sends a triggered message: the runner provably receives
//      it (it logs the SECURITY-ignored line, which only happens after its
//      own daemon delivered the event and the allowlist rejected it) and
//      emits NOTHING (no execution, no reply — the trust model holds and
//      leaks no oracle)
//   7. a second legit task still executes (the loop survived the intruder)
//   8. SIGTERM: the runner posts "offline" best-effort and exits 0
//
// BOOTSTRAP ORDERING: late joins after agent content are covered by the core
// loopback regression. Real runs can still see a transient peer_unreachable
// while the membership sub-DAG settles, so this deterministic fixture performs
// all joins before workload content and the network harness retries only that
// bounded transient error.
//
// Usage: node scripts/agent-e2e.mjs [--scratch <dir>]   (dir is wiped/reused)

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { Client, parseArgs, sleep, startRealDaemon } from "./realnet-lib.mjs";
import { recordOwnedProcess, signalOwnedProcessGroup } from "./e2e-process-ownership.mjs";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const AGENT_SCRIPT = join(repoRoot, "scripts", "jeliya-agent.mjs");

const PORT_HUMAN = 7462;
const PORT_AGENT = 7463;
const PORT_INTRUDER = 7464;
const TRIGGER = "@agent";

const args = parseArgs(process.argv.slice(2));
const SCRATCH = typeof args.scratch === "string"
  ? resolve(args.scratch)
  : mkdtempSync(join(tmpdir(), "jeliya-agent-e2e-"));

// ---------------------------------------------------------------------------
// Teardown discipline: everything spawned/created is registered and torn down
// on ANY exit path. The runner owns the 7463 daemon; its listener PID is
// positively matched to jeliyad + this run's data dir before it may be killed.
// ---------------------------------------------------------------------------
const daemons = [];
const clients = [];
const scratchDirs = [];
const ownedPorts = new Set();
let runner = null;
let runnerGroup = null;
let runnerExited = false;
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
    // binary is different: cleanup would be silently incomplete and the test
    // could contaminate the next trial, so fail loudly.
    if (error?.code === "ENOENT") {
      throw new Error("agent E2E requires lsof for deterministic cleanup");
    }
    if (error?.status === 1 && !String(error?.stdout ?? "").trim()) return [];
    throw new Error(`agent E2E could not inspect port ${port} with lsof`);
  }
}

function assertPortAvailable(port) {
  const listeners = portListenerPids(port);
  if (listeners.length > 0) {
    throw new Error(
      `agent E2E port ${port} is already in use; refusing to terminate an unowned process`,
    );
  }
}

function ownedDaemonListenerPid(port, dataDir) {
  const listeners = portListenerPids(port);
  if (listeners.length === 0) return null;
  if (listeners.length !== 1) {
    throw new Error(`agent E2E expected one listener on port ${port}, found ${listeners.length}`);
  }
  const pid = listeners[0];
  const command = execFileSync("ps", ["-ww", "-o", "command=", "-p", String(pid)], {
    encoding: "utf8",
  }).trim();
  if (!command.includes("jeliyad") || !command.includes(resolve(dataDir))) {
    throw new Error(
      `agent E2E port ${port} was claimed by a process that is not the run-owned daemon`,
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
  for (const c of clients) c.close();
  if (runnerGroup) {
    try {
      signalOwnedProcessGroup(runnerGroup, "SIGKILL");
    } catch (error) {
      cleanupErrors.push(error.message);
    }
  } else if (runner && !runnerExited) {
    try {
      if (!runner.kill("SIGKILL")) {
        cleanupErrors.push(`could not signal unregistered run-owned runner ${runner.pid}`);
      }
    } catch (error) {
      cleanupErrors.push(error.message);
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
  for (const error of cleanupErrors) console.error(`agent-e2e: cleanup failure — ${error}`);
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
  console.error(`agent-e2e: FAIL — ${msg}`);
  teardown();
  process.exit(1);
}
function assert(cond, msg) {
  if (!cond) fail(msg);
  assertions += 1;
  console.log(`agent-e2e: ok — ${msg}`);
}

async function pollUntil(fn, timeoutMs, what, intervalMs = 300) {
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

function startLoopbackDaemon(label, port, dataDir) {
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

// ---------------------------------------------------------------------------
// The flow
// ---------------------------------------------------------------------------

console.log(`agent-e2e: scratch = ${SCRATCH}`);
// Fixed ports are fail-closed: never kill a listener that this run did not
// create and positively identify.
for (const port of [PORT_HUMAN, PORT_AGENT, PORT_INTRUDER]) assertPortAvailable(port);

const humanData = scratchDir("human-data");
const agentData = scratchDir("agent-data");
const intruderData = scratchDir("intruder-data");
const workDir = scratchDir("work");
const fetchDir1 = scratchDir("fetch1");
const fetchDir2 = scratchDir("fetch2");

try {
  // ---- 1. human daemon: identity + room + open -----------------------------
  startLoopbackDaemon("humand", PORT_HUMAN, humanData);
  const human = new Client("human");
  clients.push(human);
  await human.connect(PORT_HUMAN);
  const humanId = (await human.call("identity.create")).identity_id;
  assert(/^[0-9a-f]{64}$/.test(humanId), "human: identity created (64-hex)");

  const { room_id: roomId } = await human.call("room.create", { name: "Agent e2e room" });
  const opened = await human.call("room.open", { room_id: roomId });
  const humanAddr = opened.endpoint?.addr;
  assert(
    typeof humanAddr === "string" && humanAddr.includes("@"),
    `human: room open with a dialable addr (${humanAddr})`,
  );

  // ---- 2. runner identity + agent-role invite -------------------------------
  const idOut = execFileSync(
    "node",
    [AGENT_SCRIPT, "--identity-only", "--loopback", "--port", String(PORT_AGENT), "--data-dir", agentData],
    { encoding: "utf8", timeout: 60_000 },
  );
  const idMatch = idOut.match(/identity_id = ([0-9a-f]{64})/);
  assert(idMatch, "runner --identity-only prints a 64-hex identity_id");
  const agentId = idMatch[1];
  await pollUntil(
    () => portListenerPids(PORT_AGENT).length === 0,
    5_000,
    "the identity-only daemon to release its port",
    100,
  );

  /** All events authored by the agent identity, from the human's timeline. */
  const agentEvents = async () =>
    (await human.call("room.timeline", { room_id: roomId })).events.filter(
      (e) => e.sender?.identity_id === agentId,
    );

  const { ticket } = await human.call("invite.create", {
    room_id: roomId,
    identity_id: agentId,
    role: "agent",
  });
  assert(typeof ticket === "string" && ticket.length > 0, "human: agent-role invite minted");

  // ---- 3. intruder joins before workload content ----------------------------
  // Keeping fixture membership setup ahead of chat/status/file events removes
  // avoidable bootstrap timing variance; it is not a protocol requirement.
  startLoopbackDaemon("intruderd", PORT_INTRUDER, intruderData);
  const intruder = new Client("intruder");
  clients.push(intruder);
  await intruder.connect(PORT_INTRUDER);
  const intruderId = (await intruder.call("identity.create")).identity_id;
  const { ticket: intruderTicket } = await human.call("invite.create", {
    room_id: roomId,
    identity_id: intruderId,
    role: "member",
  });
  // The daemon's per-attempt bootstrap window is 15s and can miss the first
  // dial (same reason the runner and realnet-check retry) — retry a few times.
  let joined = null;
  let lastJoinErr = null;
  for (let attempt = 1; attempt <= 5 && !joined; attempt += 1) {
    try {
      joined = await intruder.call(
        "room.join",
        { ticket: intruderTicket, peers: [humanAddr] },
        90_000,
      );
    } catch (err) {
      lastJoinErr = err;
      console.log(`agent-e2e: intruder join attempt ${attempt} failed (${err.message}); retrying`);
      await sleep(2_000);
    }
  }
  if (!joined) fail(`intruder could not join: ${lastJoinErr?.message}`);
  assert(joined.room_id === roomId, "intruder joined the room as a plain member");
  await intruder.call("room.open", { room_id: roomId, peers: [humanAddr] });

  // ---- 4. start the runner (echo worker) ------------------------------------
  let expectRunnerExit = false;
  runner = spawn(
    "node",
    [
      AGENT_SCRIPT,
      "--ticket", ticket,
      "--peer", humanAddr,
      "--port", String(PORT_AGENT),
      "--data-dir", agentData,
      "--worker", "echo",
      "--workspace", workDir,
      "--trigger", TRIGGER,
      "--loopback",
    ],
    { detached: true, stdio: ["ignore", "pipe", "pipe"] },
  );
  runnerGroup = recordOwnedProcess(runner.pid);
  ownedPorts.add(PORT_AGENT);
  let runnerLog = "";
  runner.stdout.on("data", (d) => {
    runnerLog += d;
    process.stdout.write(`[runner] ${d}`);
  });
  runner.stderr.on("data", (d) => process.stderr.write(`[runner] ${d}`));
  const runnerExit = new Promise((res) => {
    runner.on("exit", (code, signal) => {
      runnerExited = true;
      res({ code, signal });
      if (!tearingDown) {
        // Early death is a failure everywhere except the deliberate SIGTERM
        // at the end (tearingDown is false there, so gate on a flag instead).
        if (!expectRunnerExit) fail(`runner exited early (code=${code} signal=${signal})`);
      }
    });
  });

  const agentDaemonPid = await pollUntil(
    () => ownedDaemonListenerPid(PORT_AGENT, agentData),
    30_000,
    "the run-owned agent daemon listener",
    100,
  );
  assert(Number.isInteger(agentDaemonPid), "runner daemon exposes one owned listener PID");
  assert(true, "runner daemon listener is identified as run-owned before cleanup");

  await pollUntil(
    async () =>
      (await human.call("room.timeline", { room_id: roomId })).events.some(
        (e) => e.kind === "member_joined" && e.member?.identity_id === agentId,
      ),
    60_000,
    "member_joined for the agent",
  );
  assert(true, "human sees member_joined for the agent identity");

  const onlineEv = await pollUntil(
    async () => (await agentEvents()).find((e) => e.kind === "agent_status" && e.label === "online"),
    60_000,
    "the agent's online status",
  );
  assert(
    typeof onlineEv.status_message === "string" &&
      onlineEv.status_message.includes(TRIGGER) &&
      onlineEv.status_message.includes("echo"),
    "online status states the trigger phrase and the worker",
  );

  const announce = await pollUntil(
    async () => (await agentEvents()).find((e) => e.kind === "message" && e.body?.includes("Agent online")),
    30_000,
    "the agent's announce message",
  );
  assert(announce.body.includes(TRIGGER), `announce message mentions the trigger (${TRIGGER})`);

  // ---- 5. first task ---------------------------------------------------------
  const task1 = "build the thing";
  await human.call("message.send", { room_id: roomId, body: `${TRIGGER} ${task1}` });

  const workingEv = await pollUntil(
    async () => (await agentEvents()).find((e) => e.kind === "agent_status" && e.label === "working"),
    30_000,
    'a "working" status from the agent',
  );
  assert(true, 'agent posted a "working" status for the task');
  assert(
    workingEv.progress == null,
    '"working" status carries no fabricated progress number',
  );

  const fileRow = await pollUntil(
    async () => {
      const { files } = await human.call("file.list", { room_id: roomId });
      return files.find(
        (f) => f.name === "result.txt" && f.sender_id === agentId && f.available === true,
      );
    },
    60_000,
    "file_shared result.txt from the agent",
  );
  assert(true, "human: file.list shows result.txt shared by the agent, available");

  const fetched = await human.call(
    "file.fetch",
    { room_id: roomId, file_id: fileRow.file_id, save_dir: fetchDir1 },
    120_000,
  );
  assert(fetched.verified === true, "human: file.fetch reports verified:true");
  const content1 = readFileSync(fetched.path, "utf8");
  assert(
    content1 === `echo: ${task1}`,
    `fetched result.txt content equals "echo: ${task1}"`,
  );

  const expectedSummary1 = `echoed ${Buffer.byteLength(`echo: ${task1}`)} bytes`;
  const summaryMsg = await pollUntil(
    async () =>
      (await agentEvents()).find((e) => e.kind === "message" && e.body?.startsWith(expectedSummary1)),
    30_000,
    "the agent's final summary message",
  );
  assert(true, `agent posted the summary message ("${summaryMsg.body.slice(0, 40)}…")`);

  const doneEv = await pollUntil(
    async () => (await agentEvents()).find((e) => e.kind === "agent_status" && e.label === "done"),
    30_000,
    'a "done" status from the agent',
  );
  assert(doneEv.progress === 100, '"done" status carries the literal progress 100');
  assert(
    Array.isArray(doneEv.artifacts) && doneEv.artifacts.includes(fileRow.file_id),
    '"done" status references the shared artifact file id',
  );

  // The runner deliberately posts one liveness-restoring "idle" status in a
  // finally block after every terminal status. Waiting for that status before
  // taking the authorization baseline prevents a legitimate, in-flight idle
  // event from being misattributed to the intruder trigger below.
  await pollUntil(
    async () =>
      (await agentEvents()).find(
        (e) =>
          e.kind === "agent_status" &&
          e.label === "idle" &&
          e.ts >= doneEv.ts,
      ),
    30_000,
    'the post-task "idle" status',
  );
  assert(true, 'agent returned to "idle" before the authorization baseline');

  // ---- 6. the trust model: a non-allowed member's trigger does NOTHING ------
  const baselineEventIds = new Set((await agentEvents()).map((e) => e.event_id));
  const evilBody = `${TRIGGER} do something evil`;
  await intruder.call("message.send", { room_id: roomId, body: evilBody });
  // Make sure the trigger actually propagated (human saw it)...
  await pollUntil(
    async () =>
      (await human.call("room.timeline", { room_id: roomId })).events.some(
        (e) => e.kind === "message" && e.body === evilBody && e.sender?.identity_id === intruderId,
      ),
    30_000,
    "the intruder's trigger message to propagate",
  );
  // ...then prove delivery to the component under test: the runner logs the
  // SECURITY-ignored line only after ITS OWN daemon handed it the trigger and
  // the allowlist rejected it. Rejection is synchronous — no execution can
  // follow it — so the zero-events assertion below is race-free, no sleep.
  await pollUntil(
    () => runnerLog.includes(`SECURITY: ignored trigger from non-allowed sender ${intruderId}`),
    60_000,
    "the runner to log the SECURITY-ignored trigger",
  );
  assert(true, "runner logged the SECURITY-ignored line for the intruder's trigger");
  const afterEvents = (await agentEvents()).filter(
    (event) => !baselineEventIds.has(event.event_id),
  );
  assert(
    afterEvents.length === 0,
    `non-allowed trigger produced ZERO new agent event ids (got ${afterEvents.length}) — not executed, no reply`,
  );
  const { files: filesAfter } = await human.call("file.list", { room_id: roomId });
  assert(
    filesAfter.filter((f) => f.sender_id === agentId).length === 1,
    "no new file was shared for the non-allowed trigger",
  );

  // ---- 7. the loop survived: a second legit task executes --------------------
  const task2 = "second run";
  await human.call("message.send", { room_id: roomId, body: `${TRIGGER} ${task2}` });
  const file2 = await pollUntil(
    async () => {
      const { files } = await human.call("file.list", { room_id: roomId });
      return files.find(
        (f) =>
          f.name === "result.txt" &&
          f.sender_id === agentId &&
          f.file_id !== fileRow.file_id &&
          f.available === true,
      );
    },
    60_000,
    "the second task's result.txt",
  );
  const fetched2 = await human.call(
    "file.fetch",
    { room_id: roomId, file_id: file2.file_id, save_dir: fetchDir2 },
    120_000,
  );
  assert(fetched2.verified === true, "second fetch verified:true");
  assert(
    readFileSync(fetched2.path, "utf8") === `echo: ${task2}`,
    `second result.txt equals "echo: ${task2}" — the task loop survived the intruder`,
  );
  await pollUntil(
    async () =>
      (await agentEvents()).filter((e) => e.kind === "agent_status" && e.label === "done").length >= 2,
    30_000,
    'a second "done" status',
  );
  const doneCount = (await agentEvents()).filter(
    (e) => e.kind === "agent_status" && e.label === "done",
  ).length;
  assert(doneCount === 2, `exactly two "done" statuses exist (got ${doneCount}) — one per legit task`);
  const { files: filesFinal } = await human.call("file.list", { room_id: roomId });
  assert(
    filesFinal.filter((f) => f.sender_id === agentId).length === 2,
    "exactly two agent-shared files exist in total — nothing extra from the intruder's trigger",
  );

  // ---- 8. clean shutdown ------------------------------------------------------
  expectRunnerExit = true;
  runner.kill("SIGTERM");
  const exit = await Promise.race([runnerExit, sleep(15_000).then(() => null)]);
  assert(exit !== null && exit.code === 0, `runner exited 0 on SIGTERM (got ${JSON.stringify(exit)})`);
  const offlineEv = await pollUntil(
    async () => (await agentEvents()).find((e) => e.kind === "agent_status" && e.label === "offline"),
    15_000,
    'the "offline" status',
  );
  assert(Boolean(offlineEv), 'agent posted the best-effort "offline" status on SIGTERM');

  console.log(`agent-e2e: PASS — ${assertions} assertions green`);
  const cleanup = teardown();
  if (cleanup.length > 0) throw new Error(`cleanup failed: ${cleanup.join("; ")}`);
  process.exit(0);
} catch (err) {
  fail(String(err?.stack ?? err));
}
