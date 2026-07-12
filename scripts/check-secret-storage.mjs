#!/usr/bin/env node
// Static release gate for secret-bearing local state. No npm dependencies.

import { readFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const failures = [];

function read(path) {
  return readFileSync(join(repoRoot, path), "utf8");
}

function requireMatch(path, pattern, reason) {
  if (!pattern.test(read(path))) failures.push(`${path}: ${reason}`);
}

const manifest = "app/android/app/src/main/AndroidManifest.xml";
requireMatch(manifest, /android:allowBackup="false"/, "android:allowBackup must be false");
requireMatch(
  manifest,
  /android:fullBackupContent="@xml\/backup_rules"/,
  "legacy full-backup rules must be explicit",
);
requireMatch(
  manifest,
  /android:dataExtractionRules="@xml\/data_extraction_rules"/,
  "API 31+ extraction rules must be explicit",
);

const legacyRules = "app/android/app/src/main/res/xml/backup_rules.xml";
for (const domain of ["root", "file", "database", "sharedpref", "external"]) {
  requireMatch(
    legacyRules,
    new RegExp(`<exclude\\s+domain=["']${domain}["']\\s+path=["']\\.["']\\s*/>`),
    `must exclude the complete ${domain} domain`,
  );
}

const extractionRules = "app/android/app/src/main/res/xml/data_extraction_rules.xml";
requireMatch(extractionRules, /<cloud-backup(?:\s[^>]*)?>[\s\S]*<\/cloud-backup>/, "cloud backup rules missing");
requireMatch(extractionRules, /<device-transfer>[\s\S]*<\/device-transfer>/, "device-transfer rules missing");
for (const section of ["cloud-backup", "device-transfer"]) {
  const body = read(extractionRules).match(new RegExp(`<${section}(?:\\s[^>]*)?>([\\s\\S]*?)</${section}>`))?.[1] ?? "";
  for (const domain of [
    "root",
    "file",
    "database",
    "sharedpref",
    "external",
    "device_root",
    "device_file",
    "device_database",
    "device_sharedpref",
  ]) {
    if (!new RegExp(`<exclude\\s+domain=["']${domain}["']\\s+path=["']\\.["']\\s*/>`).test(body)) {
      failures.push(`${extractionRules}: ${section} must exclude the complete ${domain} domain`);
    }
  }
}

requireMatch(
  "app/android/app/src/main/kotlin/com/incubtek/jeliya_app/MainActivity.kt",
  /File\(noBackupFilesDir,\s*"engine"\)/,
  "engine identity state must live under Android noBackupFilesDir",
);
requireMatch(
  "app/android/app/src/main/kotlin/com/incubtek/jeliya_app/MainActivity.kt",
  /legacyDir\.renameTo\(protectedDir\)/,
  "legacy file-backed identities must be migrated, not silently replaced",
);
requireMatch(
  "app/lib/main.dart",
  /invokeMethod<String>\('protectedEngineDataDir'\)/,
  "Flutter must request the protected Android engine directory",
);
requireMatch(
  "scripts/jeliya-agent.mjs",
  /dataDir:\s*defaultAgentDataDir\(\)/,
  "agent default data dir must use the platform data directory",
);
requireMatch(
  "scripts/e2e.mjs",
  /connected to ws:\/\/127\.0\.0\.1:\$\{port\}\/ws \(authenticated\)/,
  "E2E logs must identify the authenticated endpoint without printing its token",
);
if (/console\.log\([^\n]*\$\{url\}/.test(read("scripts/e2e.mjs"))) {
  failures.push("scripts/e2e.mjs: authenticated WebSocket URLs must not be printed");
}
requireMatch(
  "scripts/jeliya-agent.mjs",
  /installAgentDataGitGuard\(DATA_DIR\)/,
  "agent must install a per-directory Git guard before daemon startup",
);

const trackedResult = spawnSync("git", ["ls-files", "-z"], {
  cwd: repoRoot,
  encoding: "utf8",
});
if (trackedResult.status !== 0) {
  failures.push(`git ls-files failed: ${trackedResult.stderr.trim()}`);
} else {
  const secretDir = /^\.jeliya-(?:agent|data|demo|realnet|gatea)(?:[^/]*)$/;
  for (const path of trackedResult.stdout.split("\0").filter(Boolean)) {
    const parts = path.split("/");
    if (basename(path) === "identity.secret" || basename(path) === "daemon.json") {
      failures.push(`${path}: secret-bearing runtime file is tracked`);
    }
    if (parts.some((part) => secretDir.test(part))) {
      failures.push(`${path}: file under a secret-bearing runtime directory is tracked`);
    }
  }
}

const privateKeyPemPrefix = "-----BEGIN ";
const privateKeyPemSuffix = "PRIVATE KEY-----";
const privateKeyPemPattern = `${privateKeyPemPrefix}.*${privateKeyPemSuffix}`;
const privateKeySearch = spawnSync(
  "git",
  ["grep", "-Il", "-e", privateKeyPemPattern, "--"],
  { cwd: repoRoot, encoding: "utf8" },
);
if (privateKeySearch.status === 0) {
  for (const path of privateKeySearch.stdout.split(/\r?\n/).filter(Boolean)) {
    failures.push(`${path}: tracked private-key PEM material is forbidden`);
  }
} else if (privateKeySearch.status !== 1) {
  failures.push(`git grep for private-key PEM material failed: ${privateKeySearch.stderr.trim()}`);
}

for (const candidate of [
  ".jeliya-agent/identity.secret",
  ".jeliya-agent-builder-1/identity.secret",
  ".jeliya-data/identity.secret",
  ".jeliya-realnet-host/identity.secret",
  "identity.secret",
  "daemon.json",
  "release/evidence-ed25519-private.pem",
  "release/evidence-operator.key",
  "docs/evidence/v0.5.0/direct.private.pem",
]) {
  const ignored = spawnSync("git", ["check-ignore", "--no-index", "--quiet", candidate], {
    cwd: repoRoot,
  });
  if (ignored.status !== 0) failures.push(`.gitignore does not protect ${candidate}`);
}

if (failures.length > 0) {
  console.error("secret-storage: FAIL");
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}

console.log("secret-storage: PASS — Android backup/transfer and local identity guards are explicit");
