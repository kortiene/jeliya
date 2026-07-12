#!/usr/bin/env node
// Jeliya real-agent harness: a room participant that does REAL work driven
// by chat messages. Node 22+ (global WebSocket), no npm deps.
//
// Usage:
//   node scripts/jeliya-agent.mjs --identity-only [--port 7461] [--data-dir <dir>] [--loopback]
//       Spawn the daemon, ensure an identity, print the identity_id (hand it
//       to the room owner so they can mint an agent-role invite), exit.
//
//   node scripts/jeliya-agent.mjs --ticket <T> --peer <ADDRS> [--room <room_id>]
//       [--port 7461] [--data-dir <dir>] [--worker claude|echo]
//       [--workspace <dir>] [--trigger @agent] [--allow-sender <64hex>]...
//       [--max-turns 40] [--loopback]
//       Join the room with the ticket (retrying — realnet-check pattern),
//       open it, announce itself, then serve tasks from chat.
//
//   node scripts/jeliya-agent.mjs --room <room_id> [...same flags, no --ticket]
//       Rejoin mode: this identity is already a member — skip the join, just
//       open the room and serve.
//
// --peer is repeatable; each value is one dialable "<endpoint_id>@<ip:port,...>"
// string (printed by the inviter's room.open). --loopback runs the spawned
// jeliyad on the SDK's offline 127.0.0.1 stack (must match the room's other
// daemons); default is the real network stack.
//
// =========================== TRUST MODEL ====================================
// This is room-driven code execution. A chat message that starts with the
// trigger phrase becomes either a deterministic echo (--worker echo) or a
// PROMPT TO THE `claude` CLI running with --permission-mode acceptEdits
// (--worker claude): an allowed sender effectively gets arbitrary-code /
// file-write access inside the per-task workspace on this machine.
//
//   * Allowed senders are EXACTLY the identities passed via --allow-sender
//     (repeatable, 64-hex identity ids).
//   * If no --allow-sender is given, the allowlist defaults to exactly ONE
//     identity: the sender of the room_created event in the timeline — the
//     room owner. Nobody else.
//   * A triggered message from any non-allowed sender is ignored and logged
//     locally. It is NEVER executed and gets NO reply of any kind, so a
//     probing sender learns nothing about the allowlist (no oracle).
//   * The agent never reacts to its own events or to non-message event kinds.
//   * Trigger messages timestamped before the runner started (minus 60s of
//     clock slack) are ignored, and a message with a missing/non-numeric
//     timestamp is treated as stale (fail closed): no backlog execution on
//     (re)join.
//
// HONESTY RULES: statuses reflect real activity (actual tool invocations from
// the claude stream). No fabricated progress percentages — progress is only
// ever the literal 100 on the final "done" status. Failures are posted as
// label "failed" with the real reason. See docs/agent-guide.md.
//
// ====================== TASK-CLAIM COORDINATION =============================
// When the room's membership snapshot shows MORE THAN ONE active agent-role
// member, an allowed trigger is arbitrated via a claim handshake over plain
// agent_status events (docs/agent-orchestration.md §2): post label "claiming"
// with status_message "task:<first 16 hex of the trigger's event_id>", wait
// CLAIM_SETTLE_MS collecting other agents' same-token claims (pushes + one
// timeline re-poll), then proceed only if our claim's event_id is the
// lexicographically lowest — otherwise stand down with no execution or chat
// reply, then post one best-effort "idle" status so operators can distinguish
// a resolved arbitration from a stuck claim. With ≤1 agent-role member the
// claim step is skipped entirely: no claim event, no delay.
//
// HONEST LIMITS — this is best-effort eventual coordination, NOT a lock:
//   (a) two runners whose gossip round-trip exceeds CLAIM_SETTLE_MS can both
//       see themselves as the minimum and both run;
//   (b) a lower claim arriving after a winner started does not abort the
//       winner;
//   (c) the deterministic event-id order guarantees only that all peers
//       EVENTUALLY agree on which claim won, bounding duplicates to the
//       propagation window. Duplicate "done" replies are possible and
//       acceptable; fabricating a stronger guarantee is not.
//
// ADDRESSED TRIGGERS: "<trigger> <task>" is for any agent; the form
// "<trigger>:<id-prefix> <task>" (case-insensitive hex) is only for agents
// whose identity id starts with the prefix — all others ignore it silently
// (no reply, no claim). Multiple prefix matches are arbitrated by claiming.
// ============================================================================

import { spawn } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, isAbsolute, join, relative, resolve } from "node:path";

import { Client, pollUntil, sleep, startRealDaemon } from "./realnet-lib.mjs";
import { defaultAgentDataDir, installAgentDataGitGuard } from "./agent-paths.mjs";

// Wire-protocol byte limits — enforced here by truncation, never by crashing.
const STATUS_LABEL_LIMIT = 64;
const STATUS_MESSAGE_LIMIT = 4096;
const MESSAGE_BODY_LIMIT = 16384;
const FILE_SHARE_LIMIT = 104857600; // 100 MiB

const MAX_ARTIFACTS = 8; // per task, shared via file.share
const STATUS_MIN_INTERVAL_MS = 15_000; // ctx.postStatus rate limit (per task)
const TASK_HARD_CAP_MS = 15 * 60_000; // claude worker wall-clock cap
const STALE_SLACK_MS = 60_000; // clock-skew slack for the no-backlog rule
const SEEN_CAP = 4096; // dedupe-set memory bound; must stay > RESYNC_TIMELINE_LIMIT
const FULL_TIMELINE_LIMIT = 4294967295; // u32::MAX — room.timeline defaults to a 200-event tail
const RESYNC_TIMELINE_LIMIT = 1_000; // tail size for the push-loss recovery poll
const RESYNC_INTERVAL_MS = 5_000; // how often to re-poll the timeline (pushes are lossy under load)
const REPLY_MIN_INTERVAL_MS = 10_000; // rate limit for busy/empty-task replies
const SHUTDOWN_FLUSH_MS = 2_000; // grace for the daemon to gossip final statuses before it dies
const CLAIM_SETTLE_MS = 1_500; // claim settle window (docs/agent-orchestration.md §2.3)
const CLAIM_TOKEN_RE = /^task:([0-9a-f]{16})(\s|$)/; // claim status_message parser (fail closed)

// ---------------------------------------------------------------------------
// CLI (custom parse: --allow-sender and --peer are repeatable)
// ---------------------------------------------------------------------------

function usage(code) {
  console.error(
    "usage:\n" +
      "  node scripts/jeliya-agent.mjs --identity-only [--port 7461] [--data-dir <dir>] [--loopback]\n" +
      "  node scripts/jeliya-agent.mjs --ticket <T> --peer <ADDRS> [--room <room_id>]\n" +
      "      [--port 7461] [--data-dir <dir>] [--worker claude|echo]\n" +
      "      [--workspace <dir>] [--trigger @agent] [--allow-sender <64hex>]...\n" +
      "      [--max-turns 40] [--loopback]\n" +
      "  node scripts/jeliya-agent.mjs --room <room_id> [...]   (rejoin: already a member)",
  );
  process.exit(code);
}

function parseCli(argv) {
  const cfg = {
    ticket: null,
    room: null,
    port: 7461,
    dataDir: defaultAgentDataDir(),
    // Default to the inert echo worker. Real host execution (`--worker claude`)
    // is arbitrary code/file execution for any allowlisted sender, so it must be
    // an explicit, informed opt-in — never the default a packaged shortcut or a
    // copied command line inherits silently.
    worker: "echo",
    workspace: null,
    trigger: "@agent",
    allowSenders: [],
    peers: [],
    maxTurns: 40,
    loopback: false,
    identityOnly: false,
  };
  const need = (i, flag) => {
    if (i + 1 >= argv.length) {
      console.error(`agent: ${flag} needs a value`);
      usage(2);
    }
    return argv[i + 1];
  };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    switch (a) {
      case "--ticket": cfg.ticket = need(i, a); i += 1; break;
      case "--room": cfg.room = need(i, a); i += 1; break;
      case "--port": cfg.port = Number(need(i, a)); i += 1; break;
      case "--data-dir": cfg.dataDir = need(i, a); i += 1; break;
      case "--worker": cfg.worker = need(i, a); i += 1; break;
      case "--workspace": cfg.workspace = need(i, a); i += 1; break;
      case "--trigger": cfg.trigger = need(i, a); i += 1; break;
      case "--allow-sender": cfg.allowSenders.push(need(i, a)); i += 1; break;
      case "--peer": cfg.peers.push(need(i, a)); i += 1; break;
      case "--max-turns": cfg.maxTurns = Number(need(i, a)); i += 1; break;
      case "--loopback": cfg.loopback = true; break;
      case "--identity-only": cfg.identityOnly = true; break;
      case "--help": case "-h": usage(0); break;
      default:
        console.error(`agent: unknown flag ${a}`);
        usage(2);
    }
  }
  return cfg;
}

const cfg = parseCli(process.argv.slice(2));
if (!cfg.identityOnly && !cfg.ticket && !cfg.room) usage(2);
if (!Number.isInteger(cfg.port) || cfg.port <= 0) {
  console.error("agent: --port must be a positive integer");
  usage(2);
}
if (!Number.isInteger(cfg.maxTurns) || cfg.maxTurns <= 0) {
  console.error("agent: --max-turns must be a positive integer");
  usage(2);
}
if (cfg.worker !== "echo" && cfg.worker !== "claude") {
  console.error(`agent: --worker must be "echo" or "claude" (got ${cfg.worker})`);
  usage(2);
}
if (cfg.worker === "claude") {
  console.error(
    "agent: WARNING — --worker claude runs the `claude` CLI with --permission-mode\n" +
      "       acceptEdits on every triggered message from an allowlisted sender. That\n" +
      "       is arbitrary code / file execution on this host. Only enable it for a\n" +
      "       room and senders you trust.",
  );
}
for (const s of cfg.allowSenders) {
  if (!/^[0-9a-f]{64}$/.test(s)) {
    console.error(`agent: --allow-sender must be a 64-hex identity id (got ${JSON.stringify(s)})`);
    usage(2);
  }
}

const DATA_DIR = resolve(cfg.dataDir);
// Defense in depth for explicit paths under a Git checkout: even an
// unfamiliar directory name gets its own deny-all marker before jeliyad can
// create identity.secret or daemon.json inside it.
installAgentDataGitGuard(DATA_DIR);
// SECURITY: the default workspace lives OUTSIDE the daemon data dir. DATA_DIR
// holds identity.secret (the daemon's Ed25519 private key) and the room
// stores; a task cwd under it would put that key at a fixed ../../ offset
// from every prompt-injectable claude run. Default (a fresh OS-temp dir) is
// created lazily in main; --workspace overrides it.
let WORK_PARENT = cfg.workspace ? resolve(cfg.workspace) : null;
const TRIGGER = cfg.trigger;

function log(msg) {
  console.log(`agent: ${msg}`);
}

// ---------------------------------------------------------------------------
// Byte-limit helpers
// ---------------------------------------------------------------------------

/** UTF-8-safe truncation to at most `max` bytes (never splits a code point). */
function truncateBytes(s, max) {
  const str = String(s ?? "");
  const buf = Buffer.from(str, "utf8");
  if (buf.length <= max) return str;
  let end = max;
  while (end > 0 && (buf[end] & 0xc0) === 0x80) end -= 1; // back off continuation bytes
  return buf.subarray(0, end).toString("utf8");
}

// ---------------------------------------------------------------------------
// Workers — async worker(task, ctx) -> { summary, artifacts: [abs paths] }.
// ctx = { workspace, postStatus(label, message), log }. Failures are thrown
// as errors carrying { reason, artifacts } (taskFailure below).
// ---------------------------------------------------------------------------

function taskFailure(reason, artifacts = []) {
  const err = new Error(reason);
  err.reason = reason;
  err.artifacts = artifacts;
  return err;
}

/** Recursively list regular files under root (skips dotfiles, node_modules). */
function walkFiles(root, out = []) {
  let entries;
  try {
    entries = readdirSync(root, { withFileTypes: true });
  } catch {
    return out;
  }
  entries.sort((x, y) => x.name.localeCompare(y.name));
  for (const ent of entries) {
    if (ent.name.startsWith(".") || ent.name === "node_modules") continue;
    const p = join(root, ent.name);
    if (ent.isDirectory()) walkFiles(p, out);
    else if (ent.isFile()) out.push(p);
  }
  return out;
}

/** Deterministic worker for e2e: no LLM, no network. */
async function echoWorker(task, ctx) {
  const content = `echo: ${task}`;
  const path = join(ctx.workspace, "result.txt");
  writeFileSync(path, content);
  await ctx.postStatus("working", `echoing ${Buffer.byteLength(content)} bytes into result.txt`);
  return { summary: `echoed ${Buffer.byteLength(content)} bytes`, artifacts: [path] };
}

/**
 * The real worker: spawn the `claude` CLI on the task, stream NDJSON, post
 * honest throttled "working" statuses on actual tool invocations, capture the
 * terminal result line. Hard wall-clock cap; nonzero exit / missing result /
 * error result -> taskFailure with the real reason (workspace files still
 * returned as artifacts).
 */
async function claudeWorker(task, ctx) {
  const child = spawn(
    "claude",
    [
      "-p", task,
      "--output-format", "stream-json",
      "--verbose",
      "--max-turns", String(cfg.maxTurns),
      "--permission-mode", "acceptEdits",
    ],
    // detached: the child leads its own process group, so the hard-cap /
    // shutdown kill reaches grandchildren too (a backgrounded dev server,
    // tool subprocesses, ...), not just the direct claude pid.
    { cwd: ctx.workspace, stdio: ["ignore", "pipe", "pipe"], detached: true },
  );
  workerChild = child;

  let lineBuf = "";
  let stderrTail = "";
  let resultText = null;
  let resultIsError = false;
  let toolCalls = 0;
  let timedOut = false;
  let spawnError = null;
  const timer = setTimeout(() => {
    timedOut = true;
    killProcessTree(child);
  }, TASK_HARD_CAP_MS);

  // setEncoding: Node's internal StringDecoder holds multi-byte UTF-8
  // sequences split across pipe chunks; a bare `+= <Buffer>` would corrupt
  // any code point straddling a chunk boundary into U+FFFD.
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (d) => {
    lineBuf += d;
    let nl;
    while ((nl = lineBuf.indexOf("\n")) !== -1) {
      const line = lineBuf.slice(0, nl).trim();
      lineBuf = lineBuf.slice(nl + 1);
      if (!line) continue;
      let obj;
      try {
        obj = JSON.parse(line);
      } catch {
        continue; // tolerate non-JSON noise line by line — never crash
      }
      if (obj.type === "assistant") {
        for (const blk of obj.message?.content ?? []) {
          if (blk?.type === "tool_use") {
            toolCalls += 1;
            // Honest, throttled progress: the tool actually being run,
            // counted as tool calls (the CLI's own turn accounting differs).
            // Task excerpt appended so Fleet/Agents shows what prompted the
            // tool call, not just the low-level tool name; postStatus (via
            // postStatusNow) truncates to STATUS_MESSAGE_LIMIT regardless.
            const excerpt = task.length > 60 ? `${task.slice(0, 60)}…` : task;
            void ctx.postStatus("working", `running: ${blk.name} (tool call ${toolCalls}) — ${excerpt}`);
          }
        }
      } else if (obj.type === "result") {
        resultText = typeof obj.result === "string" ? obj.result : JSON.stringify(obj.result ?? "");
        resultIsError = obj.is_error === true || (obj.subtype != null && obj.subtype !== "success");
      }
    }
  });
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (d) => {
    stderrTail = (stderrTail + d).slice(-2000);
    const c0 = stderrTail.charCodeAt(0);
    if (c0 >= 0xdc00 && c0 <= 0xdfff) stderrTail = stderrTail.slice(1); // never keep a lone low surrogate
  });

  const [code, signal] = await new Promise((res) => {
    let exited = null;
    child.on("error", (err) => {
      spawnError = err; // spawn failure (e.g. claude not on PATH) — see below
      res([-1, null]);
    });
    child.on("exit", (c, s) => {
      exited = [c, s];
      // 'close' additionally waits for the stdio pipes to drain; a surviving
      // grandchild holding them open must not park this await forever.
      setTimeout(() => res(exited), 5_000).unref();
    });
    child.on("close", (c, s) => res(exited ?? [c, s]));
  });
  clearTimeout(timer);
  workerChild = null;

  const artifacts = walkFiles(ctx.workspace);
  if (timedOut) {
    throw taskFailure(`claude timed out after ${TASK_HARD_CAP_MS / 60_000} minutes (killed)`, artifacts);
  }
  if (spawnError) {
    // Honest mechanism: the process never ran (or errored), it did not "exit".
    const what = child.pid == null ? "failed to spawn claude" : "claude process error";
    throw taskFailure(`${what}: ${spawnError.message}`, artifacts);
  }
  if (code !== 0) {
    const why = stderrTail.trim().slice(0, 500) || "no stderr";
    throw taskFailure(`claude exited with code ${code}${signal ? ` (signal ${signal})` : ""}: ${why}`, artifacts);
  }
  if (resultText === null) {
    throw taskFailure("claude stream ended without a terminal result line (parse failure)", artifacts);
  }
  if (resultIsError) {
    throw taskFailure(`claude reported an error result: ${resultText.slice(0, 500)}`, artifacts);
  }
  return { summary: resultText, artifacts };
}

const WORKERS = { echo: echoWorker, claude: claudeWorker };
if (!WORKERS[cfg.worker]) {
  console.error(`agent: unknown worker ${JSON.stringify(cfg.worker)} — use claude|echo`);
  usage(2);
}
const worker = WORKERS[cfg.worker];

// ---------------------------------------------------------------------------
// Daemon + client + lifecycle
// ---------------------------------------------------------------------------

let daemon = null;
let client = null;
let currentRoomId = null;
let shuttingDown = false;
let me = null; // this runner's identity (set in main)
let current = null; // { task } while a task is running — one at a time
let workerChild = null; // the in-flight claude process (own group leader), if any

/** SIGKILL a detached worker's whole process group, then the leader itself. */
function killProcessTree(proc, signal = "SIGKILL") {
  if (!proc || proc.pid == null) return;
  try {
    process.kill(-proc.pid, signal); // the detached group — grandchildren too
  } catch {}
  try {
    proc.kill(signal);
  } catch {}
}

// Last-resort teardown on EVERY exit path (FATAL throw, daemon onExit, the
// lib's ws-onclose process.exit, normal shutdown): never leave the spawned
// daemon or an in-flight claude tree running past the runner.
process.on("exit", () => {
  killProcessTree(workerChild);
  try {
    daemon?.kill("SIGKILL");
  } catch {}
});

async function shutdown(code = 0) {
  if (shuttingDown) return;
  shuttingDown = true;
  // Stop the in-flight worker FIRST: nothing may keep executing (acceptEdits)
  // in the workspace after the runner reports itself offline.
  killProcessTree(workerChild);
  if (client?.ws && currentRoomId) {
    // Best-effort final statuses: race against short deadlines, never hang.
    if (current) {
      // Honest terminal status for the task we just killed — the room must
      // not be left watching a live "working" status for a dead worker.
      try {
        await Promise.race([
          client.call("status.post", {
            room_id: currentRoomId,
            label: "failed",
            message: truncateBytes(
              `aborted by runner shutdown: ${current.task}`,
              STATUS_MESSAGE_LIMIT,
            ),
          }),
          sleep(3_000),
        ]);
      } catch {}
    }
    try {
      await Promise.race([
        client.call("status.post", {
          room_id: currentRoomId,
          label: "offline",
          message: "agent runner shutting down",
        }),
        sleep(3_000),
      ]);
    } catch {}
    // A status.post ack only proves LOCAL append — give the daemon a short
    // grace to gossip the final statuses to peers before we SIGKILL it.
    await sleep(SHUTDOWN_FLUSH_MS);
  }
  client?.close();
  try {
    daemon?.kill("SIGKILL");
  } catch {}
  process.exit(code);
}
for (const sig of ["SIGINT", "SIGTERM"]) {
  process.on(sig, () => void shutdown(0));
}

mkdirSync(DATA_DIR, { recursive: true });
daemon = startRealDaemon({
  port: cfg.port,
  dataDir: DATA_DIR,
  label: "agentd",
  loopback: cfg.loopback,
  onExit: (code, signal) => {
    if (!shuttingDown) {
      console.error(`agent: daemon exited early (code=${code} signal=${signal})`);
      process.exit(1);
    }
  },
});
client = new Client("agent");

/** daemon.status identity, or identity.create — idempotent, mode-agnostic. */
async function ensureIdentity() {
  const status = await client.call("daemon.status");
  if (status.identity) return status.identity;
  return client.call("identity.create");
}

/** room.timeline events; the daemon defaults to a 200-event tail, so callers
 * that need history pass an explicit limit (FULL_TIMELINE_LIMIT for all). */
async function timelineEvents(roomId, limit) {
  const { events } = await client.call("room.timeline", {
    room_id: roomId,
    ...(limit != null ? { limit } : {}),
  });
  return events;
}

async function sendMessage(body) {
  await client.call("message.send", {
    room_id: currentRoomId,
    body: truncateBytes(body, MESSAGE_BODY_LIMIT),
  });
}

// Rate-limited busy/empty-task replies: a careless (but allowed) sender must
// not be able to make the agent flood the room with control chatter.
let lastControlReplyAt = 0;
function sendControlReply(body, what) {
  const now = Date.now();
  if (now - lastControlReplyAt < REPLY_MIN_INTERVAL_MS) {
    log(`${what} reply suppressed (rate limited, kept local)`);
    return;
  }
  lastControlReplyAt = now;
  void sendMessage(body).catch((err) => log(`${what} reply failed: ${err.message}`));
}

/** Direct (unthrottled) status post — startup/final statuses. */
async function postStatusNow(label, message, extra = {}) {
  await client.call("status.post", {
    room_id: currentRoomId,
    label: truncateBytes(label, STATUS_LABEL_LIMIT),
    message: truncateBytes(message ?? "", STATUS_MESSAGE_LIMIT),
    ...extra,
  });
}

/**
 * Throttled status poster handed to workers (fresh per task): at most one
 * status per STATUS_MIN_INTERVAL_MS; the first call always lands; excess
 * calls are dropped (logged locally). Always truncated; never throws.
 */
function makeThrottledStatus() {
  let last = 0;
  return async (label, message) => {
    const now = Date.now();
    if (now - last < STATUS_MIN_INTERVAL_MS) {
      log(`status throttled (kept local): ${label} — ${message}`);
      return;
    }
    last = now;
    try {
      await postStatusNow(label, message);
    } catch (err) {
      log(`status.post failed (ignored): ${err.message}`);
    }
  };
}

// ---------------------------------------------------------------------------
// Task plumbing
// ---------------------------------------------------------------------------

/** Next fresh numeric workspace subdir under WORK_PARENT. */
function nextTaskNumber() {
  let max = 0;
  try {
    for (const name of readdirSync(WORK_PARENT)) {
      const n = Number(name);
      if (Number.isInteger(n) && n > max) max = n;
    }
  } catch {}
  return max + 1;
}

/**
 * Parse a trigger body into { idPrefix, task } or null when it is not a
 * trigger at all. Two accepted forms (docs/agent-orchestration.md §2.4):
 *   "<trigger> <task>"              → { idPrefix: null, task }  (any agent)
 *   "<trigger>:<hexprefix> <task>"  → { idPrefix, task }        (addressed)
 * The addressed prefix is case-insensitive hex; a malformed address (non-hex,
 * or no word boundary after it) is NOT a trigger — fail closed, silence.
 */
function parseTrigger(body) {
  if (body.length < TRIGGER.length) return null;
  if (body.slice(0, TRIGGER.length).toLowerCase() !== TRIGGER.toLowerCase()) return null;
  let rest = body.slice(TRIGGER.length);
  let idPrefix = null;
  if (rest.startsWith(":")) {
    const m = /^:([0-9a-fA-F]+)(\s|$)/.exec(rest);
    if (!m) return null;
    idPrefix = m[1].toLowerCase();
    rest = rest.slice(1 + m[1].length);
  }
  if (rest !== "" && !/^\s/.test(rest)) return null;
  return { idPrefix, task: rest.trim() };
}

/** True when `child` lives strictly under `parent`. */
function isUnder(child, parent) {
  const rel = relative(parent, child);
  return rel !== "" && !rel.startsWith("..") && !isAbsolute(rel);
}

/** One reporting attempt with a single retry. Returns true on success. */
async function tryReport(what, fn) {
  for (let attempt = 1; attempt <= 2; attempt += 1) {
    try {
      await fn();
      return true;
    } catch (err) {
      log(`${what} failed (attempt ${attempt}/2): ${err.message}`);
      if (attempt < 2) await sleep(1_000);
    }
  }
  return false;
}

/**
 * A file.share that timed out CLIENT-side may still complete in the daemon
 * (the import keeps running after our promise rejects). Before reporting the
 * share as failed, poll file.list for a NEW row from us matching the name
 * (and size when known) — the done status's artifact refs must match reality.
 */
async function findLateShare(name, sharePath, preShareIds) {
  let size = null;
  try {
    size = statSync(sharePath).size;
  } catch {}
  try {
    return await pollUntil(
      async () => {
        const { files } = await client.call("file.list", { room_id: currentRoomId });
        const row = files.find(
          (f) =>
            !preShareIds.has(f.file_id) &&
            f.name === name &&
            f.sender_id === me.identity_id &&
            (size == null || f.size === size),
        );
        return row ? row.file_id : null;
      },
      45_000,
      `a late-completing file.share of ${name}`,
      3_000,
    );
  } catch {
    return null;
  }
}

/**
 * After the worker returns (or fails): share artifacts (cap MAX_ARTIFACTS,
 * skip+note oversize), post the final room message, then the terminal status
 * — "done" with the literal progress 100 and the shared file ids, or
 * "failed" with the honest reason.
 *
 * The daemon confines file.share to paths under its own data dir, so any
 * artifact living elsewhere (e.g. a --workspace outside --data-dir) is first
 * copied into <data-dir>/share/<taskN>/ and shared from there.
 *
 * REPORTING errors never escape this function: a reporting hiccup must never
 * rewrite a succeeded task as failed (the ok:false path is reserved for
 * worker failures, routed here by startTask's catch).
 */
async function finishTask({ ok, summary, reason, artifacts, workspace, taskN }) {
  const notes = [];
  let allArtifacts = [...new Set((artifacts ?? []).map((p) => resolve(p)))];
  let body;
  if (ok) {
    const full = summary ?? "";
    if (Buffer.byteLength(full, "utf8") > MESSAGE_BODY_LIMIT) {
      // Don't spam chat: full result goes to result.md, shared as an artifact.
      const resultPath = join(workspace, "result.md");
      try {
        writeFileSync(resultPath, full);
        allArtifacts = [resultPath, ...allArtifacts.filter((p) => p !== resultPath)];
        body = `${truncateBytes(full, 2000)}\n\n[result truncated — full text shared as result.md]`;
      } catch (err) {
        notes.push(`could not write result.md: ${err.message}`);
        body = `${truncateBytes(full, 2000)}\n\n[result truncated — result.md could not be written]`;
      }
    } else {
      body = full || "(task finished with an empty result)";
    }
  } else {
    body = `Task failed: ${reason}`;
  }

  const toShare = [];
  for (const p of allArtifacts) {
    if (toShare.length >= MAX_ARTIFACTS) {
      notes.push(`artifact cap (${MAX_ARTIFACTS}): ${allArtifacts.length - MAX_ARTIFACTS} more not shared`);
      break;
    }
    let size;
    try {
      size = statSync(p).size;
    } catch {
      notes.push(`artifact vanished before sharing, skipped: ${basename(p)}`);
      continue;
    }
    if (size > FILE_SHARE_LIMIT) {
      notes.push(`artifact over the ${FILE_SHARE_LIMIT}-byte share limit, skipped: ${basename(p)} (${size} bytes)`);
      continue;
    }
    toShare.push(p);
  }
  const fileIds = [];
  const stagingDir = join(DATA_DIR, "share", String(taskN));
  const stagedNames = new Set();
  // Snapshot the room's file ids before sharing so a client-side share
  // timeout can be reconciled against rows that appear afterwards.
  let preShareIds = null;
  if (toShare.length > 0) {
    try {
      const { files } = await client.call("file.list", { room_id: currentRoomId });
      preShareIds = new Set(files.map((f) => f.file_id));
    } catch {
      preShareIds = null;
    }
  }
  for (const p of toShare) {
    let sharePath = p;
    try {
      if (!isUnder(p, DATA_DIR)) {
        mkdirSync(stagingDir, { recursive: true });
        let destName = basename(p);
        for (let i = 2; stagedNames.has(destName); i += 1) destName = `${i}-${basename(p)}`;
        stagedNames.add(destName);
        sharePath = join(stagingDir, destName);
        copyFileSync(p, sharePath);
      }
      const { file_id } = await client.call(
        "file.share",
        { room_id: currentRoomId, path: sharePath, name: basename(p) },
        120_000,
      );
      fileIds.push(file_id);
      preShareIds?.add(file_id);
      log(`shared artifact ${basename(p)} as ${file_id}`);
    } catch (err) {
      // A client timeout is not proof of failure — the daemon's import may
      // still finish and author file_shared. Reconcile before reporting.
      let lateId = null;
      if (preShareIds && /timed out after/.test(err.message ?? "")) {
        lateId = await findLateShare(basename(p), sharePath, preShareIds);
      }
      if (lateId) {
        fileIds.push(lateId);
        preShareIds.add(lateId);
        log(`file.share of ${basename(p)} timed out client-side but completed — recovered as ${lateId}`);
      } else {
        notes.push(`file.share failed for ${basename(p)}: ${err.message}`);
      }
    }
  }

  if (notes.length > 0) {
    // The disclosure notes must never be silently truncated away: bound the
    // note block, then shrink the summary (with a marker) to make room.
    let noteBlock = `\n\n${notes.map((n) => `note: ${truncateBytes(n, 500)}`).join("\n")}`;
    if (Buffer.byteLength(noteBlock, "utf8") > 8_192) {
      noteBlock = `${truncateBytes(noteBlock, 8_192)}\n[…more notes truncated]`;
    }
    const noteBytes = Buffer.byteLength(noteBlock, "utf8");
    if (Buffer.byteLength(body, "utf8") + noteBytes > MESSAGE_BODY_LIMIT) {
      const marker = "\n[…summary truncated to fit the notes]";
      const keep = Math.max(MESSAGE_BODY_LIMIT - noteBytes - Buffer.byteLength(marker, "utf8"), 0);
      body = `${truncateBytes(body, keep)}${marker}`;
    }
    body += noteBlock;
  }

  // Reporting errors are retried once, then kept local — never re-routed
  // through the failure path (a done task whose announcement hiccups is
  // still done; the room is not told otherwise).
  await tryReport("result message", () => sendMessage(body));
  if (ok) {
    await tryReport('"done" status', () =>
      postStatusNow("done", summary ?? "", { progress: 100, artifacts: fileIds }),
    );
  } else {
    // Honest failure: real reason, no progress number.
    await tryReport('"failed" status', () =>
      postStatusNow("failed", reason, fileIds.length > 0 ? { artifacts: fileIds } : {}),
    );
  }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

try {
  await client.connect(cfg.port);
  me = await ensureIdentity();

  if (cfg.identityOnly) {
    console.log("");
    console.log("agent: =========== HAND THIS TO THE ROOM OWNER ===========");
    console.log(`agent: identity_id = ${me.identity_id}`);
    console.log("agent: mint an agent-role invite for this identity, then run:");
    console.log("agent:   node scripts/jeliya-agent.mjs --ticket <T> --peer <ADDRS> [--worker claude|echo]");
    console.log("agent: ===================================================");
    console.log(`agent: identity persisted in ${DATA_DIR} — re-runs reuse it`);
    client.close();
    try {
      daemon.kill("SIGKILL");
    } catch {}
    shuttingDown = true;
    process.exit(0);
  }

  log(`identity ${me.identity_id}`);
  const staleBefore = Date.now() - STALE_SLACK_MS;

  // Join by ticket (with retries — the daemon's per-attempt bootstrap window
  // can miss the first dial while discovery warms up), or rejoin by --room.
  let roomId = cfg.room;
  if (cfg.ticket) {
    let lastErr = null;
    let joinedId = null;
    for (let attempt = 1; attempt <= 5 && !joinedId; attempt += 1) {
      try {
        log(`room.join attempt ${attempt}...`);
        const joined = await client.call(
          "room.join",
          { ticket: cfg.ticket, peers: cfg.peers },
          120_000,
        );
        joinedId = joined.room_id;
      } catch (err) {
        lastErr = err;
        log(`join attempt ${attempt} failed (${err.message}); retrying`);
        await sleep(2_000);
      }
    }
    if (!joinedId) throw lastErr ?? new Error("room.join failed");
    if (roomId && roomId !== joinedId) {
      throw new Error(`ticket joined room ${joinedId}, but --room says ${roomId}`);
    }
    roomId = joinedId;
    log(`joined room ${roomId}`);
  } else {
    log(`rejoin mode: opening room ${roomId} (already a member)`);
  }

  const opened = await client.call("room.open", {
    room_id: roomId,
    ...(cfg.peers.length > 0 ? { peers: cfg.peers } : {}),
  });
  currentRoomId = roomId;
  log("room open");

  // Race-free history snapshot, taken FIRST: room.open returns the full
  // synced timeline, and the daemon seeds its push dedupe with that same set,
  // so everything absent from this snapshot arrives as a push. Snapshotting
  // here (before the owner-allowlist poll below) closes the window where a
  // live trigger synced during that poll would be misfiled as history and
  // silently dropped. `seen` is BOUNDED — any room member can grow the event
  // log forever, and an unbounded dedupe set is an OOM lever; FIFO eviction
  // is safe because the resync poll below only re-reads the most recent
  // RESYNC_TIMELINE_LIMIT (< SEEN_CAP) events and the daemon never re-pushes.
  const seen = new Set();
  const rememberSeen = (id) => {
    if (seen.has(id)) return false;
    seen.add(id);
    while (seen.size > SEEN_CAP) {
      seen.delete(seen.values().next().value); // Sets iterate in insertion order
    }
    return true;
  };
  const openedTimeline = Array.isArray(opened.timeline) ? opened.timeline : [];
  for (const e of openedTimeline) {
    if (e?.event_id) rememberSeen(e.event_id);
  }

  // TRUST MODEL resolution: explicit --allow-sender list, else exactly the
  // room_created sender (the owner). room.timeline defaults to a 200-event
  // tail that never contains the genesis event in a mature room — check the
  // open snapshot first, then poll with an explicit full-log limit.
  const allowed = new Set(cfg.allowSenders);
  if (allowed.size === 0) {
    const created =
      openedTimeline.find((e) => e.kind === "room_created") ??
      (await pollUntil(
        async () =>
          (await timelineEvents(roomId, FULL_TIMELINE_LIMIT)).find((e) => e.kind === "room_created"),
        60_000,
        "the room_created event to sync (needed to derive the owner allowlist)",
        1_000,
      ));
    if (!created.sender?.identity_id) {
      throw new Error("room_created event has no sender — cannot derive the owner allowlist");
    }
    allowed.add(created.sender.identity_id);
  }
  log(`allowed senders (${allowed.size}): ${[...allowed].map((s) => s.slice(0, 12) + "…").join(", ")}`);

  // Announce: online status (trigger + worker stated), then a room message.
  await postStatusNow(
    "online",
    `worker=${cfg.worker}; trigger="${TRIGGER} <task>"; allowed senders: ${allowed.size}`,
  );
  await sendMessage(
    `Agent online — mention ${TRIGGER} <task> to hand me work (worker: ${cfg.worker}).`,
  );
  log(`online — waiting for "${TRIGGER} <task>" (worker: ${cfg.worker})`);

  if (!WORK_PARENT) WORK_PARENT = mkdtempSync(join(tmpdir(), "jeliya-agent-work-"));
  if (WORK_PARENT === DATA_DIR || isUnder(WORK_PARENT, DATA_DIR)) {
    log(
      `WARNING: workspace ${WORK_PARENT} is inside the daemon data dir — task prompts ` +
        "can reach identity.secret by relative path; use a directory outside --data-dir",
    );
  }
  mkdirSync(WORK_PARENT, { recursive: true });
  log(`task workspaces under ${WORK_PARENT}`);
  let taskCounter = nextTaskNumber();

  function startTask(task) {
    const n = taskCounter;
    taskCounter += 1;
    const workspace = join(WORK_PARENT, String(n));
    mkdirSync(workspace, { recursive: true });
    const ctx = { workspace, postStatus: makeThrottledStatus(), log };
    log(`task #${n} started: ${JSON.stringify(task.slice(0, 120))} (workspace ${workspace})`);
    current = { task };
    void (async () => {
      try {
        const { summary, artifacts } = await worker(task, ctx);
        await finishTask({ ok: true, summary, artifacts, workspace, taskN: n });
        log(`task #${n} done`);
      } catch (err) {
        const reason = err?.reason ?? err?.message ?? String(err);
        try {
          await finishTask({ ok: false, reason, artifacts: err?.artifacts ?? [], workspace, taskN: n });
        } catch (err2) {
          log(`could not report task failure: ${err2.message}`);
        }
        log(`task #${n} failed: ${reason}`);
      } finally {
        current = null;
        // LIVENESS (docs/agent-orchestration.md §1.1): one honest "idle"
        // after the terminal done/failed status — an idle agent is now
        // distinguishable from a stalled "working" one. Not a heartbeat.
        if (!shuttingDown) {
          try {
            await postStatusNow("idle", "ready for the next task");
          } catch (err) {
            log(`"idle" status failed (ignored): ${err.message}`);
          }
        }
      }
    })();
  }

  // CLAIM OBSERVATION: claims for tokens with an open settle window are
  // collected from the SAME event stream the task loop drains (pushes + the
  // periodic resync), plus one explicit timeline re-poll at the end of each
  // window. Tokens without an open window are ignored — bounded memory.
  const activeClaims = new Map(); // token -> Set<claim event_id>
  function observeClaim(ev) {
    if (ev?.kind !== "agent_status" || ev.label !== "claiming") return;
    const m = CLAIM_TOKEN_RE.exec(ev.status_message ?? "");
    if (!m) return; // fail closed: an unparseable claim is not a claim
    const ids = activeClaims.get(m[1]);
    if (ids && typeof ev.event_id === "string") ids.add(ev.event_id);
  }

  /**
   * Claim gate (docs/agent-orchestration.md §2.3). Runs AFTER the existing
   * allowlist/staleness/busy checks. ≤1 active agent-role member: execute
   * immediately (no claim event, no delay). Otherwise post a claim, settle,
   * and proceed only when holding the lexicographically lowest claim
   * event_id; losers stand down without execution or chat reply, then post
   * one best-effort idle status for truthful liveness.
   */
  async function claimAndMaybeStart(task, triggerEv) {
    let eligible = null;
    try {
      const { members } = await client.call("room.members", { room_id: currentRoomId });
      eligible = members.filter((m) => m.role === "agent" && m.status === "active").length;
    } catch (err) {
      // Fail toward coordination: if we cannot count agents, claim anyway.
      log(`room.members failed (${err.message}) — assuming multiple agents, claiming`);
    }
    if (eligible !== null && eligible <= 1) {
      startTaskIfIdle(task);
      return;
    }
    if (!/^[0-9a-f]{64}$/.test(triggerEv.event_id ?? "")) {
      // Cannot derive a token no other agent could derive either — the claim
      // protocol is impossible for this event; run rather than lose the task.
      log(`trigger event_id is not bare 64-hex — claim impossible, executing`);
      startTaskIfIdle(task);
      return;
    }
    const token = triggerEv.event_id.slice(0, 16);
    const ids = new Set();
    activeClaims.set(token, ids);
    let myClaimId;
    try {
      const posted = await client.call("status.post", {
        room_id: currentRoomId,
        label: "claiming",
        message: truncateBytes(
          `task:${token} from ${triggerEv.sender.identity_id.slice(0, 12)}… — ${task.slice(0, 120)}`,
          STATUS_MESSAGE_LIMIT,
        ),
      });
      myClaimId = posted.event_id;
    } catch (err) {
      activeClaims.delete(token);
      log(`claim post failed (${err.message}) — standing down on task:${token}`);
      return;
    }
    ids.add(myClaimId);
    log(`claim posted for task:${token} (eligible agents: ${eligible ?? "unknown"}) — settling ${CLAIM_SETTLE_MS}ms`);
    await sleep(CLAIM_SETTLE_MS);
    // Pushes are lossy — one timeline re-poll is the safety net for claims
    // whose push frame was dropped or that gossiped before our window opened.
    try {
      for (const ev of await timelineEvents(currentRoomId, RESYNC_TIMELINE_LIMIT)) observeClaim(ev);
    } catch (err) {
      log(`claim-settle timeline poll failed (${err.message}) — deciding on pushes only`);
    }
    activeClaims.delete(token);
    if (current) {
      log(`became busy during the task:${token} settle window — standing down (never queue)`);
      return;
    }
    const winner = [...ids].sort()[0];
    if (winner !== myClaimId) {
      log(`lost claim task:${token} to ${winner.slice(0, 12)}… — standing down silently`);
      // LIVENESS: without this, our last posted status for this task stays
      // "claiming" forever — an operator can't tell a just-lost arbitration
      // from a stuck agent. Mirror the idle-liveness post in startTask's
      // finally block: best-effort, never let a status failure crash us.
      try {
        await postStatusNow("idle", "stood down - lost claim arbitration");
      } catch (err) {
        log(`"idle" status failed (ignored): ${err.message}`);
      }
      return;
    }
    log(`won claim task:${token} (${ids.size} claim(s) observed)`);
    startTaskIfIdle(task);
  }

  /** Re-check the one-task-at-a-time gate after any async gap. Standing down
   * here is silent by contract (local log only — no reply, no status). */
  function startTaskIfIdle(task) {
    if (current) {
      log("busy after claim/count window — task dropped (not queued)");
      return;
    }
    startTask(task);
  }

  function handleEvent(ev) {
    if (ev.kind !== "message") return; // never react to non-message kinds
    const senderId = ev.sender?.identity_id;
    if (!senderId || senderId === me.identity_id) return; // never react to own events
    const body = (ev.body ?? "").trim();
    const parsed = parseTrigger(body);
    if (!parsed) return;
    if (parsed.idPrefix !== null && !me.identity_id.startsWith(parsed.idPrefix)) {
      // Addressed to a different agent: silence (no reply, no claim).
      log(`ignored trigger addressed to ${parsed.idPrefix}… (not this agent)`);
      return;
    }
    if (typeof ev.ts !== "number" || ev.ts < staleBefore) {
      // Fail CLOSED: a missing/non-numeric ts is treated as stale rather than
      // waved through — no backlog execution on (re)join, ever.
      log(`ignored stale trigger from ${senderId.slice(0, 12)}… (ts missing or predates startup)`);
      return;
    }
    if (!allowed.has(senderId)) {
      // SECURITY: not executed, and deliberately NO reply (no oracle).
      log(`SECURITY: ignored trigger from non-allowed sender ${senderId} — not executed, no reply`);
      return;
    }
    const task = parsed.task;
    if (current) {
      const runningHead = current.task.slice(0, 80);
      sendControlReply(
        `Busy: already running "${runningHead}" — new task NOT queued; retry after the done/failed status.`,
        "busy",
      );
      log(`busy — refused a task from ${senderId.slice(0, 12)}… (not queued)`);
      return;
    }
    if (!task) {
      sendControlReply(`No task text after ${TRIGGER} — try: ${TRIGGER} <what to do>.`, "empty-task");
      return;
    }
    void claimAndMaybeStart(task, ev);
  }

  // TASK LOOP: drain room.event pushes collected by the client. The daemon's
  // push channel is explicitly lossy under load (a lagged broadcast
  // subscriber just misses frames, and missed events are never re-pushed),
  // so a periodic timeline re-poll recovers anything a dropped push lost;
  // `rememberSeen` dedupes the two sources.
  let lastResync = Date.now();
  for (;;) {
    const pushes = client.pushes.splice(0);
    for (const frame of pushes) {
      if (frame.push !== "room.event") continue;
      if (frame.data?.room_id !== roomId) continue;
      const ev = frame.data.event;
      if (!ev?.event_id || !rememberSeen(ev.event_id)) continue;
      observeClaim(ev);
      handleEvent(ev);
    }
    if (Date.now() - lastResync >= RESYNC_INTERVAL_MS) {
      lastResync = Date.now();
      try {
        for (const ev of await timelineEvents(roomId, RESYNC_TIMELINE_LIMIT)) {
          if (!ev?.event_id || !rememberSeen(ev.event_id)) continue;
          observeClaim(ev);
          handleEvent(ev);
        }
      } catch (err) {
        log(`timeline resync failed (will retry): ${err.message}`);
      }
    }
    await sleep(200);
  }
} catch (err) {
  console.error(`agent: FATAL — ${err?.stack ?? err}`);
  shuttingDown = true;
  killProcessTree(workerChild);
  client?.close();
  try {
    daemon?.kill("SIGKILL");
  } catch {}
  process.exit(1);
}
