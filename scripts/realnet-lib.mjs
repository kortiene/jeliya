// Shared plumbing for the real-network NAT runbook scripts
// (realnet-host.mjs on machine A, realnet-check.mjs on machine B).
//
// Node 22+ only (global WebSocket, no npm deps). Everything here talks to a
// locally spawned `jeliyad` in REAL network mode (no --loopback): the SDK's
// iroh N0 stack with public relays + DNS discovery.

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { defaultDataDir, pipeDaemonOutput, wsUrlFor } from "./daemon-token.mjs";

export const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
// JELIYAD overrides the binary location (e.g. a shipped static binary on a
// machine that never built the workspace).
export const BINARY = process.env.JELIYAD || join(repoRoot, "target", "debug", "jeliyad");

// The tiny cross-machine protocol the two scripts agree on.
export const HOST_HELLO = "realnet-host: hello from A";
export const CHECK_HELLO = "realnet-check: hello from B";
export const CHECK_PASS_PREFIX = "realnet-check: PASS";
export const PAYLOAD_NAME = "realnet-payload.bin";

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Minimal CLI arg parser: --key value / --key=value / bare flags. */
export function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (!arg.startsWith("--")) continue;
    const eq = arg.indexOf("=");
    if (eq !== -1) {
      out[arg.slice(2, eq)] = arg.slice(eq + 1);
    } else if (i + 1 < argv.length && !argv[i + 1].startsWith("--")) {
      out[arg.slice(2)] = argv[i + 1];
      i += 1;
    } else {
      out[arg.slice(2)] = true;
    }
  }
  return out;
}

/** Poll `fn` (may be async; truthy return stops) with a hard deadline. */
export async function pollUntil(fn, timeoutMs, what, intervalMs = 500) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await fn();
    if (value) return value;
    if (Date.now() > deadline) {
      throw new Error(`timed out after ${timeoutMs}ms waiting for ${what}`);
    }
    await sleep(intervalMs);
  }
}

/**
 * Spawn a jeliyad and register teardown. Defaults to real network mode
 * (NO --loopback), which is what the realnet scripts need; pass
 * `loopback: true` for the SDK's offline 127.0.0.1 stack (agent harness/e2e).
 * Returns the child process. The caller's data dir persists across runs on
 * purpose (identity continuity); only the process is cleaned up.
 */
/** Data dir by daemon port, so `Client.connect(port)` can find the portfile
 *  (and its auth token) for a daemon this process spawned. */
const dataDirByPort = new Map();

/** Where `Client.connect` should look for a daemon's portfile. Registered
 *  automatically by `startRealDaemon`; call it directly when attaching to a
 *  daemon spawned some other way. */
export function registerDaemonDataDir(port, dataDir) {
  dataDirByPort.set(port, dataDir);
}

export function startRealDaemon({ port, dataDir, label, onExit, loopback = false }) {
  if (!existsSync(BINARY)) {
    console.error(
      `error: ${BINARY} not found — run \`cargo build --workspace\` first`,
    );
    process.exit(1);
  }
  registerDaemonDataDir(port, dataDir);
  const proc = spawn(
    BINARY,
    [...(loopback ? ["--loopback"] : []), "--port", String(port), "--data-dir", dataDir],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  // pipeDaemonOutput swallows a clean `already_running` adoption so a persistent
  // data dir with an orphaned daemon (a prior SIGKILLed run) is adopted rather
  // than mistaken for an early exit.
  pipeDaemonOutput(proc, label, onExit);
  return proc;
}

/** One WebSocket JSON-RPC client against a local jeliyad. */
export class Client {
  constructor(label) {
    this.label = label;
    this.nextId = 1;
    this.pending = new Map();
    this.pushes = [];
    this.ws = null;
    this.closedByUs = false;
  }

  async connect(port, deadlineMs = 30_000) {
    const start = Date.now();
    for (;;) {
      // Recomputed per attempt: the portfile (carrying the auth token) only
      // appears once the daemon is ready, and early attempts race it.
      const url = wsUrlFor(port, dataDirByPort.get(port) ?? defaultDataDir());
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
          if (!this.closedByUs) {
            console.error(`${this.label}: daemon websocket closed unexpectedly`);
            process.exit(1);
          }
        };
        return;
      } catch {
        if (Date.now() - start > deadlineMs) {
          throw new Error(`could not connect to ${url} within ${deadlineMs}ms`);
        }
        await sleep(250);
      }
    }
  }

  close() {
    this.closedByUs = true;
    try {
      this.ws?.close();
    } catch {}
  }

  /** Send one request; resolve with the raw response frame. */
  callRaw(method, params = {}, timeoutMs = 60_000) {
    const id = this.nextId++;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((res, rej) => {
      const timer = setTimeout(
        () => rej(new Error(`${method} timed out after ${timeoutMs}ms`)),
        timeoutMs,
      );
      this.pending.set(id, (frame) => {
        clearTimeout(timer);
        res(frame);
      });
    });
  }

  /** Call a method that must succeed; returns `result`, throws on error. */
  async call(method, params = {}, timeoutMs = 60_000) {
    const frame = await this.callRaw(method, params, timeoutMs);
    if (frame.ok !== true) {
      const err = new Error(
        `${method} errored: ${JSON.stringify(frame.error)}`,
      );
      err.code = frame.error?.code;
      throw err;
    }
    return frame.result;
  }
}

/** daemon.status.identity, or identity.create — idempotent across re-runs. */
export async function ensureIdentity(client) {
  const status = await client.call("daemon.status");
  if (status.mode !== "real") {
    throw new Error(`daemon is in ${status.mode} mode; the realnet scripts need real mode`);
  }
  if (status.identity) return status.identity;
  return client.call("identity.create");
}

/**
 * room.open, re-polled until the endpoint reports a dialable `id@ip:port,...`
 * addr (real-mode net discovery can land a beat after the first open).
 */
export async function openRoomWithAddr(client, roomId, timeoutMs = 30_000) {
  return pollUntil(
    async () => {
      const o = await client.call("room.open", { room_id: roomId });
      return typeof o.endpoint?.addr === "string" && o.endpoint.addr.includes("@")
        ? o
        : null;
    },
    timeoutMs,
    "room.open to report a dialable addr",
  );
}

/** The room timeline (full), as an events array. */
export async function timeline(client, roomId) {
  const { events } = await client.call("room.timeline", { room_id: roomId });
  return events;
}

/**
 * peers.status polled until at least one peer is connected with a known path
 * (or the deadline passes — then whatever is there is returned honestly).
 * Returns the peers array.
 */
export async function settledPeers(client, roomId, timeoutMs = 60_000) {
  try {
    return await pollUntil(
      async () => {
        const { peers } = await client.call("peers.status", { room_id: roomId });
        const settled = peers.some((p) => p.state === "connected" && p.path);
        return settled ? peers : null;
      },
      timeoutMs,
      "a connected peer with a known path",
    );
  } catch {
    const { peers } = await client.call("peers.status", { room_id: roomId });
    return peers;
  }
}

/** Print the peers table with the direct/relay verdict that matters. */
export function reportPeers(side, peers) {
  if (peers.length === 0) {
    console.log(`${side}: peers.status is EMPTY (no peer entries)`);
    return null;
  }
  let verdict = null;
  for (const p of peers) {
    console.log(
      `${side}: peer ${p.endpoint_id.slice(0, 12)}… state=${p.state} path=${p.path ?? "unknown"}`,
    );
    if (p.state === "connected" && p.path) verdict = p.path;
  }
  if (verdict === "direct") {
    console.log(`${side}: PATH = direct — NAT hole punch (or same-network dial) succeeded`);
  } else if (verdict === "relay") {
    console.log(`${side}: PATH = relay — hole punch did not complete; traffic relays via n0`);
  } else {
    console.log(`${side}: PATH = unknown — no connected peer reported a path`);
  }
  return verdict;
}
