#!/usr/bin/env node
// Jeliya daemon smoke test (Node 22+, global WebSocket — no npm deps).
//
// Starts `jeliyad --loopback --port 7431 --data-dir <fresh tmp dir>`, then
// over the WebSocket protocol: daemon.status -> identity.create ->
// room.create -> room.open -> message.send -> room.timeline, asserting the
// timeline shows room_created + the message with the correct kinds.
//
// Usage: node scripts/smoke.mjs [path-to-jeliyad]
//   (default binary: target/debug/jeliyad, relative to the repo root)

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { wsUrlFor } from "./daemon-token.mjs";

const PORT = 7431;
const URL = `ws://127.0.0.1:${PORT}/ws`;
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const binary = process.argv[2] ?? join(repoRoot, "target", "debug", "jeliyad");

if (!existsSync(binary)) {
  console.error(`smoke: daemon binary not found at ${binary}`);
  console.error("smoke: run `cargo build --workspace` first (or pass the path)");
  process.exit(1);
}

const dataDir = mkdtempSync(join(tmpdir(), "jeliya-smoke-"));
console.log(`smoke: data dir ${dataDir}`);
console.log(`smoke: starting ${binary} --loopback --port ${PORT}`);

const daemon = spawn(
  binary,
  ["--loopback", "--port", String(PORT), "--data-dir", dataDir],
  { stdio: ["ignore", "pipe", "pipe"] },
);
daemon.stdout.on("data", (d) => process.stdout.write(`[daemon] ${d}`));
daemon.stderr.on("data", (d) => process.stderr.write(`[daemon] ${d}`));

let exiting = false;
daemon.on("exit", (code, signal) => {
  if (!exiting) {
    console.error(`smoke: FAIL — daemon exited early (code=${code} signal=${signal})`);
    process.exit(1);
  }
});

function fail(msg) {
  console.error(`smoke: FAIL — ${msg}`);
  cleanup(1);
}

function cleanup(code) {
  exiting = true;
  try {
    daemon.kill("SIGKILL");
  } catch {}
  try {
    rmSync(dataDir, { recursive: true, force: true });
  } catch {}
  process.exit(code);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function connectWithRetry(deadlineMs) {
  const start = Date.now();
  for (;;) {
    // Recomputed per attempt: the portfile (with the auth token) appears when
    // the daemon is ready, and early attempts race it.
    const url = wsUrlFor(PORT, dataDir);
    try {
      const ws = new WebSocket(url);
      await new Promise((resolveOpen, rejectOpen) => {
        ws.onopen = () => resolveOpen();
        ws.onerror = (e) => rejectOpen(new Error("connect failed"));
      });
      return ws;
    } catch {
      if (Date.now() - start > deadlineMs) {
        fail(`could not connect to ${url} within ${deadlineMs}ms`);
      }
      await sleep(250);
    }
  }
}

const ws = await connectWithRetry(60_000);
console.log(`smoke: connected to ${URL}`);

let nextId = 1;
const pending = new Map();
const pushes = [];
ws.onmessage = (event) => {
  const frame = JSON.parse(String(event.data));
  if (frame.push) {
    pushes.push(frame);
    return;
  }
  const waiter = pending.get(frame.id);
  if (waiter) {
    pending.delete(frame.id);
    waiter(frame);
  }
};
ws.onclose = () => {
  if (!exiting) fail("websocket closed unexpectedly");
};

function call(method, params = {}, timeoutMs = 30_000) {
  const id = nextId++;
  ws.send(JSON.stringify({ id, method, params }));
  return new Promise((resolveReply, rejectReply) => {
    const timer = setTimeout(
      () => rejectReply(new Error(`${method} timed out after ${timeoutMs}ms`)),
      timeoutMs,
    );
    pending.set(id, (frame) => {
      clearTimeout(timer);
      resolveReply(frame);
    });
  });
}

function assert(cond, msg) {
  if (!cond) fail(msg);
  console.log(`smoke: ok — ${msg}`);
}

try {
  // 1. daemon.status
  let r = await call("daemon.status");
  assert(r.ok === true, "daemon.status responds ok");
  assert(r.result.mode === "loopback", "daemon runs in loopback mode");
  assert(r.result.identity === null, "fresh daemon has no identity yet");
  assert(Array.isArray(r.result.rooms_open) && r.result.rooms_open.length === 0,
    "fresh daemon has no open rooms");

  // 2. identity.create
  r = await call("identity.create");
  assert(r.ok === true, "identity.create responds ok");
  assert(/^[0-9a-f]{64}$/.test(r.result.identity_id), "identity_id is 64-hex");
  assert(/^[0-9a-f]{64}$/.test(r.result.device_id), "device_id is 64-hex");

  // 2b. identity.create again must be identity_exists
  r = await call("identity.create");
  assert(r.ok === false && r.error.code === "identity_exists",
    "second identity.create errors identity_exists");

  // 3. room.create
  r = await call("room.create", { name: "Smoke Room" });
  assert(r.ok === true, "room.create responds ok");
  const roomId = r.result.room_id;
  assert(typeof roomId === "string" && roomId.startsWith("blake3:"),
    "room_id has the blake3: form");

  // 4. room.open
  r = await call("room.open", { room_id: roomId }, 60_000);
  assert(r.ok === true, "room.open responds ok");
  assert(typeof r.result.endpoint.endpoint_id === "string",
    "room.open returns the endpoint id");
  assert(Array.isArray(r.result.members) && r.result.members.length === 1,
    "room.open returns the single-owner roster");
  assert(r.result.timeline.length === 1 && r.result.timeline[0].kind === "room_created",
    "room.open timeline starts with room_created");

  // 5. message.send
  r = await call("message.send", { room_id: roomId, body: "hello jeliya" });
  assert(r.ok === true, "message.send responds ok");
  assert(/^[0-9a-f]{64}$/.test(r.result.event_id), "message event_id is 64-hex");

  // 6. room.timeline shows room_created + the message with correct kinds
  r = await call("room.timeline", { room_id: roomId });
  assert(r.ok === true, "room.timeline responds ok");
  const kinds = r.result.events.map((e) => e.kind);
  assert(kinds[0] === "room_created", "timeline[0] is room_created");
  assert(kinds.includes("message"), "timeline includes the message");
  const msg = r.result.events.find((e) => e.kind === "message");
  assert(msg.body === "hello jeliya", "message body round-trips");
  assert(msg.sender && /^[0-9a-f]{64}$/.test(msg.sender.identity_id),
    "message sender is attributed");

  // Bonus (non-fatal): the push loop should also have delivered the message
  // exactly once as room.event.
  await sleep(800);
  const msgPushes = pushes.filter(
    (p) => p.push === "room.event" && p.data.event.kind === "message",
  );
  console.log(`smoke: note — received ${msgPushes.length} room.event message push(es)`);

  console.log("smoke: PASS");
  ws.close();
  cleanup(0);
} catch (err) {
  fail(String(err?.stack ?? err));
}
