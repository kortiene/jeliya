#!/usr/bin/env node
// Gate A: the real-NAT hole-punching test, orchestrated from machine A (this host).
//
// Everything in Jeliya's P2P claims rests on Gate A: two nodes on DIFFERENT
// networks connecting directly (hole-punch) or, failing that, over a relay.
// A same-network run proves nothing — so this wrapper fingerprints both sides'
// public IPs and REFUSES to certify a pass when they share one.
//
// It reuses the existing halves unchanged: scripts/realnet-host.mjs runs here,
// scripts/realnet-check.mjs runs on machine B (shipped as a static binary +
// the two .mjs it needs). Node 22+ (global fetch, no deps).
//
// Usage:
//   node scripts/gate-a.mjs --remote user@<B-host> [--remote-node <path>]
//   node scripts/gate-a.mjs --local-dryrun          # exercise the machinery on one box
//   node scripts/gate-a.mjs --remote ... --manual    # print B's command instead of ssh-ing
//
// Options:
//   --remote <ssh-target>   machine B, reachable by ssh (cloud VM ip, or a host alias)
//   --remote-node <path>    node on B (default: try `node`, then ~/tools/node22/bin/node)
//   --remote-dir <path>     working dir on B (default: ~/jeliya-gatea)
//   --linux-bin <path>      static linux jeliyad to ship (default: the musl release build)
//   --skip-provision        B already has the binary + scripts from a prior run
//   --manual                do not ssh; print the exact command to run on B yourself
//   --local-dryrun          run B as a local subprocess (always same-network: machinery test)
//   --allow-same-network    proceed past the same-NAT guard (dry-runs/testing only)
//   --wait-mins <n>         host wait for B to join (default 10)

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "./realnet-lib.mjs";

const args = parseArgs(process.argv.slice(2));
const REPO = fileURLToPath(new URL("..", import.meta.url));
const SCRATCH = join(REPO, ".jeliya-gatea");
mkdirSync(SCRATCH, { recursive: true });

const LOCAL_DRYRUN = Boolean(args["local-dryrun"]);
const MANUAL = Boolean(args.manual);
const REMOTE = args.remote ? String(args.remote) : null;
const REMOTE_DIR = String(args["remote-dir"] ?? "~/jeliya-gatea");
const REMOTE_NODE = args["remote-node"] ? String(args["remote-node"]) : null;
const LINUX_BIN = String(
  args["linux-bin"] ?? join(REPO, "target/x86_64-unknown-linux-musl/release/jeliyad"),
);
const WAIT_MINS = Number(args["wait-mins"] ?? 10);
const LOCAL_BIN = join(REPO, "target/debug/jeliyad");

const die = (msg) => { console.error(`gate-a: ${msg}`); process.exit(2); };
if (!LOCAL_DRYRUN && !REMOTE && !MANUAL) die("need --remote <ssh-target>, --manual, or --local-dryrun. See --help header.");
if (!existsSync(LOCAL_BIN)) die(`${LOCAL_BIN} missing — run: cargo build --workspace`);

/** Run a command, capture {code, out, err}; never rejects. */
function run(cmd, argv, { onLine } = {}) {
  return new Promise((resolve) => {
    const p = spawn(cmd, argv, { stdio: ["ignore", "pipe", "pipe"] });
    let out = "", err = "";
    p.stdout.setEncoding("utf8");
    p.stderr.setEncoding("utf8");
    let buf = "";
    p.stdout.on("data", (d) => {
      out += d;
      if (!onLine) return;
      buf += d;
      let i;
      while ((i = buf.indexOf("\n")) >= 0) { onLine(buf.slice(0, i), p); buf = buf.slice(i + 1); }
    });
    p.stderr.on("data", (d) => { err += d; });
    p.on("exit", (code) => resolve({ code, out, err, proc: p }));
    p.on("error", (e) => resolve({ code: -1, out, err: err + String(e), proc: p }));
  });
}

const ssh = (remoteCmd) =>
  run("ssh", ["-o", "BatchMode=yes", "-o", "ConnectTimeout=12", REMOTE, remoteCmd]);

/** Public IPv4 / IPv6 as this-or-that side sees them (empty string if none). */
async function fingerprint(where) {
  const v4cmd = "curl -4 -s --max-time 8 https://ifconfig.me || true";
  const v6cmd = "curl -6 -s --max-time 8 https://ifconfig.me || true";
  if (where === "local") {
    const v4 = (await run("bash", ["-c", v4cmd])).out.trim();
    const v6 = (await run("bash", ["-c", v6cmd])).out.trim();
    return { v4, v6 };
  }
  const r = await ssh(`${v4cmd}; echo '|'; ${v6cmd}`);
  const [v4 = "", v6 = ""] = r.out.split("|").map((s) => s.trim());
  return { v4, v6 };
}

const slash64 = (v6) => (v6 && v6.includes(":") ? v6.split(":").slice(0, 4).join(":") + "::/64" : "");

// -> { status: "same" | "different" | "indeterminate", basis }
function netStatus(a, b) {
  if (a.v4 && b.v4) return { status: a.v4 === b.v4 ? "same" : "different", basis: `public IPv4 (A=${a.v4} B=${b.v4})` };
  if (a.v6 && b.v6) return { status: slash64(a.v6) === slash64(b.v6) ? "same" : "different", basis: `IPv6 /64 (A=${slash64(a.v6)} B=${slash64(b.v6)})` };
  return { status: "indeterminate", basis: "no shared IP family observed (is B online with curl?)" };
}

async function provision() {
  if (!existsSync(LINUX_BIN)) die(`${LINUX_BIN} missing — build it:\n  rustup target add x86_64-unknown-linux-musl && cargo zigbuild --release --target x86_64-unknown-linux-musl -p jeliyad`);
  console.log(`gate-a: provisioning ${REMOTE}:${REMOTE_DIR} …`);
  const mk = await ssh(`mkdir -p ${REMOTE_DIR}/scripts`);
  if (mk.code !== 0) die(`cannot ssh/mkdir on ${REMOTE}: ${mk.err.trim()}`);
  const scp = (src, dst) => run("scp", ["-q", "-o", "BatchMode=yes", src, `${REMOTE}:${dst}`]);
  const dir = REMOTE_DIR.replace(/^~\//, "");
  for (const [src, dst] of [
    [LINUX_BIN, `${dir}/jeliyad`],
    [join(REPO, "scripts/realnet-lib.mjs"), `${dir}/scripts/realnet-lib.mjs`],
    [join(REPO, "scripts/realnet-check.mjs"), `${dir}/scripts/realnet-check.mjs`],
  ]) {
    const r = await scp(src, dst);
    if (r.code !== 0) die(`scp ${src} failed: ${r.err.trim()}`);
  }
  await ssh(`chmod +x ${REMOTE_DIR}/jeliyad`);
  // Resolve a working node on B.
  const candidates = REMOTE_NODE ? [REMOTE_NODE] : ["node", "$HOME/tools/node22/bin/node"];
  for (const c of candidates) {
    const v = await ssh(`${c} --version 2>/dev/null || true`);
    const m = v.out.trim().match(/^v(\d+)\./);
    if (m && Number(m[1]) >= 22) { console.log(`gate-a: B node = ${c} (${v.out.trim()})`); return c; }
  }
  die(`no Node >= 22 on ${REMOTE} (tried: ${candidates.join(", ")}). Install Node 22 or pass --remote-node <path>.`);
}

// B-side command builder (remote via ssh, or local subprocess for the dry-run).
const TICKET_RE = /--ticket '([^']+)' --peer '([^']+)'/;
const looksSafe = (s) => /^[A-Za-z0-9@.:,[\]_-]+$/.test(s);

async function bIdentity(remoteNode, bDir) {
  if (LOCAL_DRYRUN) {
    const r = await run("node", ["scripts/realnet-check.mjs", "--identity-only", "--data-dir", bDir], { onLine: (l) => process.stdout.write(`  [B] ${l}\n`) });
    return r.out.match(/identity_id = ([0-9a-f]{64})/)?.[1];
  }
  const r = await ssh(`cd ${REMOTE_DIR} && JELIYAD=${REMOTE_DIR}/jeliyad ${remoteNode} scripts/realnet-check.mjs --identity-only --data-dir ${REMOTE_DIR}/b`);
  process.stdout.write(r.out.replace(/^/gm, "  [B] "));
  return r.out.match(/identity_id = ([0-9a-f]{64})/)?.[1];
}

async function bCheck(remoteNode, bDir, ticket, peer) {
  if (!looksSafe(ticket) || !looksSafe(peer)) die("refusing to forward a ticket/peer with unexpected characters");
  if (LOCAL_DRYRUN) {
    return run("node", ["scripts/realnet-check.mjs", "--ticket", ticket, "--peer", peer, "--data-dir", bDir, "--wait-mins", String(WAIT_MINS)], { onLine: (l) => process.stdout.write(`  [B] ${l}\n`) });
  }
  const r = await ssh(`cd ${REMOTE_DIR} && JELIYAD=${REMOTE_DIR}/jeliyad ${remoteNode} scripts/realnet-check.mjs --ticket '${ticket}' --peer '${peer}' --data-dir ${REMOTE_DIR}/b --wait-mins ${WAIT_MINS}`);
  process.stdout.write(r.out.replace(/^/gm, "  [B] "));
  return r;
}

async function main() {
  console.log(`gate-a: mode = ${LOCAL_DRYRUN ? "local-dryrun" : MANUAL ? "manual" : "remote:" + REMOTE}\n`);

  // 1. Network fingerprints + validity gate.
  const a = await fingerprint("local");
  const b = LOCAL_DRYRUN ? a : MANUAL ? { v4: "", v6: "" } : await fingerprint("remote");
  console.log(`gate-a: A public { v4:${a.v4 || "-"} v6:${a.v6 || "-"} }`);
  console.log(`gate-a: B public { v4:${b.v4 || "-"} v6:${b.v6 || "-"} }`);
  const net = MANUAL ? { status: "manual", basis: "manual mode — you must verify B is on another network" } : netStatus(a, b);
  console.log(`gate-a: network basis: ${net.basis} -> ${net.status.toUpperCase()}`);
  if (net.status === "same" && !args["allow-same-network"] && !LOCAL_DRYRUN) {
    die(`both nodes share a network (${net.basis}). This cannot test NAT traversal.\n` +
        `  Put machine B on a DIFFERENT network (tether this Mac to a phone hotspot, or use a cloud VM), then re-run.\n` +
        `  To exercise the machinery anyway (no Gate A claim), add --local-dryrun or --allow-same-network.`);
  }

  // 2. Provision B (remote only).
  const remoteNode = LOCAL_DRYRUN || MANUAL ? null : args["skip-provision"] ? (REMOTE_NODE ?? "node") : await provision();
  const bDir = join(SCRATCH, "b");
  const aDir = join(SCRATCH, "a");
  if (LOCAL_DRYRUN) { mkdirSync(bDir, { recursive: true }); mkdirSync(aDir, { recursive: true }); }

  // 3. B identity (skip in manual mode — the operator runs --identity-only themselves).
  let bId = args["peer-identity"] ? String(args["peer-identity"]) : null;
  if (!MANUAL && !bId) {
    console.log("gate-a: fetching B identity …");
    bId = await bIdentity(remoteNode, bDir);
    if (!bId) die("could not read B's identity_id");
    console.log(`gate-a: B identity = ${bId}\n`);
  }
  if (MANUAL && !bId) die("manual mode needs --peer-identity <B id> (run `node scripts/realnet-check.mjs --identity-only` on B first)");

  // 4. Start the host here; capture the ticket line it prints.
  console.log("gate-a: starting host (machine A) …");
  let ticket, peer, hostDone;
  const hostP = new Promise((res) => { hostDone = res; });
  const host = run("node", ["scripts/realnet-host.mjs", "--peer-identity", bId, "--data-dir", aDir, "--wait-mins", String(WAIT_MINS)], {
    onLine: (line) => {
      process.stdout.write(`  [A] ${line}\n`);
      const m = line.match(TICKET_RE);
      if (m && !ticket) { [, ticket, peer] = m; kickB(); }
    },
  }).then((r) => hostDone(r));

  let bResultP = Promise.resolve(null);
  let kicked = false;
  function kickB() {
    if (kicked) return; kicked = true;
    if (MANUAL) {
      console.log("\ngate-a: ===== RUN THIS ON MACHINE B (a different network) =====");
      console.log(`node scripts/realnet-check.mjs --ticket '${ticket}' --peer '${peer}'`);
      console.log("gate-a: =======================================================\n");
      return;
    }
    console.log("\ngate-a: driving B (machine B) …");
    bResultP = bCheck(remoteNode, bDir, ticket, peer);
  }

  const [hostRes, bRes] = await Promise.all([hostP, (async () => { await hostP; return bResultP; })()]);

  // 5. Parse verdicts.
  const hostPass = /host: PASS —/.test(hostRes.out);
  const aPath = hostRes.out.match(/A-side path = (\w+)/)?.[1] ?? "unknown";
  const bPass = MANUAL ? null : /check: PASS —/.test((bRes && bRes.out) || "");
  const bPath = MANUAL ? "unknown" : (bRes && bRes.out.match(/B-side path = (\w+)/)?.[1]) ?? "unknown";

  // 6. Gate A verdict.
  let verdict, ok = false;
  if (MANUAL) {
    verdict = `MANUAL — host ${hostPass ? "PASSED" : "did not confirm"} (A-side path=${aPath}). Read B's own output for its verdict.`;
    ok = hostPass;
  } else if (!hostPass || !bPass) {
    verdict = `FAIL — connectivity/assertions did not pass (A=${hostPass ? "pass" : "fail"} B=${bPass ? "pass" : "fail"}).`;
  } else if (net.status === "same") {
    verdict = `NOT A GATE A — assertions passed but both nodes shared a network (${net.basis}); machinery/dry-run only, no hole-punch claim.`;
    ok = LOCAL_DRYRUN; // a dry-run that drove the full flow did its job
  } else if (net.status === "indeterminate") {
    verdict = `UNVERIFIED NETWORK — assertions passed (A path=${aPath}, B path=${bPath}) but B's public IP was never observed, so a DIFFERENT-network run cannot be certified. Confirm B has internet+curl, or verify the networks differ by hand.`;
  } else if (aPath === "direct" && bPath === "direct") {
    verdict = `PASS — direct P2P across different networks. NAT hole-punch CONFIRMED (A=${aPath} B=${bPath}).`;
    ok = true;
  } else {
    verdict = `PARTIAL — connectivity across different networks SUCCEEDED but via RELAY fallback; hole-punch not achieved on this network pair (A=${aPath} B=${bPath}).`;
    ok = true; // connectivity works; honestly not a direct-path pass
  }

  const evidence = {
    when: new Date().toISOString(),
    mode: LOCAL_DRYRUN ? "local-dryrun" : MANUAL ? "manual" : "remote",
    remote: REMOTE, fingerprintA: a, fingerprintB: b, network: net,
    hostPass, aPath, bPass, bPath, verdict,
  };
  const evPath = join(SCRATCH, `gate-a-${Date.now()}.json`);
  writeFileSync(evPath, JSON.stringify(evidence, null, 2));

  console.log(`\n${"=".repeat(64)}\nGATE A: ${verdict}\n${"=".repeat(64)}`);
  console.log(`evidence: ${evPath}`);
  process.exit(ok ? 0 : 1);
}

main().catch((e) => die(e?.stack ?? String(e)));
