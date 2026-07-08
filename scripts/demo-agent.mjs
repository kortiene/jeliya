#!/usr/bin/env node
// Jeliya demo orchestrator (Node 22+, global WebSocket, no npm deps).
//
// Drives the developer demo against an already-running human daemon
// (default ws://127.0.0.1:7420/ws):
//   1. ensures the human daemon has an identity and the demo room, open
//   2. spawns a second `jeliyad` (the agent daemon) on --agent-port
//   3. invites the agent identity (role agent) and joins it to the room
//   4. posts periodic agent.status updates (plus an occasional message)
//      until Ctrl-C
//
// Usage: node scripts/demo-agent.mjs [--human-port 7420] [--agent-port 7421]
//                                    [--agent-data-dir .jeliya-demo/agent]

import { spawn } from "node:child_process";
import { mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { defaultDataDir, pipeDaemonOutput, wsUrlFor } from "./daemon-token.mjs";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const BINARY = join(repoRoot, "target", "debug", "jeliyad");
const ROOM_NAME = "Build Iroh Rooms MVP";

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  return i >= 0 && process.argv[i + 1] ? process.argv[i + 1] : fallback;
}
const HUMAN_PORT = Number(arg("human-port", "7420"));
const AGENT_PORT = Number(arg("agent-port", "7421"));
const AGENT_DIR = resolve(repoRoot, arg("agent-data-dir", ".jeliya-demo/agent"));

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let agentDaemon = null;
let stopping = false;
function shutdown(code) {
  stopping = true;
  if (agentDaemon) {
    try {
      agentDaemon.kill("SIGKILL");
    } catch {}
  }
  process.exit(code);
}
for (const sig of ["SIGINT", "SIGTERM"]) process.on(sig, () => shutdown(0));

function die(msg) {
  console.error(`demo-agent: ${msg}`);
  shutdown(1);
}

class Client {
  constructor(label, port, dataDir = defaultDataDir()) {
    this.label = label;
    this.port = port;
    this.dataDir = dataDir;
    this.url = `ws://127.0.0.1:${port}/ws`;
    this.nextId = 1;
    this.pending = new Map();
    this.ws = null;
  }

  async connect(deadlineMs = 60_000) {
    const start = Date.now();
    for (;;) {
      // Recomputed per attempt: the portfile (with the auth token) appears
      // when the daemon is ready, and early attempts race it.
      this.url = wsUrlFor(this.port, this.dataDir);
      try {
        const ws = new WebSocket(this.url);
        await new Promise((res, rej) => {
          ws.onopen = () => res();
          ws.onerror = () => rej(new Error("connect failed"));
        });
        this.ws = ws;
        ws.onmessage = (event) => {
          const frame = JSON.parse(String(event.data));
          if (frame.push) return; // the demo driver ignores pushes
          const waiter = this.pending.get(frame.id);
          if (waiter) {
            this.pending.delete(frame.id);
            waiter(frame);
          }
        };
        ws.onclose = () => {
          if (!stopping) die(`${this.label}: websocket to ${this.url} closed`);
        };
        return;
      } catch {
        if (Date.now() - start > deadlineMs)
          die(`could not connect to ${this.url} — is the daemon running?`);
        await sleep(300);
      }
    }
  }

  callRaw(method, params = {}, timeoutMs = 60_000) {
    const id = this.nextId++;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((res, rej) => {
      const timer = setTimeout(
        () => rej(new Error(`${this.label}: ${method} timed out`)),
        timeoutMs,
      );
      this.pending.set(id, (frame) => {
        clearTimeout(timer);
        res(frame);
      });
    });
  }

  async call(method, params = {}, timeoutMs = 60_000) {
    const frame = await this.callRaw(method, params, timeoutMs);
    if (frame.ok !== true)
      die(`${this.label}: ${method} errored: ${JSON.stringify(frame.error)}`);
    return frame.result;
  }
}

// --- 1. human daemon: identity + demo room, open ---------------------------
const human = new Client("human", HUMAN_PORT);
await human.connect();
let status = await human.call("daemon.status");
if (!status.identity) {
  const created = await human.call("identity.create");
  console.log(`demo-agent: created human identity ${created.identity_id.slice(0, 12)}…`);
}

const { rooms } = await human.call("room.list");
let roomId = rooms.find((r) => r.name === ROOM_NAME)?.room_id;
if (!roomId) {
  ({ room_id: roomId } = await human.call("room.create", { name: ROOM_NAME }));
  console.log(`demo-agent: created room ${roomId.slice(0, 20)}… ("${ROOM_NAME}")`);
}
const openedHuman = await human.call("room.open", { room_id: roomId });
const humanAddr = openedHuman.endpoint.addr;
if (!humanAddr) die("the human daemon's room session has no dialable addr");
console.log(`demo-agent: room open on the human daemon at ${humanAddr}`);

// --- 2. the agent daemon ----------------------------------------------------
mkdirSync(AGENT_DIR, { recursive: true });
agentDaemon = spawn(
  BINARY,
  ["--loopback", "--port", String(AGENT_PORT), "--data-dir", AGENT_DIR],
  { stdio: ["ignore", "pipe", "pipe"] },
);
pipeDaemonOutput(agentDaemon, "agentd", (code, signal) => {
  if (!stopping) die(`agent daemon exited early (code=${code} signal=${signal})`);
});

const agent = new Client("agent", AGENT_PORT, AGENT_DIR);
await agent.connect();
status = await agent.call("daemon.status");
let agentIdentity = status.identity?.identity_id;
if (!agentIdentity) {
  ({ identity_id: agentIdentity } = await agent.call("identity.create"));
  console.log(`demo-agent: created agent identity ${agentIdentity.slice(0, 12)}…`);
}

// --- 3. invite + join (idempotent across demo restarts) ---------------------
const { members } = await human.call("room.members", { room_id: roomId });
const already = members.find(
  (m) => m.identity_id === agentIdentity && m.status === "active",
);
if (!already) {
  const { ticket } = await human.call("invite.create", {
    room_id: roomId,
    identity_id: agentIdentity,
    role: "agent",
  });
  await agent.call("room.join", { ticket, peers: [humanAddr] }, 90_000);
  console.log("demo-agent: agent joined the room");
} else {
  console.log("demo-agent: agent is already an active member");
}
await agent.call("room.open", { room_id: roomId, peers: [humanAddr] });
console.log("demo-agent: agent room session open — posting periodic statuses");

// --- 4. periodic agent.status ------------------------------------------------
const script = [
  { label: "planning", message: "Reading the PRD and sketching the plan", progress: 5 },
  { label: "scaffolding", message: "Generating the crate layout", progress: 20 },
  { label: "implementing", message: "Wiring the sync engine to the store", progress: 45 },
  { label: "running_tests", message: "cargo test --workspace", progress: 60 },
  { label: "fixing", message: "Two flaky assertions in the join flow", progress: 75 },
  { label: "running_tests", message: "Re-running the full suite", progress: 90 },
  { label: "done", message: "All green — ready for review", progress: 100 },
];
let step = 0;
await agent.call("message.send", {
  room_id: roomId,
  body: "Agent online — starting the build loop.",
});
for (;;) {
  const s = script[step % script.length];
  await agent.call("status.post", {
    room_id: roomId,
    label: s.label,
    message: s.message,
    progress: s.progress,
  });
  console.log(`demo-agent: posted status ${s.label} (${s.progress}%)`);
  if (step % script.length === script.length - 1) {
    await agent.call("message.send", {
      room_id: roomId,
      body: "Build loop finished — restarting the demo cycle.",
    });
  }
  step += 1;
  await sleep(5_000);
}
