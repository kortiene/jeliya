#!/usr/bin/env node
// Phase 0 exit gate: verify the process-supervision contract end-to-end
// against a real jeliyad binary (docs/PROTOCOL.md, "Process supervision").
//
//   node scripts/sidecar-check.mjs [path/to/jeliyad]
//
// Checks, in order:
//   1. spawn --supervised --port 0 → one `ready` JSON line on stdout with the
//      real bound port; portfile exists, parses, and carries the auth token
//   2. /api/health answers without auth and identifies pid + data dir
//   3. /ws refuses a token-less connect (401) and accepts a tokened one
//      (authenticated daemon.status returns matching pid/port/data_dir)
//   4. a second spawn on the same data dir prints `already_running` with the
//      first daemon's pid and exits 0 (single instance + adoption contract)
//   5. forced port collision on an explicit port scans upward and reports the
//      truth in the ready line
//   6. SIGTERM → clean exit and the portfile is removed
//   7. --supervised parent death: closing stdin makes the daemon exit on its
//      own (the orphan test) and release the lock so a fresh spawn recovers
//   8. kill -9 → the lock dies with the process; a fresh spawn acquires the
//      same data dir immediately (stale portfile is ignored via health check)
//
// Node 22+ (global WebSocket, fetch). Loopback network mode; no npm deps.

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createServer, Socket } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const BINARY = process.argv[2] ?? join(repoRoot, "target", "debug", "jeliyad");

if (!existsSync(BINARY)) {
  console.error(`sidecar-check: binary not found at ${BINARY}`);
  console.error("sidecar-check: run `cargo build --workspace` first (or pass the path)");
  process.exit(1);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const children = [];
const dataDirs = [];
let failures = 0;

function cleanup() {
  for (const child of children) {
    try {
      child.kill("SIGKILL");
    } catch {}
  }
  for (const dir of dataDirs) {
    try {
      rmSync(dir, { recursive: true, force: true });
    } catch {}
  }
}
process.on("exit", cleanup);

function ok(name) {
  console.log(`  ok  ${name}`);
}
function fail(name, detail) {
  failures += 1;
  console.error(`FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
}

function freshDataDir(label) {
  const dir = mkdtempSync(join(tmpdir(), `jeliya-sidecar-${label}-`));
  dataDirs.push(dir);
  return dir;
}

/** Spawn jeliyad and resolve with the first parsed JSON line from stdout. */
function spawnDaemon(args, { label, keepStdinOpen = true } = {}) {
  const child = spawn(BINARY, args, { stdio: ["pipe", "pipe", "pipe"] });
  children.push(child);
  child.stderr.on("data", (d) => {
    if (process.env.SIDECAR_VERBOSE) process.stderr.write(`[${label}] ${d}`);
  });
  if (!keepStdinOpen) child.stdin.end();
  const firstJsonLine = new Promise((resolveLine, rejectLine) => {
    let buffer = "";
    const timer = setTimeout(
      () => rejectLine(new Error("no JSON line on stdout within 15s")),
      15_000,
    );
    child.stdout.on("data", (chunk) => {
      buffer += String(chunk);
      for (const line of buffer.split("\n")) {
        const trimmed = line.trim();
        if (!trimmed.startsWith("{")) continue;
        try {
          const parsed = JSON.parse(trimmed);
          clearTimeout(timer);
          resolveLine(parsed);
          return;
        } catch {}
      }
    });
    child.on("exit", (code) => {
      // A contract line may still have been emitted; give the reader a beat.
      setTimeout(() => rejectLine(new Error(`exited (code=${code}) before a JSON line`)), 250);
    });
  });
  return { child, firstJsonLine };
}

function readPortfile(dataDir) {
  return JSON.parse(readFileSync(join(dataDir, "daemon.json"), "utf8"));
}

/** Raw HTTP GET so we can forge headers `fetch` forbids (notably Host). Returns
 *  the numeric status code, or 0 on transport failure. */
function rawStatus(port, path, headers = {}) {
  return new Promise((resolveStatus) => {
    const socket = new Socket();
    let buf = "";
    const done = (code) => {
      socket.destroy();
      resolveStatus(code);
    };
    const timer = setTimeout(() => done(0), 4000);
    socket.connect(port, "127.0.0.1", () => {
      const lines = [`GET ${path} HTTP/1.1`, ...Object.entries(headers).map(([k, v]) => `${k}: ${v}`), "Connection: close", "", ""];
      socket.write(lines.join("\r\n"));
    });
    socket.on("data", (d) => {
      buf += String(d);
      const m = buf.match(/^HTTP\/1\.[01] (\d{3})/);
      if (m) {
        clearTimeout(timer);
        done(Number(m[1]));
      }
    });
    socket.on("error", () => {
      clearTimeout(timer);
      done(0);
    });
    socket.on("close", () => {
      clearTimeout(timer);
      resolveStatus(0);
    });
  });
}

function wsRoundtrip(url, method = "daemon.status") {
  return new Promise((resolveWs, rejectWs) => {
    const ws = new WebSocket(url);
    const timer = setTimeout(() => {
      ws.close();
      rejectWs(new Error("ws timeout"));
    }, 5_000);
    ws.onopen = () => ws.send(JSON.stringify({ id: 1, method, params: {} }));
    ws.onmessage = (event) => {
      const frame = JSON.parse(String(event.data));
      if (frame.push) return;
      clearTimeout(timer);
      ws.close();
      resolveWs(frame);
    };
    ws.onerror = () => {
      clearTimeout(timer);
      rejectWs(new Error("ws connect/handshake failed"));
    };
  });
}

function waitForExit(child, timeoutMs) {
  return new Promise((resolveExit) => {
    if (child.exitCode !== null || child.signalCode !== null) {
      resolveExit({ code: child.exitCode, signal: child.signalCode });
      return;
    }
    const timer = setTimeout(() => resolveExit(null), timeoutMs);
    child.once("exit", (code, signal) => {
      clearTimeout(timer);
      resolveExit({ code, signal });
    });
  });
}

console.log(`sidecar-check: ${BINARY}`);

// --- 1+2+3: ready contract, health, token gate ------------------------------
{
  const dataDir = freshDataDir("main");
  const { child, firstJsonLine } = spawnDaemon(
    ["--supervised", "--loopback", "--port", "0", "--data-dir", dataDir],
    { label: "main" },
  );
  let ready;
  try {
    ready = await firstJsonLine;
  } catch (err) {
    fail("ready line", err.message);
    process.exit(1);
  }
  if (ready.event === "ready" && Number.isInteger(ready.port) && ready.port > 0) {
    ok(`ready line carries the OS-assigned port (${ready.port})`);
  } else {
    fail("ready line", JSON.stringify(ready));
  }
  if (ready.pid === child.pid) ok("ready line pid matches the process");
  else fail("ready pid", `${ready.pid} != ${child.pid}`);

  let portfile;
  try {
    portfile = readPortfile(dataDir);
  } catch (err) {
    fail("portfile", err.message);
    process.exit(1);
  }
  if (
    portfile.schema === 1 &&
    portfile.port === ready.port &&
    portfile.pid === ready.pid &&
    typeof portfile.auth_token === "string" &&
    portfile.auth_token.length === 64
  ) {
    ok("portfile parses with matching pid/port and a 64-hex token");
  } else {
    fail("portfile shape", JSON.stringify({ ...portfile, auth_token: "<redacted>" }));
  }

  const health = await fetch(`http://127.0.0.1:${ready.port}/api/health`).then((r) => r.json());
  if (health.ok === true && health.pid === ready.pid && health.data_dir && health.protocol === 1) {
    ok("/api/health identifies the daemon without auth");
  } else {
    fail("/api/health", JSON.stringify(health));
  }

  // Token-less /ws must be refused before the WebSocket opens.
  const refused = await wsRoundtrip(`ws://127.0.0.1:${ready.port}/ws`).then(
    () => false,
    () => true,
  );
  if (refused) ok("/ws refuses a token-less connect");
  else fail("/ws token gate", "connected without a token");

  const status = await wsRoundtrip(
    `ws://127.0.0.1:${ready.port}/ws?token=${portfile.auth_token}`,
  );
  if (
    status.ok === true &&
    status.result.pid === ready.pid &&
    status.result.port === ready.port &&
    status.result.protocol === 1
  ) {
    ok("authenticated daemon.status returns pid/port/protocol");
  } else {
    fail("daemon.status", JSON.stringify(status));
  }

  // --- auth surface: /api/session Origin rules, Host gate, /api/files gate ---
  const base = `http://127.0.0.1:${ready.port}`;
  // Node's fetch sends neither Origin nor Sec-Fetch-* → the untrusted shape → 403.
  const noOrigin = await fetch(`${base}/api/session`);
  if (noOrigin.status === 403) ok("/api/session refuses a request with no Origin and no Sec-Fetch-Site");
  else fail("/api/session bare", `expected 403, got ${noOrigin.status}`);
  // A loopback Origin (cross-origin dev shape) → 200 + token.
  const withOrigin = await fetch(`${base}/api/session`, { headers: { Origin: base } });
  const originToken = withOrigin.ok ? (await withOrigin.json()).token : null;
  if (withOrigin.status === 200 && originToken === portfile.auth_token) {
    ok("/api/session serves the token to a loopback Origin");
  } else {
    fail("/api/session Origin", `status ${withOrigin.status}`);
  }
  // A same-origin browser GET shape (Sec-Fetch-Site, no Origin) → 200 + token.
  const sameOrigin = await fetch(`${base}/api/session`, {
    headers: { "Sec-Fetch-Site": "same-origin" },
  });
  const sameToken = sameOrigin.ok ? (await sameOrigin.json()).token : null;
  if (sameOrigin.status === 200 && sameToken === portfile.auth_token) {
    ok("/api/session serves the token to a same-origin browser (Sec-Fetch-Site)");
  } else {
    fail("/api/session Sec-Fetch-Site", `status ${sameOrigin.status}`);
  }
  // DNS-rebinding gate: a non-loopback Host is refused everywhere. `fetch`
  // forbids overriding Host, so forge it over a raw socket. Sanity-check the
  // loopback Host is accepted on the same path first.
  const okHost = await rawStatus(ready.port, "/api/health", { Host: `127.0.0.1:${ready.port}` });
  const evilHost = await rawStatus(ready.port, "/api/health", { Host: "evil.example" });
  if (okHost === 200 && evilHost === 403) ok("/api/* refuses a non-loopback Host header");
  else fail("Host gate", `loopback=${okHost} evil=${evilHost} (want 200/403)`);
  // /api/files/local without a token → 401.
  const noTok = await fetch(`${base}/api/files/local?room_id=x&file_id=y`);
  if (noTok.status === 401) ok("/api/files/local refuses a request with no token");
  else fail("/api/files/local token gate", `expected 401, got ${noTok.status}`);

  // --- 4: second spawn on the same data dir reports already_running --------
  const second = spawnDaemon(
    ["--supervised", "--loopback", "--port", "0", "--data-dir", dataDir],
    { label: "second" },
  );
  let alreadyRunning;
  try {
    alreadyRunning = await second.firstJsonLine;
  } catch (err) {
    fail("already_running line", err.message);
  }
  if (alreadyRunning?.event === "already_running" && alreadyRunning.pid === ready.pid) {
    ok("second spawn reports already_running with the live daemon's pid");
  } else if (alreadyRunning) {
    fail("already_running", JSON.stringify(alreadyRunning));
  }
  const secondExit = await waitForExit(second.child, 5_000);
  if (secondExit?.code === 0) ok("second spawn exits 0 (adoption, not error)");
  else fail("second spawn exit", JSON.stringify(secondExit));

  // --- 6: SIGTERM → clean exit + portfile removed ---------------------------
  child.kill("SIGTERM");
  const exit = await waitForExit(child, 15_000);
  if (exit && exit.code === 0) ok("SIGTERM exits cleanly (code 0)");
  else fail("SIGTERM exit", JSON.stringify(exit));
  if (!existsSync(join(dataDir, "daemon.json"))) ok("portfile removed on shutdown");
  else fail("portfile removal", "daemon.json still present after SIGTERM");
}

// --- 5: forced port collision scans upward and tells the truth --------------
{
  const blocker = createServer();
  await new Promise((r) => blocker.listen(0, "127.0.0.1", r));
  const blockedPort = blocker.address().port;
  const dataDir = freshDataDir("collision");
  const { child, firstJsonLine } = spawnDaemon(
    ["--supervised", "--loopback", "--port", String(blockedPort), "--data-dir", dataDir],
    { label: "collision" },
  );
  try {
    const ready = await firstJsonLine;
    if (ready.event === "ready" && ready.port > blockedPort) {
      ok(`port collision scans upward and reports the real port (${blockedPort} → ${ready.port})`);
    } else {
      fail("collision ready line", JSON.stringify(ready));
    }
  } catch (err) {
    fail("collision spawn", err.message);
  }
  child.kill("SIGKILL");
  blocker.close();
}

// --- daemon.shutdown RPC → clean exit + portfile removed --------------------
{
  const dataDir = freshDataDir("shutdown");
  const { child, firstJsonLine } = spawnDaemon(
    ["--supervised", "--loopback", "--port", "0", "--data-dir", dataDir],
    { label: "shutdown" },
  );
  try {
    const ready = await firstJsonLine;
    const portfile = readPortfile(dataDir);
    const reply = await wsRoundtrip(
      `ws://127.0.0.1:${ready.port}/ws?token=${portfile.auth_token}`,
      "daemon.shutdown",
    );
    if (reply.ok === true && reply.result.shutting_down === true) {
      ok("daemon.shutdown replies { shutting_down: true }");
    } else {
      fail("daemon.shutdown reply", JSON.stringify(reply));
    }
    const exit = await waitForExit(child, 15_000);
    if (exit && exit.code === 0) ok("daemon.shutdown exits cleanly");
    else fail("daemon.shutdown exit", JSON.stringify(exit));
    if (!existsSync(join(dataDir, "daemon.json"))) ok("daemon.shutdown removes the portfile");
    else fail("daemon.shutdown portfile", "still present");
  } catch (err) {
    fail("daemon.shutdown", err.message);
  }
  child.kill("SIGKILL");
}

// --- 7: parent death (stdin EOF) → the daemon exits on its own --------------
{
  const dataDir = freshDataDir("orphan");
  const { child, firstJsonLine } = spawnDaemon(
    ["--supervised", "--loopback", "--port", "0", "--data-dir", dataDir],
    { label: "orphan" },
  );
  try {
    await firstJsonLine;
  } catch (err) {
    fail("orphan spawn", err.message);
  }
  // Simulate parent death: the parent's end of the stdin pipe closes.
  child.stdin.end();
  const exit = await waitForExit(child, 15_000);
  if (exit && exit.code === 0) ok("stdin EOF (parent death) → daemon exits on its own");
  else fail("stdin-EOF exit", JSON.stringify(exit));

  // The data dir must be immediately reusable.
  const revived = spawnDaemon(
    ["--supervised", "--loopback", "--port", "0", "--data-dir", dataDir],
    { label: "revived" },
  );
  try {
    const ready = await revived.firstJsonLine;
    if (ready.event === "ready") ok("data dir reusable after supervised exit");
    else fail("revive after stdin EOF", JSON.stringify(ready));
  } catch (err) {
    fail("revive after stdin EOF", err.message);
  }
  revived.child.kill("SIGKILL");
}

// --- 8: kill -9 → lock released, fresh spawn recovers past stale portfile ---
{
  const dataDir = freshDataDir("kill9");
  const { child, firstJsonLine } = spawnDaemon(
    ["--supervised", "--loopback", "--port", "0", "--data-dir", dataDir],
    { label: "kill9" },
  );
  try {
    await firstJsonLine;
  } catch (err) {
    fail("kill9 spawn", err.message);
  }
  child.kill("SIGKILL");
  await waitForExit(child, 5_000);
  // The portfile is now STALE (kill -9 skips cleanup) — the next spawn must
  // acquire the lock anyway (the OS released it) and overwrite the portfile.
  const revived = spawnDaemon(
    ["--supervised", "--loopback", "--port", "0", "--data-dir", dataDir],
    { label: "kill9-revived" },
  );
  try {
    const ready = await revived.firstJsonLine;
    const portfile = readPortfile(dataDir);
    if (ready.event === "ready" && portfile.pid === ready.pid) {
      ok("kill -9 releases the lock; fresh spawn recovers and rewrites the portfile");
    } else {
      fail("kill9 recovery", JSON.stringify(ready));
    }
  } catch (err) {
    fail("kill9 recovery", err.message);
  }
  revived.child.kill("SIGKILL");
}

if (failures === 0) {
  console.log("sidecar-check: PASS — all supervision-contract checks green");
  process.exit(0);
} else {
  console.error(`sidecar-check: ${failures} check(s) failed`);
  process.exit(1);
}
