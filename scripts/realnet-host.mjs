#!/usr/bin/env node
// Machine A (host) side of the cross-NAT real-network test.
// See docs/realnet-runbook.md. Node 22+, no npm deps.
//
// Spawns a REAL-mode jeliyad (no --loopback), ensures an identity, creates a
// FRESH room, opens it, mints an invite bound to machine B's identity, prints
// exactly what to paste on B, then waits and reports:
//   - B's member_joined
//   - B's hello message and B's final PASS message
//   - A-side peers.status path (direct vs relay) — the NAT-traversal evidence
// It also sends A's hello and shares a random payload file for B to fetch.
//
// Usage:
//   node scripts/realnet-host.mjs --peer-identity <B_IDENTITY_64HEX>
//     [--port 7431] [--data-dir .jeliya-realnet-host]
//     [--room-name "Realnet NAT test"] [--wait-mins 15]

import { randomBytes } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

import {
  CHECK_HELLO,
  CHECK_PASS_PREFIX,
  Client,
  HOST_HELLO,
  PAYLOAD_NAME,
  ensureIdentity,
  openRoomWithAddr,
  parseArgs,
  pollUntil,
  reportPeers,
  settledPeers,
  sleep,
  startRealDaemon,
  timeline,
} from "./realnet-lib.mjs";

const args = parseArgs(process.argv.slice(2));
const PORT = Number(args.port ?? 7431);
const DATA_DIR = resolve(String(args["data-dir"] ?? ".jeliya-realnet-host"));
const ROOM_NAME = String(args["room-name"] ?? "Realnet NAT test");
const WAIT_MS = Number(args["wait-mins"] ?? 15) * 60_000;
const PEER_IDENTITY = args["peer-identity"];

if (typeof PEER_IDENTITY !== "string" || !/^[0-9a-f]{64}$/.test(PEER_IDENTITY)) {
  console.error(
    "usage: node scripts/realnet-host.mjs --peer-identity <B_IDENTITY_64HEX>\n" +
      "       (get it by running `node scripts/realnet-check.mjs --identity-only` on machine B)",
  );
  process.exit(2);
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
  label: "A",
  onExit: (code, signal) => {
    if (!done) {
      console.error(`host: daemon exited early (code=${code} signal=${signal})`);
      process.exit(1);
    }
  },
});
client = new Client("host");

try {
  await client.connect(PORT);
  const me = await ensureIdentity(client);
  console.log(`host: identity ${me.identity_id}`);

  // Always a FRESH room: re-runs never trip over an already-redeemed invite.
  const { room_id: roomId } = await client.call("room.create", { name: ROOM_NAME });
  const openedFirst = await openRoomWithAddr(client, roomId);
  // Give net discovery a moment to also learn the publicly observed socket
  // addr (helps cross-NAT direct dialing), then re-read the addr.
  await sleep(3_000);
  const opened = await openRoomWithAddr(client, roomId);
  const addr = opened.endpoint.addr ?? openedFirst.endpoint.addr;
  const { ticket } = await client.call("invite.create", {
    room_id: roomId,
    identity_id: PEER_IDENTITY,
    role: "member",
  });

  console.log("");
  console.log("host: ================= PASTE ON MACHINE B =================");
  console.log(`node scripts/realnet-check.mjs --ticket '${ticket}' --peer '${addr}'`);
  console.log("host: =======================================================");
  console.log(`host: room_id  ${roomId}`);
  console.log(`host: endpoint ${opened.endpoint.endpoint_id}`);
  console.log(`host: addr     ${addr}`);
  console.log("");

  console.log(`host: waiting up to ${WAIT_MS / 60_000} min for B to join...`);
  await pollUntil(
    async () =>
      (await timeline(client, roomId)).some(
        (e) => e.kind === "member_joined" && e.member?.identity_id === PEER_IDENTITY,
      ),
    WAIT_MS,
    "B's member_joined",
    1_000,
  );
  console.log("host: OK — B joined (member_joined in A's timeline)");

  await client.call("message.send", { room_id: roomId, body: HOST_HELLO });
  console.log("host: sent A->B hello message");

  const payloadPath = join(DATA_DIR, PAYLOAD_NAME);
  writeFileSync(
    payloadPath,
    Buffer.concat([Buffer.from("jeliya realnet payload\n"), randomBytes(256 * 1024)]),
  );
  const sharedFile = await client.call(
    "file.share",
    { room_id: roomId, path: payloadPath, name: PAYLOAD_NAME, mime: "application/octet-stream" },
    120_000,
  );
  console.log(`host: shared ${PAYLOAD_NAME} (${sharedFile.file_id}) — B will fetch it`);

  await pollUntil(
    async () =>
      (await timeline(client, roomId)).some(
        (e) => e.kind === "message" && e.body === CHECK_HELLO,
      ),
    WAIT_MS,
    "B's hello message",
    1_000,
  );
  console.log("host: OK — received B->A hello message");

  const passEv = await pollUntil(
    async () =>
      (await timeline(client, roomId)).find(
        (e) => e.kind === "message" && e.body?.startsWith(CHECK_PASS_PREFIX),
      ),
    WAIT_MS,
    "B's final PASS message",
    1_000,
  );
  console.log(`host: OK — B reports: ${passEv.body}`);

  const peers = await settledPeers(client, roomId);
  const verdict = reportPeers("host", peers);

  console.log("");
  console.log(
    `host: PASS — join + message both ways + file share confirmed; A-side path = ${verdict ?? "unknown"}`,
  );
  teardown();
  process.exit(0);
} catch (err) {
  console.error(`host: FAIL — ${err?.stack ?? err}`);
  teardown();
  process.exit(1);
}
