import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { createConnection, createServer } from "node:net";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import {
  readProcessIdentity,
  recordOwnedProcess,
  signalOwnedProcessGroup,
} from "./e2e-process-ownership.mjs";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

function listen(port) {
  return new Promise((resolveListen, reject) => {
    const server = createServer((socket) => socket.end("owned-by-test\n"));
    server.once("error", reject);
    server.listen(port, "127.0.0.1", () => resolveListen(server));
  });
}

function close(server) {
  return new Promise((resolveClose, reject) => {
    server.close((error) => error ? reject(error) : resolveClose());
  });
}

function probe(port) {
  return new Promise((resolveProbe, reject) => {
    const socket = createConnection({ host: "127.0.0.1", port });
    socket.setEncoding("utf8");
    let output = "";
    socket.on("data", (chunk) => { output += chunk; });
    socket.on("end", () => resolveProbe(output));
    socket.on("error", reject);
  });
}

function processGroupHasMembers(pgid) {
  const groups = execFileSync("ps", ["-ax", "-o", "pgid="], { encoding: "utf8" });
  return groups
    .split(/\r?\n/)
    .some((value) => Number.parseInt(value.trim(), 10) === pgid);
}

function runScript(script) {
  return new Promise((resolveRun, reject) => {
    const child = spawn(process.execPath, [resolve(repoRoot, "scripts", script)], {
      cwd: repoRoot,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let output = "";
    child.stdout.on("data", (chunk) => { output += chunk; });
    child.stderr.on("data", (chunk) => { output += chunk; });
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error(`${script} did not reject an occupied port within 10 seconds`));
    }, 10_000);
    child.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on("exit", (code, signal) => {
      clearTimeout(timer);
      resolveRun({ code, signal, output });
    });
  });
}

for (const { script, port } of [
  { script: "agent-e2e.mjs", port: 7462 },
  { script: "fleet-e2e.mjs", port: 7482 },
]) {
  test(`${script} refuses an occupied port without killing its owner`, async () => {
    const server = await listen(port);
    try {
      const result = await runScript(script);
      assert.notEqual(result.code, 0, result.output);
      assert.equal(result.signal, null, result.output);
      assert.match(result.output, /already in use; refusing to terminate an unowned process/);
      assert.equal(await probe(port), "owned-by-test\n");
    } finally {
      await close(server);
    }
  });
}

test("owned process groups are signalled only while the leader identity matches", () => {
  const signals = [];
  const record = recordOwnedProcess(4242, { readIdentity: () => "start-token command" });
  assert.equal(signalOwnedProcessGroup(record, "SIGKILL", {
    readIdentity: () => "start-token command",
    signalProcess: (pid, signal) => signals.push({ pid, signal }),
  }), "signalled");
  assert.deepEqual(signals, [{ pid: -4242, signal: "SIGKILL" }]);

  assert.throws(() => signalOwnedProcessGroup(record, "SIGKILL", {
    readIdentity: () => "different-start-token unrelated-command",
    signalProcess: () => assert.fail("a recycled process must not be signalled"),
  }), /recycled process-group leader/);
  assert.equal(signalOwnedProcessGroup(record, "SIGKILL", {
    readIdentity: () => null,
    signalProcess: (_pid, signal) => {
      assert.equal(signal, 0);
      const error = new Error("group absent");
      error.code = "ESRCH";
      throw error;
    },
  }), "already-exited");

  const orphanSignals = [];
  assert.equal(signalOwnedProcessGroup(record, "SIGKILL", {
    readIdentity: () => null,
    signalProcess: (pid, signal) => orphanSignals.push({ pid, signal }),
  }), "signalled");
  assert.deepEqual(orphanSignals, [
    { pid: -4242, signal: 0 },
    { pid: -4242, signal: "SIGKILL" },
  ]);
});

test("owned process-group signal failures are never silent", () => {
  const record = recordOwnedProcess(4343, { readIdentity: () => "stable identity" });
  assert.throws(() => signalOwnedProcessGroup(record, "SIGKILL", {
    readIdentity: () => "stable identity",
    signalProcess: () => {
      const error = new Error("not permitted");
      error.code = "EPERM";
      throw error;
    },
  }), /EPERM/);
  assert.equal(signalOwnedProcessGroup(record, "SIGKILL", {
    readIdentity: () => "stable identity",
    signalProcess: () => {
      const error = new Error("gone");
      error.code = "ESRCH";
      throw error;
    },
  }), "already-exited");
});

test("an owned process group is reaped after its leader exits", {
  skip: process.platform === "win32",
  timeout: 10_000,
}, async () => {
  const leader = spawn("sh", ["-c", "sleep 0.25; sleep 30 & exit 0"], {
    detached: true,
    stdio: "ignore",
  });
  const record = recordOwnedProcess(leader.pid);
  try {
    await new Promise((resolveExit, reject) => {
      leader.once("error", reject);
      leader.once("exit", resolveExit);
    });
    assert.equal(processGroupHasMembers(record.pid), true);
    assert.equal(signalOwnedProcessGroup(record, "SIGKILL"), "signalled");
    const deadline = Date.now() + 5_000;
    while (processGroupHasMembers(record.pid)) {
      if (Date.now() > deadline) assert.fail("orphaned process group remained alive");
      await new Promise((resolveWait) => setTimeout(resolveWait, 50));
    }
  } finally {
    try { process.kill(-record.pid, "SIGKILL"); } catch {}
  }
});

test("a zombie group leader is treated as absent while its orphan is reaped", {
  skip: process.platform !== "linux",
  timeout: 10_000,
}, async () => {
  const leader = spawn("sh", ["-c", "sleep 0.1; sleep 30 & exit 0"], {
    detached: true,
    stdio: "ignore",
  });
  const record = recordOwnedProcess(leader.pid);
  const sleeper = new Int32Array(new SharedArrayBuffer(4));
  try {
    // Keep libuv from handling SIGCHLD so the exited leader remains a zombie
    // while the ownership helper inspects it.
    Atomics.wait(sleeper, 0, 0, 300);
    assert.equal(readProcessIdentity(record.pid), null);
    assert.equal(processGroupHasMembers(record.pid), true);
    assert.equal(signalOwnedProcessGroup(record, "SIGKILL"), "signalled");
    await new Promise((resolveExit, reject) => {
      leader.once("error", reject);
      leader.once("exit", resolveExit);
    });
    const deadline = Date.now() + 5_000;
    while (processGroupHasMembers(record.pid)) {
      if (Date.now() > deadline) assert.fail("zombie-led process group remained alive");
      await new Promise((resolveWait) => setTimeout(resolveWait, 50));
    }
  } finally {
    try { process.kill(-record.pid, "SIGKILL"); } catch {}
  }
});
