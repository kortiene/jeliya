#!/usr/bin/env node
// Jeliya two-daemon end-to-end test (Node 22+, global WebSocket, no npm deps).
//
// Spawns TWO `jeliyad` daemons (A on 7411, B on 7412, separate scratch data
// dirs) and drives the full product flow with hard assertions:
//
//   a. both: daemon.status + identity.create
//   b. A: room.create "Build Iroh Rooms MVP" + room.open (capture addr)
//   c. A: invite.create for B (role member); B: room.join with ticket +
//      A's addr; A sees member_joined (push or bounded timeline poll)
//   d. messages both ways; both timelines show both, kind=message, correct sender
//   e. A: file.share; B: file.list shows it available; B: file.fetch ->
//      verified:true + byte-identical content
//   f. B: status.post (running_tests, 60); A's timeline shows the agent_status
//   g. A: pipe.expose of a throwaway local HTTP server authorized to B;
//      B: pipe.connect -> HTTP GET through the forwarded local_addr; pipe.close
//   h. push discipline: every room.event push carries a distinct event_id
//      (exactly once per event per client), checked on both daemons
//
// Usage: node scripts/e2e.mjs [--mode loopback|real]   (builds the workspace first)
//
//   --mode loopback (default): daemons run with `--loopback` (the SDK's
//     offline/CI network stack over 127.0.0.1).
//   --mode real: daemons run WITHOUT `--loopback`, on the SDK's real network
//     stack (iroh N0 preset: relay + DNS discovery). Same 67 assertions; the
//     explicit dial addrs passed by the flow make same-host direct
//     connections work even when relays are unreachable.
//   The JELIYA_E2E_MODE env var is honored; the flag wins over the env.

import { execFileSync, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { wsUrlFor } from "./daemon-token.mjs";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const BINARY = join(repoRoot, "target", "debug", "jeliyad");
const PORT_A = 7411;
const PORT_B = 7412;

/** Network mode: "loopback" (default) or "real". Flag > env > default. */
function parseMode() {
  let mode = process.env.JELIYA_E2E_MODE || "loopback";
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--mode") mode = argv[i + 1];
    else if (argv[i].startsWith("--mode=")) mode = argv[i].slice("--mode=".length);
  }
  if (mode !== "loopback" && mode !== "real") {
    console.error(`e2e: invalid mode ${JSON.stringify(mode)} — use --mode loopback|real`);
    process.exit(2);
  }
  return mode;
}
const MODE = parseMode();
const REAL = MODE === "real";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------------------------------------------------------------------------
// Teardown discipline: every spawned process/server/dir is registered here and
// torn down on ANY exit path (pass, assertion failure, crash, signal).
// ---------------------------------------------------------------------------
const daemons = [];
const dataDirs = [];
let httpServer = null;
let tearingDown = false;

function teardown() {
  tearingDown = true;
  for (const d of daemons) {
    try {
      d.proc.kill("SIGKILL");
    } catch {}
  }
  if (httpServer) {
    try {
      httpServer.close();
    } catch {}
  }
  for (const dir of dataDirs) {
    try {
      rmSync(dir, { recursive: true, force: true });
    } catch {}
  }
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
  console.error(`e2e: FAIL — ${msg}`);
  teardown();
  process.exit(1);
}
function assert(cond, msg) {
  if (!cond) fail(msg);
  assertions += 1;
  console.log(`e2e: ok — ${msg}`);
}

/** Poll `fn` (may be async; returns truthy to stop) with a deadline. */
async function pollUntil(fn, timeoutMs, what) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() > deadline) fail(`timed out after ${timeoutMs}ms waiting for ${what}`);
    await sleep(200);
  }
}

// ---------------------------------------------------------------------------
// Daemon + protocol client
// ---------------------------------------------------------------------------

/** Data dir by port so Client.connect can read the portfile's auth token. */
const dataDirByPort = new Map();

function startDaemon(label, port) {
  const dataDir = mkdtempSync(join(tmpdir(), `jeliya-e2e-${label}-`));
  dataDirs.push(dataDir);
  dataDirByPort.set(port, dataDir);
  const proc = spawn(
    BINARY,
    [...(REAL ? [] : ["--loopback"]), "--port", String(port), "--data-dir", dataDir],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  proc.stdout.on("data", (d) => process.stdout.write(`[${label}] ${d}`));
  proc.stderr.on("data", (d) => process.stderr.write(`[${label}] ${d}`));
  proc.on("exit", (code, signal) => {
    if (!tearingDown) fail(`daemon ${label} exited early (code=${code} signal=${signal})`);
  });
  const daemon = { label, port, proc, dataDir };
  daemons.push(daemon);
  return daemon;
}

class Client {
  constructor(label) {
    this.label = label;
    this.nextId = 1;
    this.pending = new Map();
    /** Every push frame received, in order. */
    this.pushes = [];
    this.ws = null;
  }

  async connect(port, deadlineMs = 60_000) {
    const start = Date.now();
    for (;;) {
      // Recomputed per attempt: the portfile (with the auth token) appears
      // when the daemon is ready, and early attempts race it.
      const url = wsUrlFor(port, dataDirByPort.get(port));
      try {
        const ws = new WebSocket(url);
        await new Promise((res, rej) => {
          ws.onopen = () => res();
          ws.onerror = () => rej(new Error("connect failed"));
        });
        this.ws = ws;
        ws.onmessage = (event) => {
          const frame = JSON.parse(String(event.data));
          if (frame.push) {
            this.pushes.push(frame);
            return;
          }
          const waiter = this.pending.get(frame.id);
          if (waiter) {
            this.pending.delete(frame.id);
            waiter(frame);
          }
        };
        ws.onclose = () => {
          if (!tearingDown) fail(`${this.label}: websocket closed unexpectedly`);
        };
        // The query parameter carries the per-start bearer token. Never echo
        // it into terminal or CI logs; the loopback endpoint is sufficient
        // operational evidence that the authenticated connection succeeded.
        console.log(`e2e: ${this.label} connected to ws://127.0.0.1:${port}/ws (authenticated)`);
        return;
      } catch {
        if (Date.now() - start > deadlineMs) fail(`could not connect to ${url}`);
        await sleep(250);
      }
    }
  }

  /** Send one request; resolve with the raw response frame. */
  callRaw(method, params = {}, timeoutMs = 60_000) {
    const id = this.nextId++;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((res, rej) => {
      const timer = setTimeout(
        () => rej(new Error(`${this.label}: ${method} timed out after ${timeoutMs}ms`)),
        timeoutMs,
      );
      this.pending.set(id, (frame) => {
        clearTimeout(timer);
        res(frame);
      });
    });
  }

  /** Call a method that must succeed; returns `result`. */
  async call(method, params = {}, timeoutMs = 60_000) {
    const frame = await this.callRaw(method, params, timeoutMs);
    if (frame.ok !== true) {
      fail(
        `${this.label}: ${method} errored: ${JSON.stringify(frame.error)} (params ${JSON.stringify(params)})`,
      );
    }
    return frame.result;
  }

  roomEventPushes(roomId) {
    return this.pushes.filter(
      (p) => p.push === "room.event" && p.data.room_id === roomId,
    );
  }
}

// ---------------------------------------------------------------------------
// The flow
// ---------------------------------------------------------------------------

console.log(`e2e: network mode = ${MODE}`);
console.log("e2e: building the workspace (cargo build --workspace)");
execFileSync("cargo", ["build", "--workspace"], { cwd: repoRoot, stdio: "inherit" });

startDaemon("A", PORT_A);
startDaemon("B", PORT_B);
const a = new Client("A");
const b = new Client("B");
await a.connect(PORT_A);
await b.connect(PORT_B);

try {
  // ---- a. daemon.status + identity.create on both ------------------------
  for (const [c, name] of [
    [a, "A"],
    [b, "B"],
  ]) {
    const status = await c.call("daemon.status");
    assert(status.mode === MODE, `${name}: daemon.status mode is ${MODE}`);
    assert(status.identity === null, `${name}: fresh daemon has no identity`);
    assert(
      Array.isArray(status.rooms_open) && status.rooms_open.length === 0,
      `${name}: fresh daemon has no open rooms`,
    );
  }
  const idA = await a.call("identity.create");
  const idB = await b.call("identity.create");
  assert(/^[0-9a-f]{64}$/.test(idA.identity_id), "A: identity_id is 64-hex");
  assert(/^[0-9a-f]{64}$/.test(idB.identity_id), "B: identity_id is 64-hex");
  assert(idA.identity_id !== idB.identity_id, "A and B have distinct identities");

  // ---- b. A creates + opens the room -------------------------------------
  const { room_id: roomId } = await a.call("room.create", {
    name: "Build Iroh Rooms MVP",
  });
  assert(roomId.startsWith("blake3:"), "A: room.create returns a blake3: room_id");

  // room.open is idempotent. In real mode the endpoint's dialable socket
  // addrs come from live net discovery and can land a beat after the first
  // open returns, so re-poll the same call until the addr is populated
  // (loopback: the first call already carries it).
  const opened = await pollUntil(
    async () => {
      const o = await a.call("room.open", { room_id: roomId });
      return typeof o.endpoint?.addr === "string" && o.endpoint.addr.includes("@")
        ? o
        : null;
    },
    30_000,
    "A's room.open to report a dialable addr",
  );
  assert(
    typeof opened.endpoint.endpoint_id === "string",
    "A: room.open returns the endpoint id",
  );
  const addrA = opened.endpoint.addr;
  assert(
    typeof addrA === "string" && addrA.includes("@"),
    `A: room.open returns a dialable addr (${addrA})`,
  );
  assert(
    opened.timeline.length === 1 && opened.timeline[0].kind === "room_created",
    "A: fresh room timeline is exactly [room_created]",
  );
  const roomsA = await a.call("room.list");
  const listedA = roomsA.rooms.find((r) => r.room_id === roomId);
  assert(
    listedA && listedA.name === "Build Iroh Rooms MVP" && listedA.open === true,
    "A: room.list shows the room by name, open",
  );

  // ---- c. invite + join ---------------------------------------------------
  const { ticket } = await a.call("invite.create", {
    room_id: roomId,
    identity_id: idB.identity_id,
    role: "member",
  });
  assert(typeof ticket === "string" && ticket.length > 0, "A: invite.create returns a ticket");

  const joined = await b.call("room.join", { ticket, peers: [addrA] }, 90_000);
  assert(joined.room_id === roomId, "B: room.join lands in A's room");

  // A must observe member_joined for B — push preferred, timeline poll bounds it.
  await pollUntil(
    async () => {
      const pushed = a
        .roomEventPushes(roomId)
        .some(
          (p) =>
            p.data.event.kind === "member_joined" &&
            p.data.event.member?.identity_id === idB.identity_id,
        );
      if (pushed) return true;
      const { events } = await a.call("room.timeline", { room_id: roomId });
      return events.some(
        (e) => e.kind === "member_joined" && e.member?.identity_id === idB.identity_id,
      );
    },
    30_000,
    "A to see member_joined for B",
  );
  assert(true, "A sees member_joined for B (push or timeline)");
  const membersA = await a.call("room.members", { room_id: roomId });
  const bRow = membersA.members.find((m) => m.identity_id === idB.identity_id);
  assert(
    bRow && bRow.role === "member" && bRow.status === "active",
    "A: room.members shows B as an active member",
  );

  // B opens the room (its own live session; hints persisted from the join).
  const openedB = await b.call("room.open", { room_id: roomId });
  assert(
    openedB.timeline.some((e) => e.kind === "room_created"),
    "B: room.open timeline contains the synced room_created",
  );

  // ---- d. messages both ways ----------------------------------------------
  const bodyA = "hello from A — the room owner";
  const bodyB = "hello from B — the invited member";
  const msgA = await a.call("message.send", { room_id: roomId, body: bodyA });
  assert(/^[0-9a-f]{64}$/.test(msgA.event_id), "A: message.send returns a 64-hex event_id");
  const msgB = await b.call("message.send", { room_id: roomId, body: bodyB });
  assert(/^[0-9a-f]{64}$/.test(msgB.event_id), "B: message.send returns a 64-hex event_id");

  for (const [c, name] of [
    [a, "A"],
    [b, "B"],
  ]) {
    const events = await pollUntil(
      async () => {
        const { events } = await c.call("room.timeline", { room_id: roomId });
        const gotA = events.some((e) => e.kind === "message" && e.body === bodyA);
        const gotB = events.some((e) => e.kind === "message" && e.body === bodyB);
        return gotA && gotB ? events : null;
      },
      30_000,
      `${name}'s timeline to show both messages`,
    );
    const evA = events.find((e) => e.kind === "message" && e.body === bodyA);
    const evB = events.find((e) => e.kind === "message" && e.body === bodyB);
    assert(
      evA.sender.identity_id === idA.identity_id,
      `${name}: A's message is attributed to A`,
    );
    assert(
      evB.sender.identity_id === idB.identity_id,
      `${name}: B's message is attributed to B`,
    );
    assert(evA.event_id === msgA.event_id, `${name}: A's message keeps its event_id`);
    assert(evB.event_id === msgB.event_id, `${name}: B's message keeps its event_id`);
  }

  // ---- e. file share + verified fetch --------------------------------------
  const fileBytes = Buffer.concat([
    Buffer.from("jeliya e2e payload\n"),
    randomBytes(128 * 1024),
  ]);
  const filePath = join(daemons[0].dataDir, "e2e-shared.bin");
  writeFileSync(filePath, fileBytes);
  const shared = await a.call(
    "file.share",
    { room_id: roomId, path: filePath, name: "e2e-shared.bin", mime: "application/octet-stream" },
    120_000,
  );
  assert(/^file_[0-9a-f]{32}$/.test(shared.file_id), "A: file.share returns a file_ id");
  assert(/^[0-9a-f]{64}$/.test(shared.event_id), "A: file.share returns the event_id");

  const fileRow = await pollUntil(
    async () => {
      const { files } = await b.call("file.list", { room_id: roomId });
      const row = files.find((f) => f.file_id === shared.file_id);
      return row && row.available === true ? row : null;
    },
    60_000,
    "B's file.list to show the shared file as available",
  );
  assert(fileRow.name === "e2e-shared.bin", "B: file.list carries the shared name");
  assert(fileRow.size === fileBytes.length, "B: file.list carries the true size");
  assert(fileRow.sender_id === idA.identity_id, "B: file.list attributes the file to A");
  assert(fileRow.providers >= 1, "B: file.list reports at least one provider");

  const fetched = await b.call(
    "file.fetch",
    { room_id: roomId, file_id: shared.file_id },
    120_000,
  );
  assert(fetched.verified === true, "B: file.fetch reports verified:true");
  assert(fetched.bytes === fileBytes.length, "B: file.fetch reports the true byte count");
  const roundTripped = readFileSync(fetched.path);
  assert(
    roundTripped.equals(fileBytes),
    "B: fetched file is byte-identical to what A shared",
  );

  // ---- f. agent status ------------------------------------------------------
  const posted = await b.call("status.post", {
    room_id: roomId,
    label: "running_tests",
    message: "e2e status check",
    progress: 60,
  });
  assert(/^[0-9a-f]{64}$/.test(posted.event_id), "B: status.post returns an event_id");
  const statusEv = await pollUntil(
    async () => {
      const { events } = await a.call("room.timeline", { room_id: roomId });
      return events.find((e) => e.kind === "agent_status" && e.event_id === posted.event_id);
    },
    30_000,
    "A's timeline to show B's agent_status",
  );
  assert(statusEv.label === "running_tests", "A: agent_status label survives");
  assert(statusEv.progress === 60, "A: agent_status progress survives");
  assert(statusEv.status_message === "e2e status check", "A: agent_status message survives");
  assert(
    statusEv.sender.identity_id === idB.identity_id,
    "A: agent_status is attributed to B",
  );

  // ---- g. live pipe -----------------------------------------------------------
  const pipeBody = `jeliya pipe demo ${randomBytes(8).toString("hex")}`;
  httpServer = createServer((_req, res) => {
    res.writeHead(200, { "content-type": "text/plain" });
    res.end(pipeBody);
  });
  await new Promise((res) => httpServer.listen(0, "127.0.0.1", res));
  const targetPort = httpServer.address().port;

  const exposed = await a.call("pipe.expose", {
    room_id: roomId,
    target: `127.0.0.1:${targetPort}`,
    peer_identity: idB.identity_id,
  });
  assert(/^[0-9a-f]{32}$/.test(exposed.pipe_id), "A: pipe.expose returns a 32-hex pipe_id");
  assert(/^[0-9a-f]{64}$/.test(exposed.event_id), "A: pipe.expose returns the event_id");

  const bPipeRow = await pollUntil(
    async () => {
      const { pipes } = await b.call("pipe.list", { room_id: roomId });
      return pipes.find((p) => p.pipe_id === exposed.pipe_id && p.state === "open");
    },
    30_000,
    "B's pipe.list to show the open pipe",
  );
  assert(
    bPipeRow.authorized_peer === idB.identity_id,
    "B: pipe.list shows B as the single authorized peer",
  );
  assert(bPipeRow.opened_by === idA.identity_id, "B: pipe.list attributes the pipe to A");

  const { local_addr } = await b.call(
    "pipe.connect",
    { room_id: roomId, pipe_id: exposed.pipe_id },
    60_000,
  );
  assert(
    /^127\.0\.0\.1:\d+$/.test(local_addr),
    `B: pipe.connect returns a loopback local_addr (${local_addr})`,
  );
  const resp = await fetch(`http://${local_addr}/`, { signal: AbortSignal.timeout(20_000) });
  assert(resp.status === 200, "B: HTTP GET through the pipe returns 200");
  const gotBody = await resp.text();
  assert(gotBody === pipeBody, "B: HTTP body through the pipe matches the served body");

  const closedPipe = await a.call("pipe.close", {
    room_id: roomId,
    pipe_id: exposed.pipe_id,
  });
  assert(/^[0-9a-f]{64}$/.test(closedPipe.event_id), "A: pipe.close returns an event_id");
  await pollUntil(
    async () => {
      const { pipes } = await b.call("pipe.list", { room_id: roomId });
      const row = pipes.find((p) => p.pipe_id === exposed.pipe_id);
      return row && row.state === "closed";
    },
    30_000,
    "B's pipe.list to show the pipe closed",
  );
  assert(true, "B: pipe.list shows the pipe closed after A's pipe.close");

  // ---- h. push discipline -------------------------------------------------
  // Let the ~300ms push loops settle, then require: every room.event push id
  // is unique per client (exactly once), and each flow event above reached
  // both clients as a push exactly once (each postdates both room.opens,
  // except member_joined which predates B's open and A's invite which
  // predates nothing — checked on A only).
  await sleep(3_000);
  const flowEvents = [
    ["A's message", msgA.event_id, ["A", "B"]],
    ["B's message", msgB.event_id, ["A", "B"]],
    ["file.shared", shared.event_id, ["A", "B"]],
    ["agent_status", posted.event_id, ["A", "B"]],
    ["pipe.opened", exposed.event_id, ["A", "B"]],
    ["pipe.closed", closedPipe.event_id, ["A", "B"]],
  ];
  for (const [c, name] of [
    [a, "A"],
    [b, "B"],
  ]) {
    const pushes = c.roomEventPushes(roomId);
    const ids = pushes.map((p) => p.data.event.event_id);
    const dupes = ids.filter((id, i) => ids.indexOf(id) !== i);
    assert(
      dupes.length === 0,
      `${name}: no room.event push is duplicated (${ids.length} pushes, ${new Set(ids).size} distinct)`,
    );
    for (const [what, eventId, clients] of flowEvents) {
      if (!clients.includes(name)) continue;
      const count = ids.filter((id) => id === eventId).length;
      assert(count === 1, `${name}: ${what} arrived as a room.event push exactly once`);
    }
  }
  const joinPushCount = a
    .roomEventPushes(roomId)
    .filter(
      (p) =>
        p.data.event.kind === "member_joined" &&
        p.data.event.member?.identity_id === idB.identity_id,
    ).length;
  assert(joinPushCount === 1, "A: member_joined arrived as a room.event push exactly once");

  console.log(`e2e: PASS — ${assertions} assertions green`);
  teardown();
  process.exit(0);
} catch (err) {
  fail(String(err?.stack ?? err));
}
