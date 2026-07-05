#!/usr/bin/env node
// Machine B (joiner) side of the cross-NAT real-network test.
// See docs/realnet-runbook.md. Node 22+, no npm deps.
//
// Two phases:
//   1. `node scripts/realnet-check.mjs --identity-only`
//      Spawns a REAL-mode jeliyad, ensures an identity, prints the
//      identity_id to hand to machine A (realnet-host.mjs --peer-identity ...).
//   2. `node scripts/realnet-check.mjs --ticket <T> --peer <ADDR>`
//      Joins A's room with the ticket (+ dial addr hint), opens it, and
//      asserts the flow end to end:
//        - member_joined for this identity is visible in B's synced timeline
//          (the host asserts the same on A — both sides covered)
//        - message each way (receives A's hello, sends B's hello)
//        - fetches A's shared payload file: verified:true + size match
//        - prints B-side peers.status path (direct vs relay) — the evidence
//      Then sends a final "realnet-check: PASS ..." message so the host can
//      finish, and exits 0.
//
// Options: [--port 7432] [--data-dir .jeliya-realnet-b] [--wait-mins 15]

import { mkdirSync } from "node:fs";
import { resolve } from "node:path";

import {
  CHECK_HELLO,
  CHECK_PASS_PREFIX,
  Client,
  HOST_HELLO,
  PAYLOAD_NAME,
  ensureIdentity,
  parseArgs,
  pollUntil,
  reportPeers,
  settledPeers,
  sleep,
  startRealDaemon,
  timeline,
} from "./realnet-lib.mjs";

const args = parseArgs(process.argv.slice(2));
const PORT = Number(args.port ?? 7432);
const DATA_DIR = resolve(String(args["data-dir"] ?? ".jeliya-realnet-b"));
const WAIT_MS = Number(args["wait-mins"] ?? 15) * 60_000;
const IDENTITY_ONLY = Boolean(args["identity-only"]);
const TICKET = typeof args.ticket === "string" ? args.ticket : null;
const PEER_ADDR = typeof args.peer === "string" ? args.peer : null;

if (!IDENTITY_ONLY && !TICKET) {
  console.error(
    "usage:\n" +
      "  node scripts/realnet-check.mjs --identity-only\n" +
      "  node scripts/realnet-check.mjs --ticket <TICKET> --peer <ID@IP:PORT,...>\n" +
      "(the exact second command is printed by realnet-host.mjs on machine A)",
  );
  process.exit(2);
}

let checks = 0;
function ok(msg) {
  checks += 1;
  console.log(`check: ok — ${msg}`);
}

let daemon = null;
let client = null;
let done = false;
function teardown() {
  done = true;
  client?.close();
  try {
    daemon?.kill("SIGKILL");
  } catch {}
}
process.on("exit", teardown);
for (const sig of ["SIGINT", "SIGTERM"]) {
  process.on(sig, () => {
    teardown();
    process.exit(1);
  });
}

mkdirSync(DATA_DIR, { recursive: true });
daemon = startRealDaemon({
  port: PORT,
  dataDir: DATA_DIR,
  label: "B",
  onExit: (code, signal) => {
    if (!done) {
      console.error(`check: daemon exited early (code=${code} signal=${signal})`);
      process.exit(1);
    }
  },
});
client = new Client("check");

try {
  await client.connect(PORT);
  const me = await ensureIdentity(client);

  if (IDENTITY_ONLY) {
    console.log("");
    console.log("check: ============== HAND THIS TO MACHINE A ==============");
    console.log(`check: identity_id = ${me.identity_id}`);
    console.log(`node scripts/realnet-host.mjs --peer-identity ${me.identity_id}`);
    console.log("check: ====================================================");
    console.log(`check: identity persisted in ${DATA_DIR} — re-runs reuse it`);
    teardown();
    process.exit(0);
  }

  console.log(`check: identity ${me.identity_id}`);

  // Join with the ticket + A's dial addr. The daemon's per-attempt bootstrap
  // window is 15s (jeliya-core JOIN_TIMEOUT); across a real NAT the first
  // dial can miss it while discovery/relay warm up, so retry a few times.
  const peers = PEER_ADDR ? [PEER_ADDR] : [];
  let roomId = null;
  let lastErr = null;
  for (let attempt = 1; attempt <= 5 && !roomId; attempt += 1) {
    try {
      console.log(`check: room.join attempt ${attempt}...`);
      const joined = await client.call("room.join", { ticket: TICKET, peers }, 120_000);
      roomId = joined.room_id;
    } catch (err) {
      lastErr = err;
      console.log(`check: join attempt ${attempt} failed (${err.message}); retrying`);
      await sleep(2_000);
    }
  }
  if (!roomId) throw lastErr ?? new Error("room.join failed");
  ok(`room.join succeeded (room ${roomId})`);

  await client.call("room.open", { room_id: roomId });
  ok("room.open succeeded on B");

  await pollUntil(
    async () =>
      (await timeline(client, roomId)).some(
        (e) => e.kind === "member_joined" && e.member?.identity_id === me.identity_id,
      ),
    60_000,
    "member_joined for B in B's timeline",
    1_000,
  );
  ok("member_joined for B is visible in B's synced timeline");

  await pollUntil(
    async () =>
      (await timeline(client, roomId)).some(
        (e) => e.kind === "message" && e.body === HOST_HELLO,
      ),
    WAIT_MS,
    "A's hello message",
    1_000,
  );
  ok("received A->B message");

  await client.call("message.send", { room_id: roomId, body: CHECK_HELLO });
  ok("sent B->A message (the host asserts receipt on A)");

  const fileRow = await pollUntil(
    async () => {
      const { files } = await client.call("file.list", { room_id: roomId });
      const row = files.find((f) => f.name === PAYLOAD_NAME);
      return row && row.available === true ? row : null;
    },
    WAIT_MS,
    `${PAYLOAD_NAME} to be listed as available`,
    1_000,
  );
  ok(`file.list shows ${PAYLOAD_NAME} available (${fileRow.size} bytes, from A)`);

  const fetched = await client.call(
    "file.fetch",
    { room_id: roomId, file_id: fileRow.file_id },
    600_000,
  );
  if (fetched.verified !== true) throw new Error("file.fetch did not report verified:true");
  ok("file.fetch verified:true (blake3-verified content from A)");
  if (fetched.bytes !== fileRow.size) {
    throw new Error(`file.fetch bytes ${fetched.bytes} != listed size ${fileRow.size}`);
  }
  ok(`fetched byte count matches the listed size (${fetched.bytes})`);

  const peerRows = await settledPeers(client, roomId);
  const verdict = reportPeers("check", peerRows);

  // Tell the host everything passed (it waits for this before reporting).
  await client.call("message.send", {
    room_id: roomId,
    body: `${CHECK_PASS_PREFIX} — ${checks} checks green on B; B-side path=${verdict ?? "unknown"}`,
  });
  // Leave the daemon up briefly so the PASS message syncs to A before exit.
  await sleep(5_000);

  console.log("");
  console.log(`check: PASS — ${checks} checks green; B-side path = ${verdict ?? "unknown"}`);
  teardown();
  process.exit(0);
} catch (err) {
  console.error(`check: FAIL — ${err?.stack ?? err}`);
  teardown();
  process.exit(1);
}
