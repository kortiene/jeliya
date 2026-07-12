import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, win32 } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

import { defaultAgentDataDir, installAgentDataGitGuard } from "./agent-paths.mjs";

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));

test("agent defaults stay in platform data directories", () => {
  assert.equal(
    defaultAgentDataDir({ platform: "darwin", env: {}, home: "/Users/agent" }),
    join("/Users/agent", "Library", "Application Support", "Jeliya", "agents", "default"),
  );
  assert.equal(
    defaultAgentDataDir({ platform: "linux", env: { XDG_DATA_HOME: "/state" }, home: "/home/agent" }),
    join("/state", "jeliya", "agents", "default"),
  );
  assert.equal(
    defaultAgentDataDir({ platform: "linux", env: {}, home: "/home/agent" }),
    join("/home/agent", ".local", "share", "jeliya", "agents", "default"),
  );
  assert.equal(
    defaultAgentDataDir({ platform: "win32", env: { APPDATA: "C:/Profiles/agent/AppData/Roaming" }, home: "C:/Profiles/agent" }),
    win32.join("C:/Profiles/agent/AppData/Roaming", "Jeliya", "agents", "default"),
  );
});

test("relative or checkout-scoped environment paths cannot capture agent secrets", () => {
  assert.equal(
    defaultAgentDataDir({
      platform: "linux",
      env: { XDG_DATA_HOME: "." },
      home: "/home/agent",
      repositoryRoot: "/workspace/jeliya",
    }),
    "/home/agent/.local/share/jeliya/agents/default",
  );
  assert.equal(
    defaultAgentDataDir({
      platform: "linux",
      env: { XDG_DATA_HOME: "/workspace/jeliya/local-state" },
      home: "/home/agent",
      repositoryRoot: "/workspace/jeliya",
    }),
    "/home/agent/.local/share/jeliya/agents/default",
  );
  assert.equal(
    defaultAgentDataDir({
      platform: "win32",
      env: { APPDATA: "." },
      home: "C:\\Users\\agent",
      repositoryRoot: "C:\\work\\jeliya",
    }),
    "C:\\Users\\agent\\AppData\\Roaming\\Jeliya\\agents\\default",
  );
  assert.throws(
    () => defaultAgentDataDir({
      platform: "linux",
      env: {},
      home: "relative-home",
      repositoryRoot: "/workspace/jeliya",
    }),
    /home directory must be absolute/,
  );
});

test("an explicit data directory installs a deny-all Git guard", () => {
  const root = mkdtempSync(join(tmpdir(), "jeliya-agent-git-guard-"));
  try {
    const init = spawnSync("git", ["init", "--quiet"], { cwd: root });
    assert.equal(init.status, 0);
    const dataDir = join(root, "custom-agent-state");
    const marker = installAgentDataGitGuard(dataDir);
    const body = readFileSync(marker, "utf8");
    assert.match(body, /^\*/m);
    assert.match(body, /^!\.gitignore$/m);

    const ignored = spawnSync(
      "git",
      ["check-ignore", "--no-index", "--quiet", "custom-agent-state/identity.secret"],
      { cwd: root },
    );
    assert.equal(ignored.status, 0, "identity.secret must be ignored inside a custom data dir");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("an unsafe pre-existing marker fails closed", () => {
  const root = mkdtempSync(join(tmpdir(), "jeliya-agent-unsafe-guard-"));
  try {
    writeFileSync(join(root, ".gitignore"), "node_modules/\n", "utf8");
    assert.throws(
      () => installAgentDataGitGuard(root),
      /not the exact deny-all policy; refusing to start/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a later negation cannot reopen identity.secret", () => {
  const root = mkdtempSync(join(tmpdir(), "jeliya-agent-reopened-guard-"));
  try {
    writeFileSync(join(root, ".gitignore"), "*\n!.gitignore\n!identity.secret\n", "utf8");
    assert.throws(
      () => installAgentDataGitGuard(root),
      /not the exact deny-all policy; refusing to start/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("repository secret-storage gate passes", () => {
  const checked = spawnSync(process.execPath, ["scripts/check-secret-storage.mjs"], {
    cwd: repoRoot,
    encoding: "utf8",
  });
  assert.equal(checked.status, 0, `${checked.stdout}\n${checked.stderr}`);
  assert.match(checked.stdout, /secret-storage: PASS/);
});
