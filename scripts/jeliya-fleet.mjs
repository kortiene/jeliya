#!/usr/bin/env node
// Jeliya fleet supervisor: spawn several jeliya-agent.mjs runners from one
// JSON config, restart crashed runners with backoff, forward shutdown
// signals, and log each child's liveness. Node 22+, no npm deps.
//
// Usage:
//   node scripts/jeliya-fleet.mjs --config fleet.json [--max-restarts 3]
//
// Config schema (docs/agent-orchestration.md §4):
//   {
//     "agents": [
//       {
//         "name": "builder-1",                 // required, unique — log prefix
//         "room_id": "…",                      // rejoin mode (--room)
//         "ticket": "…",                       // first-join mode (--ticket);
//                                              //   may combine with room_id
//         "peer": ["<endpoint_id>@<ip:port>"], // optional; string or array
//         "worker": "claude" | "echo",         // required
//         "trigger": "@agent",                 // optional (--trigger)
//         "allow_sender": ["…64-hex…"],        // optional (--allow-sender)
//         "data_dir": "/home/jeliya/.local/share/jeliya/agents/b1",
//                                               // required, unique per agent
//         "port": 7481,                        // required, unique per agent
//         "loopback": false                    // optional (--loopback)
//       }
//     ]
//   }
//
// SCOPE / HONESTY: this script only spawns and monitors child PROCESSES and
// prefixes their logs. All room logic (join, claims, statuses) stays in the
// runner. The supervisor posts NO statuses of its own and fabricates NO fleet
// counts — every liveness line below states a real child-process fact
// (spawned pid / exited code / restarting / gave up); the dashboard's numbers
// come only from the daemon's agents.fleet read. A ticket-mode entry switches
// to rejoin mode (--room) on restart once its first join succeeded, so a
// restart never burns a second join on an already-member identity.

import { execFileSync, spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const AGENT_SCRIPT = join(repoRoot, "scripts", "jeliya-agent.mjs");

const DEFAULT_MAX_RESTARTS = 3; // per child, per fleet lifetime
const BACKOFF_BASE_MS = 2_000; // restart delay: BASE * 2^(restarts-1)
const SHUTDOWN_GRACE_MS = 20_000; // SIGTERM → SIGKILL escalation window
const LIVENESS_LOG_INTERVAL_MS = 30_000; // periodic child-process summary

function log(msg) {
  console.log(`fleet: ${msg}`);
}

function die(msg) {
  console.error(`fleet: ${msg}`);
  process.exit(1);
}

function usage(code) {
  console.error("usage: node scripts/jeliya-fleet.mjs --config <fleet.json> [--max-restarts N]");
  process.exit(code);
}

// ---------------------------------------------------------------------------
// CLI + config validation (fail-fast, before any spawn)
// ---------------------------------------------------------------------------

let configPath = null;
let maxRestarts = DEFAULT_MAX_RESTARTS;
{
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    const need = () => {
      if (i + 1 >= argv.length) die(`${a} needs a value`);
      i += 1;
      return argv[i];
    };
    switch (a) {
      case "--config": configPath = need(); break;
      case "--max-restarts": maxRestarts = Number(need()); break;
      case "--help": case "-h": usage(0); break;
      default:
        console.error(`fleet: unknown flag ${a}`);
        usage(2);
    }
  }
}
if (!configPath) usage(2);
if (!Number.isInteger(maxRestarts) || maxRestarts < 0) die("--max-restarts must be a non-negative integer");

let config;
try {
  config = JSON.parse(readFileSync(resolve(configPath), "utf8"));
} catch (err) {
  die(`cannot read config ${configPath}: ${err.message}`);
}
if (!Array.isArray(config?.agents) || config.agents.length === 0) {
  die('config must have a non-empty "agents" array');
}

const KNOWN_FIELDS = new Set([
  "name", "room_id", "ticket", "peer", "worker", "trigger",
  "allow_sender", "data_dir", "port", "loopback",
]);
const seenNames = new Set();
const seenPorts = new Set();
const seenDataDirs = new Set();

function validateEntry(raw, i) {
  const where = `agents[${i}]`;
  if (typeof raw !== "object" || raw === null) die(`${where}: not an object`);
  for (const k of Object.keys(raw)) {
    if (!KNOWN_FIELDS.has(k)) die(`${where}: unknown field ${JSON.stringify(k)}`);
  }
  const name = raw.name;
  if (typeof name !== "string" || name.length === 0) die(`${where}: "name" is required`);
  if (seenNames.has(name)) die(`${where}: duplicate name ${JSON.stringify(name)}`);
  seenNames.add(name);

  if (raw.worker !== "claude" && raw.worker !== "echo") {
    die(`${where} (${name}): "worker" must be "claude" or "echo"`);
  }
  if (raw.ticket == null && raw.room_id == null) {
    die(`${where} (${name}): needs "ticket" (first join) and/or "room_id" (rejoin)`);
  }
  for (const f of ["ticket", "room_id", "trigger"]) {
    if (raw[f] != null && (typeof raw[f] !== "string" || raw[f].length === 0)) {
      die(`${where} (${name}): "${f}" must be a non-empty string`);
    }
  }
  if (typeof raw.data_dir !== "string" || raw.data_dir.length === 0) {
    die(`${where} (${name}): "data_dir" is required`);
  }
  const dataDir = resolve(raw.data_dir);
  if (seenDataDirs.has(dataDir)) {
    die(`${where} (${name}): duplicate data_dir ${dataDir} — two runners must never share a daemon data dir`);
  }
  seenDataDirs.add(dataDir);
  if (!Number.isInteger(raw.port) || raw.port <= 0) {
    die(`${where} (${name}): "port" must be a positive integer`);
  }
  if (seenPorts.has(raw.port)) die(`${where} (${name}): duplicate port ${raw.port}`);
  seenPorts.add(raw.port);

  let peers = raw.peer ?? [];
  if (typeof peers === "string") peers = [peers]; // bare string = one-element array
  if (!Array.isArray(peers) || peers.some((p) => typeof p !== "string" || p.length === 0)) {
    die(`${where} (${name}): "peer" must be a dial string or an array of dial strings`);
  }
  const allowSenders = raw.allow_sender ?? [];
  if (!Array.isArray(allowSenders) || allowSenders.some((s) => !/^[0-9a-f]{64}$/.test(s ?? ""))) {
    die(`${where} (${name}): "allow_sender" must be an array of 64-hex identity ids`);
  }
  if (raw.loopback != null && typeof raw.loopback !== "boolean") {
    die(`${where} (${name}): "loopback" must be a boolean`);
  }
  return {
    name,
    ticket: raw.ticket ?? null,
    roomId: raw.room_id ?? null, // learned from the runner's log after a first ticket join
    peers,
    worker: raw.worker,
    trigger: raw.trigger ?? null,
    allowSenders,
    dataDir,
    port: raw.port,
    loopback: raw.loopback === true,
    // supervision state
    child: null,
    restarts: 0,
    restartTimer: null,
    joinedOnce: raw.room_id != null,
    state: "pending", // pending | running | restarting | stopped | gave-up
  };
}

const agents = config.agents.map(validateEntry);
log(`config OK — ${agents.length} agent(s): ${agents.map((a) => a.name).join(", ")}`);

// ---------------------------------------------------------------------------
// Spawning + supervision
// ---------------------------------------------------------------------------

let shuttingDown = false;

/**
 * A hard-killed runner cannot reap the jeliyad it spawned, leaving the
 * agent's dedicated port bound and every restart failing with "Address
 * already in use". Before a restart, SIGKILL whatever still listens on that
 * port — by config contract the port belongs to this agent alone (same
 * discipline as agent-e2e's killPortListeners).
 */
function killPortListeners(agent) {
  let out;
  try {
    out = execFileSync("lsof", ["-ti", `tcp:${agent.port}`], { encoding: "utf8" });
  } catch {
    return; // lsof exits nonzero when nothing listens — fine
  }
  for (const pidStr of out.split("\n").filter(Boolean)) {
    const pid = Number(pidStr);
    if (!Number.isInteger(pid) || pid === process.pid) continue;
    log(`${agent.name}: killing orphaned listener pid=${pid} on port ${agent.port}`);
    try {
      process.kill(pid, "SIGKILL");
    } catch {}
  }
}

function runnerArgs(agent) {
  const args = [AGENT_SCRIPT];
  // After a successful first join the identity is a member: rejoin by room_id
  // (the runner errors on a reused single-use ticket, and doesn't need one).
  if (agent.ticket && !agent.joinedOnce) {
    args.push("--ticket", agent.ticket);
    if (agent.roomId) args.push("--room", agent.roomId); // cross-check, like the runner
  } else {
    args.push("--room", agent.roomId);
  }
  for (const p of agent.peers) args.push("--peer", p);
  args.push("--worker", agent.worker);
  if (agent.trigger) args.push("--trigger", agent.trigger);
  for (const s of agent.allowSenders) args.push("--allow-sender", s);
  args.push("--data-dir", agent.dataDir, "--port", String(agent.port));
  if (agent.loopback) args.push("--loopback");
  return args;
}

function prefixPipe(agent, stream, sink) {
  let buf = "";
  stream.setEncoding("utf8");
  stream.on("data", (d) => {
    buf += d;
    let nl;
    while ((nl = buf.indexOf("\n")) !== -1) {
      const line = buf.slice(0, nl);
      buf = buf.slice(nl + 1);
      sink.write(`[${agent.name}] ${line}\n`);
      // Real join evidence from the runner's own log: after this, restarts
      // switch to rejoin mode (--room) instead of reusing the ticket.
      const m = /agent: joined room (\S+)/.exec(line);
      if (m) {
        agent.roomId = m[1];
        agent.joinedOnce = true;
      }
    }
  });
  stream.on("end", () => {
    if (buf) sink.write(`[${agent.name}] ${buf}\n`);
  });
}

function startAgent(agent) {
  if (shuttingDown) return;
  if (!agent.ticket && !agent.roomId) {
    // Unreachable given validation, but never spawn a runner with no join mode.
    agent.state = "gave-up";
    log(`${agent.name}: no ticket or room_id available — cannot start`);
    return;
  }
  const child = spawn("node", runnerArgs(agent), { stdio: ["ignore", "pipe", "pipe"] });
  agent.child = child;
  agent.state = "running";
  log(`${agent.name}: spawned runner pid=${child.pid} (port ${agent.port}, worker ${agent.worker})`);
  prefixPipe(agent, child.stdout, process.stdout);
  prefixPipe(agent, child.stderr, process.stderr);
  child.on("error", (err) => {
    log(`${agent.name}: spawn error: ${err.message}`);
  });
  child.on("exit", (code, signal) => {
    agent.child = null;
    if (shuttingDown) {
      agent.state = "stopped";
      log(`${agent.name}: exited during shutdown (code=${code} signal=${signal})`);
      maybeFinishShutdown();
      return;
    }
    log(`${agent.name}: runner exited unexpectedly (code=${code} signal=${signal})`);
    if (agent.restarts >= maxRestarts) {
      agent.state = "gave-up";
      log(`${agent.name}: gave up after ${agent.restarts} restart(s) (max ${maxRestarts})`);
      maybeExitAllDead();
      return;
    }
    agent.restarts += 1;
    const delay = BACKOFF_BASE_MS * 2 ** (agent.restarts - 1);
    agent.state = "restarting";
    log(`${agent.name}: restart ${agent.restarts}/${maxRestarts} in ${delay}ms${agent.joinedOnce ? ` (rejoin --room ${agent.roomId})` : ""}`);
    agent.restartTimer = setTimeout(() => {
      agent.restartTimer = null;
      killPortListeners(agent); // the dead runner's orphaned daemon, if any
      startAgent(agent);
    }, delay);
  });
}

/** All children permanently done and none coming back: exit non-zero. */
function maybeExitAllDead() {
  if (shuttingDown) return;
  if (agents.every((a) => a.state === "gave-up")) {
    log("all runners gave up — exiting 1");
    process.exit(1);
  }
}

// ---------------------------------------------------------------------------
// Shutdown: forward the signal to every child, wait, escalate, exit 0.
// ---------------------------------------------------------------------------

let shutdownKillTimer = null;

function maybeFinishShutdown() {
  if (!shuttingDown) return;
  if (agents.some((a) => a.child !== null)) return;
  if (shutdownKillTimer) clearTimeout(shutdownKillTimer);
  log("all runners exited — fleet down");
  process.exit(0);
}

function shutdown(sig) {
  if (shuttingDown) return;
  shuttingDown = true;
  log(`${sig} received — forwarding to all runners and waiting`);
  for (const agent of agents) {
    if (agent.restartTimer) {
      clearTimeout(agent.restartTimer);
      agent.restartTimer = null;
      agent.state = "stopped";
    }
    if (agent.child) {
      try {
        agent.child.kill("SIGTERM"); // the runner posts "offline" and exits 0
      } catch {}
    }
  }
  shutdownKillTimer = setTimeout(() => {
    for (const agent of agents) {
      if (agent.child) {
        log(`${agent.name}: still running after ${SHUTDOWN_GRACE_MS}ms — SIGKILL`);
        try {
          agent.child.kill("SIGKILL");
        } catch {}
      }
    }
    // exit handler below reaps anything that survives even that
    setTimeout(() => process.exit(0), 2_000).unref();
  }, SHUTDOWN_GRACE_MS);
  shutdownKillTimer.unref?.();
  maybeFinishShutdown(); // in case nothing was running
}
for (const sig of ["SIGINT", "SIGTERM"]) {
  process.on(sig, () => shutdown(sig));
}
process.on("exit", () => {
  for (const agent of agents) {
    if (agent.child) {
      try {
        agent.child.kill("SIGKILL");
      } catch {}
    }
  }
});

// ---------------------------------------------------------------------------
// Liveness log: a periodic one-line summary of REAL child-process state.
// (Process liveness only — room-level liveness comes from agents.fleet.)
// ---------------------------------------------------------------------------

setInterval(() => {
  if (shuttingDown) return;
  const parts = agents.map((a) => {
    if (a.child) return `${a.name}=running(pid ${a.child.pid})`;
    return `${a.name}=${a.state}`;
  });
  log(`liveness: ${parts.join(" ")}`);
}, LIVENESS_LOG_INTERVAL_MS).unref();

for (const agent of agents) startAgent(agent);
