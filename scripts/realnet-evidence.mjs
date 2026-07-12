#!/usr/bin/env node
// Revision-bound Jeliya network verification harness.
//
// The harness is deliberately driven from one trusted operator machine. A
// remote host runs only a pre-verified `jeliyad` under a supervised SSH
// session; its loopback HTTP/WebSocket control plane is reached through an SSH
// tunnel. Remote Node, package installation, firewall changes, and persistent
// services are neither required nor permitted.
//
// Local machinery check (non-certifying):
//   node scripts/realnet-evidence.mjs --local-dryrun [--with-third]
//
// Certifying different-network direct-path candidate run. The independently
// supplied digest must identify the already verified Zig 0.15.2 executable:
//   node scripts/realnet-evidence.mjs \
//     --remote user@kilo \
//     --third-remote user@stargate-03 \
//     --build-from-source \
//     --zig-sha256 <64-hex-verified-zig-executable-digest> \
//     --expect-path direct
//
// Supplying --local-bin/--linux-bin instead remains useful for diagnostics but
// is never certifiable because the harness cannot bind those bytes to source.
//
// A relay run uses binaries compiled with the non-default
// `relay-only-test` Cargo feature throughout the Jeliya/iroh-rooms stack and
// adds `--relay-only-build --expect-path relay` to the source-build command.
// There is no runtime switch: ordinary releases retain direct-capable behavior.

import { execFileSync, spawn, spawnSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { createServer as createHttpServer } from "node:http";
import { createConnection, createServer as createTcpServer, isIP } from "node:net";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { parseArgs, pollUntil } from "./realnet-lib.mjs";

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), "..");
const DEFAULT_LOCAL_BIN = join(REPO_ROOT, "target", "release", "jeliyad");
const DEFAULT_DEBUG_BIN = join(REPO_ROOT, "target", "debug", "jeliyad");
const EVIDENCE_ROOT = process.env.JELIYA_EVIDENCE_ROOT
  ? resolve(process.env.JELIYA_EVIDENCE_ROOT)
  : join(REPO_ROOT, ".jeliya-gatea", "v0.5.0");
const WAIT_MS = 120_000;
const LINUX_TARGET = "x86_64-unknown-linux-musl";
const REQUIRED_RUST_TOOLCHAIN = "1.91.0";
const REQUIRED_NODE_VERSION = "v22.22.3";
const REQUIRED_ZIG_VERSION = "0.15.2";
const REQUIRED_CARGO_ZIGBUILD_VERSION = "cargo-zigbuild 0.23.0";
export const RELAY_ONLY_VERIFICATION_MARKER = "jeliya-relay-only-test-build-v1";
const SSH_BASE = [
  "-o", "BatchMode=yes",
  "-o", "ConnectTimeout=12",
  "-o", "ServerAliveInterval=15",
  "-o", "ServerAliveCountMax=2",
];

const sleep = (ms) => new Promise((resolvePromise) => setTimeout(resolvePromise, ms));

function die(message) {
  const error = new Error(message);
  error.exitCode = 2;
  throw error;
}

export function validSshTarget(value) {
  return typeof value === "string" && /^[A-Za-z0-9][A-Za-z0-9._@-]*$/.test(value);
}

export function validRunId(value) {
  return typeof value === "string" && /^\d{8}T\d{6}Z-[0-9a-f]{8}$/.test(value);
}

export function remoteRunDir(runId, role) {
  if (!validRunId(runId) || !/^[abc]$/.test(role)) {
    throw new Error("invalid generated run id or role");
  }
  return `/tmp/jeliya-v050-${runId}-${role}`;
}

function validateRemoteRunDir(runId, runDir) {
  const role = typeof runDir === "string" ? runDir.at(-1) : "";
  if (!/^[abc]$/.test(role) || remoteRunDir(runId, role) !== runDir) {
    throw new Error("remote run directory is not an exact generated path");
  }
  return runDir;
}

function validatePrivilegeDrop(privilegeDrop) {
  if (privilegeDrop == null) return null;
  const uid = String(privilegeDrop.uid);
  const gid = String(privilegeDrop.gid);
  if (!/^\d+$/.test(uid) || !/^\d+$/.test(gid) || uid === "0" || gid === "0") {
    throw new Error("privilege-drop uid/gid must be non-root decimal identifiers");
  }
  return { uid, gid };
}

export function remoteDaemonCommand(runId, runDir, privilegeDrop = null) {
  const validated = validateRemoteRunDir(runId, runDir);
  const drop = validatePrivilegeDrop(privilegeDrop);
  const executable = drop
    ? `setpriv --reuid=${drop.uid} --regid=${drop.gid} --clear-groups '${validated}/jeliyad'`
    : `'${validated}/jeliyad'`;
  return [
    "umask 077",
    `printf '%s\\n' "$$" > '${validated}/jeliyad.pid'`,
    `exec ${executable} --supervised --no-open --port 0 --data-dir '${validated}/state'`,
  ].join(" && ");
}

export function remoteCleanupCommand(runId, runDir) {
  const validated = validateRemoteRunDir(runId, runDir);
  const pidFile = `${validated}/jeliyad.pid`;
  const expectedExe = `${validated}/jeliyad`;
  return [
    `run_dir='${validated}'`,
    `pid_file='${pidFile}'`,
    `expected_exe='${expectedExe}'`,
    `case "$run_dir" in '/tmp/jeliya-v050-${runId}-'[abc]) ;; *) exit 90 ;; esac`,
    '[ ! -e "$run_dir" ] && exit 0',
    `owner=$(cat -- '${validated}/.jeliya-run-owner' 2>/dev/null) || exit 97`,
    `[ "$owner" = '${runId}' ] || exit 97`,
    '[ -d /proc/1 ] || exit 96',
    'pid=$(cat -- "$pid_file" 2>/dev/null) || exit 91',
    'case "$pid" in ""|*[!0-9]*) exit 91 ;; esac',
    'if [ -e "/proc/$pid" ]; then',
    '  actual_exe=$(readlink "/proc/$pid/exe" 2>/dev/null) || exit 92',
    '  [ "$actual_exe" = "$expected_exe" ] || exit 92',
    '  kill -TERM "$pid" 2>/dev/null || true',
    '  attempts=0',
    '  while [ -e "/proc/$pid" ] && [ "$attempts" -lt 50 ]; do sleep 0.1; attempts=$((attempts + 1)); done',
    '  if [ -e "/proc/$pid" ]; then',
    '    actual_exe=$(readlink "/proc/$pid/exe" 2>/dev/null) || exit 93',
    '    [ "$actual_exe" = "$expected_exe" ] || exit 93',
    '    kill -KILL "$pid" 2>/dev/null || exit 93',
    '    attempts=0',
    '    while [ -e "/proc/$pid" ] && [ "$attempts" -lt 50 ]; do sleep 0.1; attempts=$((attempts + 1)); done',
    '  fi',
    'fi',
    '[ ! -e "/proc/$pid" ] || exit 94',
    'find "$run_dir" -depth -delete',
    '[ ! -e "$run_dir" ] || exit 95',
  ].join("\n");
}

export function remoteOwnedDirectoryCleanupCommand(runId, runDir) {
  const validated = validateRemoteRunDir(runId, runDir);
  return [
    `run_dir='${validated}'`,
    `case "$run_dir" in '/tmp/jeliya-v050-${runId}-'[abc]) ;; *) exit 90 ;; esac`,
    '[ ! -e "$run_dir" ] && exit 0',
    '[ -d "$run_dir" ] || exit 97',
    `owner=$(cat -- '${validated}/.jeliya-run-owner' 2>/dev/null) || exit 97`,
    `[ "$owner" = '${runId}' ] || exit 97`,
    'find "$run_dir" -depth -delete',
    '[ ! -e "$run_dir" ] || exit 95',
  ].join("\n");
}

export function remoteCreateCommand(runId, runDir) {
  const validated = validateRemoteRunDir(runId, runDir);
  return [
    "set -eu",
    "umask 077",
    `mkdir -- '${validated}'`,
    `printf '%s\\n' '${runId}' > '${validated}/.jeliya-run-owner'`,
    "printf 'RUN_DIR_CREATED\\n'",
    `mkdir -- '${validated}/state'`,
  ].join("\n");
}

export function remoteBinaryVerificationCommand(
  runId,
  runDir,
  expectedSha,
  privilegeDrop = null,
) {
  const validated = validateRemoteRunDir(runId, runDir);
  const drop = validatePrivilegeDrop(privilegeDrop);
  if (!/^[0-9a-f]{64}$/.test(expectedSha)) {
    throw new Error("expected remote binary digest must be 64 lowercase hex characters");
  }
  const binary = `${validated}/jeliyad`;
  const runner = drop
    ? `run_as_peer() { setpriv --reuid=${drop.uid} --regid=${drop.gid} --clear-groups "$@"; }`
    : 'run_as_peer() { "$@"; }';
  return [
    "set -eu",
    `binary='${binary}'`,
    runner,
    'chmod 700 "$binary"',
    'digest=$(sha256sum "$binary" | awk \'{print $1}\')',
    `if [ "$digest" != '${expectedSha}' ]; then printf 'SHA256=%s\\n' "$digest"; exit 98; fi`,
    'execution_uid=$(run_as_peer id -u)',
    'version=$(run_as_peer "$binary" --version)',
    "set +e",
    'relay_output=$(run_as_peer "$binary" --verification-relay-only-build 2>/dev/null)',
    "relay_status=$?",
    "set -e",
    'relay_hex=$(printf \'%s\' "$relay_output" | od -An -tx1 | tr -d \' \\n\')',
    'printf \'SHA256=%s\\nVERSION=%s\\nEXECUTION_UID=%s\\nRELAY_STATUS=%s\\nRELAY_STDOUT_HEX=%s\\n\' "$digest" "$version" "$execution_uid" "$relay_status" "$relay_hex"',
  ].join("\n");
}

export function parseRemoteBinaryVerification(
  stdout,
  { expectedSha, expectedVersion, expectedExecutionUid, expectedRelayOnly },
) {
  const lines = String(stdout).trimEnd().split(/\r?\n/);
  if (lines.length !== 5) throw new Error("remote binary verification returned an invalid record");
  const value = (prefix, line) => {
    if (!line.startsWith(prefix)) throw new Error("remote binary verification returned an invalid record");
    return line.slice(prefix.length);
  };
  const sha256 = value("SHA256=", lines[0]);
  const versionWire = value("VERSION=", lines[1]);
  const executionUid = value("EXECUTION_UID=", lines[2]);
  const relayStatusWire = value("RELAY_STATUS=", lines[3]);
  const relayStdoutHex = value("RELAY_STDOUT_HEX=", lines[4]);
  if (!/^[0-9a-f]{64}$/.test(sha256) || sha256 !== expectedSha) {
    throw new Error("remote binary digest mismatch");
  }
  if (versionWire !== `jeliyad ${expectedVersion}`) {
    throw new Error("remote binary version mismatch");
  }
  if (!/^\d+$/.test(executionUid) || executionUid !== String(expectedExecutionUid)) {
    throw new Error("remote binary execution uid mismatch");
  }
  if (!/^\d+$/.test(relayStatusWire)) {
    throw new Error("remote relay attestation returned an invalid status");
  }
  const relayStatus = Number(relayStatusWire);
  const expectedMarkerHex = Buffer.from(RELAY_ONLY_VERIFICATION_MARKER, "utf8").toString("hex");
  const relayOnlyAttested = relayStatus === 0 && relayStdoutHex === expectedMarkerHex;
  if (expectedRelayOnly && !relayOnlyAttested) {
    throw new Error("remote binary lacks the compile-time relay-only attestation");
  }
  if (!expectedRelayOnly && (relayStatus !== 2 || relayStdoutHex !== "")) {
    throw new Error("ordinary remote binary accepted or emitted relay-only attestation output");
  }
  return {
    sha256,
    version: expectedVersion,
    execution_uid: executionUid,
    relay_only_attested: relayOnlyAttested,
    relay_attestation_exit_status: relayStatus,
  };
}

export function parseExpectedSha256(value) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error("a 64-hex --linux-sha256 value or sidecar path is required");
  }
  const direct = value.trim().toLowerCase();
  if (/^[0-9a-f]{64}$/.test(direct)) return direct;
  const source = readFileSync(resolve(value), "utf8");
  const lines = source.split(/\r?\n/).filter((line) => line.trim() !== "");
  if (lines.length !== 1) throw new Error("checksum sidecar must have one non-empty line");
  const match = /^([0-9a-fA-F]{64})  ([^\s]+)$/.exec(lines[0]);
  if (!match) throw new Error("checksum sidecar has an invalid format");
  return match[1].toLowerCase();
}

export function sha256File(path) {
  const hash = createHash("sha256");
  hash.update(readFileSync(path));
  return hash.digest("hex");
}

export function parseRelayBuildAttestation(status, stdout) {
  return status === 0 && String(stdout).trim() === RELAY_ONLY_VERIFICATION_MARKER;
}

function binaryAttestsRelayOnly(path) {
  const result = spawnSync(path, ["--verification-relay-only-build"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  return parseRelayBuildAttestation(result.status, result.stdout);
}

export function classifyNetworks(localAddress, remoteAddress) {
  const localFamily = isIP(localAddress);
  const remoteFamily = isIP(remoteAddress);
  if (localFamily === 0 || remoteFamily === 0) {
    return { status: "indeterminate", family: null };
  }
  if (localFamily !== remoteFamily) return { status: "different", family: "mixed" };
  const family = localFamily === 6 ? "ipv6" : "ipv4";
  if (family === "ipv6") {
    const prefix = (value) => {
      const canonical = new URL(`http://[${value}]/`).hostname.slice(1, -1);
      const [left, right = ""] = canonical.split("::");
      const leftGroups = left === "" ? [] : left.split(":");
      const rightGroups = right === "" ? [] : right.split(":");
      const zeros = Array.from({ length: 8 - leftGroups.length - rightGroups.length }, () => "0");
      return [...leftGroups, ...zeros, ...rightGroups]
        .slice(0, 4)
        .map((group) => group.padStart(4, "0"))
        .join(":");
    };
    return { status: prefix(localAddress) === prefix(remoteAddress) ? "same" : "different", family };
  }
  return { status: localAddress === remoteAddress ? "same" : "different", family };
}

export function pathSummary(peers) {
  const connected = peers.filter((peer) => peer.state === "connected" && peer.path);
  const counts = { direct: 0, relay: 0, other: 0 };
  for (const peer of connected) {
    if (peer.path === "direct") counts.direct += 1;
    else if (peer.path === "relay") counts.relay += 1;
    else counts.other += 1;
  }
  return { connected: connected.length, ...counts };
}

export function pathMatches(peers, expected, minimumPeers = 1) {
  const summary = pathSummary(peers);
  if (summary.connected < minimumPeers || summary.other !== 0) return false;
  if (expected === "direct") return summary.direct === summary.connected;
  if (expected === "relay") return summary.relay === summary.connected;
  return expected === "any";
}

export function pathMatchesExpectedIdentities(peers, expected, identityIds) {
  if (!Array.isArray(identityIds) || identityIds.length === 0) return false;
  return identityIds.every((identityId) => {
    const rows = peers.filter((peer) => peer.identity_id === identityId);
    return rows.length === 1
      && rows[0].state === "connected"
      && rows[0].path === expected;
  });
}

export function assertEvidenceContainsNoSecrets(evidence, secrets) {
  const encoded = JSON.stringify(evidence);
  for (const secret of secrets) {
    if (typeof secret === "string" && secret.length >= 8 && encoded.includes(secret)) {
      throw new Error("refusing to write evidence containing an in-memory secret");
    }
  }
  const forbiddenKeys = /(?:auth[_-]?token|bearer|ticket|identity[_-]?seed|private[_-]?key)/i;
  const visit = (value) => {
    if (Array.isArray(value)) return value.forEach(visit);
    if (!value || typeof value !== "object") return;
    for (const [key, child] of Object.entries(value)) {
      if (forbiddenKeys.test(key)) throw new Error(`forbidden evidence key: ${key}`);
      visit(child);
    }
  };
  visit(evidence);
}

export function redactLogExcerpt(value, secrets = new Set()) {
  let redacted = String(value);
  for (const secret of secrets) {
    if (typeof secret === "string" && secret.length >= 8) {
      redacted = redacted.split(secret).join("[REDACTED_SECRET]");
    }
  }
  redacted = redacted
    .replace(/("(?:auth[_-]?token|ticket|identity[_-]?seed|private[_-]?key)"\s*:\s*")[^"]*(")/gi, "$1[REDACTED]$2")
    .replace(/\bBearer\s+[^\s"']+/gi, "Bearer [REDACTED]")
    .replace(/([?&](?:token|ticket|auth|bearer)=)[^&\s"']+/gi, "$1[REDACTED]")
    .replace(/((?:auth[_-]?token|ticket|identity[_-]?seed|private[_-]?key)\s*[:=]\s*)[^\s,}"']+/gi, "$1[REDACTED]")
    .replace(/\b[0-9a-f]{32,}\b/gi, "[REDACTED_LONG_HEX]")
    .replace(/\b[A-Za-z0-9+/_-]{40,}={0,2}\b/g, "[REDACTED_LONG_TOKEN]");
  return redacted.slice(0, 1_024);
}

function attachLogCollectors(proc) {
  const make = () => ({
    bytes: 0,
    newlineCount: 0,
    endsWithNewline: false,
    hash: createHash("sha256"),
    excerptRaw: "",
    excerptInputBytes: 0,
  });
  const collectors = { stdout: make(), stderr: make() };
  for (const [streamName, stream] of [["stdout", proc.stdout], ["stderr", proc.stderr]]) {
    stream.on("data", (chunk) => {
      const text = String(chunk);
      const bytes = Buffer.byteLength(text);
      const collector = collectors[streamName];
      collector.bytes += bytes;
      collector.newlineCount += (text.match(/\n/g) ?? []).length;
      collector.endsWithNewline = text.endsWith("\n");
      collector.hash.update(text);
      if (collector.excerptRaw.length < 4_096) {
        const remaining = 4_096 - collector.excerptRaw.length;
        const accepted = text.slice(0, remaining);
        collector.excerptRaw += accepted;
        collector.excerptInputBytes += Buffer.byteLength(accepted);
      }
    });
  }
  return collectors;
}

function summarizeLogCollector(collector, secrets) {
  return {
    lines: collector.newlineCount + (collector.bytes > 0 && !collector.endsWithNewline ? 1 : 0),
    bytes: collector.bytes,
    sha256: collector.hash.digest("hex"),
    redacted_excerpt: redactLogExcerpt(collector.excerptRaw, secrets),
    excerpt_truncated: collector.bytes > collector.excerptInputBytes,
  };
}

function git(args) {
  return execFileSync("git", args, { cwd: REPO_ROOT, encoding: "utf8" }).trim();
}

function gitAt(cwd, args) {
  return execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
}

export function isPublicGitSource(value) {
  if (typeof value !== "string" || value.trim() === "") return false;
  if (/^git@github\.com:[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+(?:\.git)?$/i.test(value)) return true;
  try {
    const parsed = new URL(value);
    return parsed.protocol === "https:"
      && parsed.hostname.toLowerCase() === "github.com"
      && parsed.username === ""
      && parsed.password === ""
      && parsed.search === ""
      && parsed.hash === "";
  } catch {
    return false;
  }
}

function requireCredentialFreeGitSource(value, label) {
  if (/[?#]/.test(value)) {
    throw new Error(`${label} must not contain URL credentials`);
  }
  try {
    const parsed = new URL(value);
    if (
      parsed.username !== ""
      || parsed.password !== ""
      || parsed.search !== ""
      || parsed.hash !== ""
    ) {
      throw new Error(`${label} must not contain URL credentials`);
    }
  } catch (error) {
    if (error.message?.includes("must not contain URL credentials")) throw error;
    // SCP-like SSH remotes (for example git@github.com:owner/repo.git) do not
    // contain URL passwords and are safe to identify in evidence.
  }
  return value;
}

function publicGitReadUrl(value) {
  const scpLike = /^git@github\.com:([^/\s]+\/[^\s]+)$/i.exec(value);
  if (scpLike) return `https://github.com/${scpLike[1]}`;
  return value;
}

export function commitPublishedAtOrigin(origin, commit) {
  if (!isPublicGitSource(origin) || !/^[0-9a-f]{40}$/.test(commit)) return false;
  const result = spawnSync(
    "git",
    ["ls-remote", "--refs", publicGitReadUrl(origin)],
    {
      encoding: "utf8",
      env: { ...process.env, GIT_TERMINAL_PROMPT: "0" },
      timeout: 30_000,
      stdio: ["ignore", "pipe", "ignore"],
    },
  );
  if (result.status !== 0) return false;
  return String(result.stdout)
    .split(/\r?\n/)
    .some((line) => line.startsWith(`${commit}\t`));
}

function localPathFromGitSource(value) {
  try {
    const parsed = new URL(value);
    return parsed.protocol === "file:" ? fileURLToPath(parsed) : null;
  } catch {
    return null;
  }
}

export function dependencyIdentity(cargoToml, lock) {
  const declaration = cargoToml.match(/^iroh-rooms\s*=\s*\{([^}]*)\}/m)?.[1];
  if (!declaration) throw new Error("could not find the iroh-rooms workspace dependency");
  const field = (name) => declaration.match(new RegExp(`\\b${name}\\s*=\\s*"([^"]+)"`))?.[1];
  const gitUrl = field("git");
  const requestedRevision = field("rev");
  const pathValue = field("path");

  if (gitUrl) {
    requireCredentialFreeGitSource(gitUrl, "iroh-rooms git source");
    if (!requestedRevision || !/^[0-9a-f]{40}$/.test(requestedRevision)) {
      throw new Error("iroh-rooms git dependencies must use an exact 40-hex rev");
    }
    const lockedSources = [...lock.matchAll(/^source = "git\+([^"]+)"$/gm)].map((match) => match[1]);
    const expectedLockedSource = `${gitUrl}?rev=${requestedRevision}#${requestedRevision}`;
    const locked = lockedSources.find((source) => source === expectedLockedSource);
    if (!locked) throw new Error("Cargo.lock does not resolve the declared iroh-rooms revision");
    const localPath = localPathFromGitSource(gitUrl);
    const local = localPath ? {
      commit: gitAt(localPath, ["rev-parse", "HEAD"]),
      dirty: gitAt(localPath, ["status", "--porcelain"]) !== "",
      origin: requireCredentialFreeGitSource(
        gitAt(localPath, ["config", "--get", "remote.origin.url"]),
        "local iroh-rooms origin",
      ),
    } : null;
    if (local && local.commit !== requestedRevision) {
      throw new Error("local iroh-rooms git source HEAD does not match the declared rev");
    }
    const publicSource = isPublicGitSource(gitUrl);
    return {
      kind: localPath ? "local-git-url" : "git",
      source: gitUrl,
      requested_revision: requestedRevision,
      resolved_revision: requestedRevision,
      public_source: publicSource,
      local_checkout: local,
      releaseable: publicSource,
    };
  }

  if (pathValue) {
    const localPath = resolve(REPO_ROOT, pathValue);
    const commit = gitAt(localPath, ["rev-parse", "HEAD"]);
    return {
      kind: "local-path",
      source: localPath,
      requested_revision: null,
      resolved_revision: commit,
      public_source: false,
      local_checkout: {
        commit,
        dirty: gitAt(localPath, ["status", "--porcelain"]) !== "",
        origin: requireCredentialFreeGitSource(
          gitAt(localPath, ["config", "--get", "remote.origin.url"]),
          "local iroh-rooms origin",
        ),
      },
      releaseable: false,
    };
  }

  throw new Error("iroh-rooms must be revision-pinned by git or explicitly identified as a local source");
}

export function sourceIdentity() {
  const cargoToml = readFileSync(join(REPO_ROOT, "Cargo.toml"), "utf8");
  const lock = readFileSync(join(REPO_ROOT, "Cargo.lock"), "utf8");
  const dependency = dependencyIdentity(cargoToml, lock);
  const origin = requireCredentialFreeGitSource(
    git(["config", "--get", "remote.origin.url"]),
    "Jeliya origin",
  );
  const dirty = git(["status", "--porcelain"]) !== "";
  const commit = git(["rev-parse", "HEAD"]);
  const sourcePublished = commitPublishedAtOrigin(origin, commit);
  const dependencyPublished = dependency.public_source
    && commitPublishedAtOrigin(dependency.source, dependency.resolved_revision);
  return {
    commit,
    dirty,
    origin,
    public_source: isPublicGitSource(origin),
    published_at_origin: sourcePublished,
    iroh_rooms_revision: dependency.resolved_revision,
    iroh_rooms: {
      ...dependency,
      published_at_origin: dependencyPublished,
    },
    releaseable: !dirty
      && isPublicGitSource(origin)
      && sourcePublished
      && dependency.releaseable
      && dependencyPublished,
  };
}

function binaryVersion(path) {
  const output = execFileSync(path, ["--version"], { encoding: "utf8" }).trim();
  const match = /^jeliyad\s+(\d+\.\d+\.\d+)$/.exec(output);
  if (!match) throw new Error(`${path} returned an unexpected --version response`);
  return match[1];
}

function verifyBinaryFile(path) {
  const absolute = resolve(path);
  if (!existsSync(absolute) || !statSync(absolute).isFile()) {
    throw new Error(`binary is missing: ${absolute}`);
  }
  return {
    path: absolute,
    filename: basename(absolute),
    sha256: sha256File(absolute),
  };
}

function verifyLocalBinary(path, expectedVersion, expectedRelayOnly) {
  const binary = verifyBinaryFile(path);
  const version = binaryVersion(binary.path);
  if (version !== expectedVersion) {
    throw new Error(`${binary.path} reports ${version}, expected ${expectedVersion}`);
  }
  const relayOnlyAttested = binaryAttestsRelayOnly(binary.path);
  if (relayOnlyAttested !== expectedRelayOnly) {
    throw new Error(
      expectedRelayOnly
        ? `${binary.path} lacks the compile-time relay-only attestation`
        : `${binary.path} unexpectedly attests as a relay-only diagnostic build`,
    );
  }
  return {
    ...binary,
    version,
    relay_only_attested: relayOnlyAttested,
  };
}

function commandIdentity(name, versionArgs) {
  const path = execFileSync("which", [name], { encoding: "utf8" }).trim();
  if (!path) throw new Error(`required build tool is missing: ${name}`);
  const version = execFileSync(path, versionArgs, { encoding: "utf8" }).trim();
  return { filename: basename(path), version, sha256: sha256File(path) };
}

function rustToolIdentity(name, versionArgs) {
  const rustup = execFileSync("which", ["rustup"], { encoding: "utf8" }).trim();
  if (!rustup) throw new Error("rustup is required for the pinned source build");
  const path = execFileSync(
    rustup,
    ["which", "--toolchain", REQUIRED_RUST_TOOLCHAIN, name],
    { encoding: "utf8" },
  ).trim();
  const version = execFileSync(path, versionArgs, { encoding: "utf8" }).trim();
  return { filename: basename(path), version, sha256: sha256File(path) };
}

function embeddedUiReady(sourceRoot) {
  const root = join(sourceRoot, "ui", "dist");
  const index = join(root, "index.html");
  if (!existsSync(index) || statSync(index).size === 0) return false;
  const countFiles = (directory) => readdirSync(directory, { withFileTypes: true }).reduce(
    (count, entry) => count + (entry.isDirectory() ? countFiles(join(directory, entry.name)) : 1),
    0,
  );
  return countFiles(root) > 1;
}

function runBuildCommand(command, args, { cwd, env = process.env, timeoutMs = 30 * 60_000 }) {
  const result = spawnSync(command, args, {
    cwd,
    env,
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 16 * 1024 * 1024,
    timeout: timeoutMs,
  });
  if (result.status !== 0) {
    const raw = `${String(result.stdout ?? "")}\n${String(result.stderr ?? "")}`;
    const tail = raw.slice(-4_096);
    const diagnostic = redactLogExcerpt(tail).replace(/\s+/g, " ").trim();
    throw new Error(
      `source build failed: ${command} ${args[0]} (${result.signal ?? `exit ${result.status}`})${diagnostic ? ` — ${diagnostic}` : ""}`,
    );
  }
}

function runCargoBuild(args, env, sourceRoot) {
  runBuildCommand("cargo", [`+${REQUIRED_RUST_TOOLCHAIN}`, ...args], { cwd: sourceRoot, env });
}

export function validBuildDirectory(path, runId) {
  if (!validRunId(runId) || typeof path !== "string") return false;
  const prefix = join(tmpdir(), `jeliya-v050-${runId}-build-`);
  return path.startsWith(prefix)
    && /^[A-Za-z0-9]{6}$/.test(path.slice(prefix.length));
}

function removeBuildDirectory(path, runId) {
  if (!validBuildDirectory(path, runId)) throw new Error("refusing to remove an unowned build directory");
  rmSync(path, { recursive: true, force: true });
  return !existsSync(path);
}

function buildCandidateFromSource({ runId, relayOnlyBuild, zigSha256, sourceCommit }) {
  const expectedZigSha = parseExpectedSha256(zigSha256);
  const rustc = rustToolIdentity("rustc", ["--version", "--verbose"]);
  const cargo = rustToolIdentity("cargo", ["--version"]);
  const node = commandIdentity("node", ["--version"]);
  const npm = commandIdentity("npm", ["--version"]);
  const zig = commandIdentity("zig", ["version"]);
  const cargoZigbuild = commandIdentity("cargo-zigbuild", ["-V"]);
  if (!rustc.version.startsWith(`rustc ${REQUIRED_RUST_TOOLCHAIN} `)) {
    throw new Error(`rustc ${REQUIRED_RUST_TOOLCHAIN} is required, found ${rustc.version.split("\n")[0]}`);
  }
  if (!cargo.version.startsWith(`cargo ${REQUIRED_RUST_TOOLCHAIN} `)) {
    throw new Error(`cargo ${REQUIRED_RUST_TOOLCHAIN} is required, found ${cargo.version}`);
  }
  if (node.version !== REQUIRED_NODE_VERSION) {
    throw new Error(`Node ${REQUIRED_NODE_VERSION} is required for the source build, found ${node.version}`);
  }
  if (zig.version !== REQUIRED_ZIG_VERSION) {
    throw new Error(`zig ${REQUIRED_ZIG_VERSION} is required, found ${zig.version}`);
  }
  if (zig.sha256 !== expectedZigSha) {
    throw new Error("the Zig executable does not match the independently supplied SHA-256");
  }
  if (cargoZigbuild.version !== REQUIRED_CARGO_ZIGBUILD_VERSION) {
    throw new Error(`${REQUIRED_CARGO_ZIGBUILD_VERSION} is required, found ${cargoZigbuild.version}`);
  }

  const targetDir = mkdtempSync(join(tmpdir(), `jeliya-v050-${runId}-build-`));
  const sourceRoot = join(targetDir, "source");
  const cargoTargetDir = join(targetDir, "target");
  const archivePath = join(targetDir, "source.tar");
  const featureList = ["embed-ui", ...(relayOnlyBuild ? ["relay-only-test"] : [])];
  const features = featureList.join(",");
  const nativeArgs = ["build", "--locked", "--release", "-p", "jeliyad", "--features", features];
  const linuxArgs = [
    "zigbuild", "--locked", "--release", "-p", "jeliyad", "--features", features,
    "--target", LINUX_TARGET,
  ];
  const env = { ...process.env, CARGO_TARGET_DIR: cargoTargetDir };
  try {
    mkdirSync(sourceRoot, { mode: 0o700 });
    execFileSync(
      "git",
      ["archive", "--format=tar", "--output", archivePath, sourceCommit],
      { cwd: REPO_ROOT },
    );
    runBuildCommand("tar", ["-xf", archivePath, "-C", sourceRoot], { cwd: targetDir });
    rmSync(archivePath, { force: true });

    const packageLock = join(sourceRoot, "ui", "package-lock.json");
    if (!existsSync(packageLock)) throw new Error("the committed UI package-lock.json is missing");
    const packageLockSha256 = sha256File(packageLock);
    runBuildCommand("npm", ["ci"], { cwd: join(sourceRoot, "ui") });
    runBuildCommand("npm", ["run", "build"], { cwd: join(sourceRoot, "ui") });
    if (!embeddedUiReady(sourceRoot)) {
      throw new Error("the source-built ui/dist is missing or incomplete");
    }

    runCargoBuild(nativeArgs, env, sourceRoot);
    runCargoBuild(linuxArgs, env, sourceRoot);
    const localPath = join(cargoTargetDir, "release", "jeliyad");
    const linuxPath = join(cargoTargetDir, LINUX_TARGET, "release", "jeliyad");
    if (!existsSync(localPath) || !existsSync(linuxPath)) {
      throw new Error("source build completed without the expected binary set");
    }
    return {
      targetDir,
      localPath,
      linuxPath,
      linuxSha256: sha256File(linuxPath),
      evidence: {
        mode: "from-source",
        source_bound: true,
        source_snapshot_commit: sourceCommit,
        locked: true,
        embedded_ui: {
          built_from_source: true,
          package_lock_sha256: packageLockSha256,
        },
        features: featureList,
        targets: [rustc.version.match(/^host:\s*(.+)$/m)?.[1] ?? "native-host", LINUX_TARGET],
        commands: [
          `git archive ${sourceCommit}`,
          "npm ci",
          "npm run build",
          `cargo +${REQUIRED_RUST_TOOLCHAIN} ${nativeArgs.join(" ")}`,
          `cargo +${REQUIRED_RUST_TOOLCHAIN} ${linuxArgs.join(" ")}`,
        ],
        toolchain: {
          rustc,
          cargo,
          node,
          npm,
          zig: { ...zig, expected_sha256: expectedZigSha, integrity_verified: true },
          cargo_zigbuild: cargoZigbuild,
        },
      },
    };
  } catch (error) {
    try { removeBuildDirectory(targetDir, runId); } catch {}
    throw error;
  }
}

function runCaptured(
  command,
  argv,
  { timeoutMs = 30_000, input = undefined, signal = undefined } = {},
) {
  return new Promise((resolvePromise) => {
    const proc = spawn(command, argv, { stdio: ["pipe", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    let settled = false;
    let termTimer = null;
    let killTimer = null;
    let finalTimer = null;
    let terminationReason = null;
    const clearTimers = () => {
      clearTimeout(termTimer);
      clearTimeout(killTimer);
      clearTimeout(finalTimer);
    };
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimers();
      signal?.removeEventListener("abort", onAbort);
      resolvePromise(result);
    };
    const terminate = (reason) => {
      if (terminationReason) return;
      terminationReason = reason;
      try { proc.kill("SIGTERM"); } catch {}
      killTimer = setTimeout(() => {
        try { proc.kill("SIGKILL"); } catch {}
      }, 2_000);
      finalTimer = setTimeout(() => {
        try { proc.stdin.destroy(); } catch {}
        try { proc.stdout.destroy(); } catch {}
        try { proc.stderr.destroy(); } catch {}
        finish({
          code: -2,
          signal: "SIGKILL",
          stdout,
          stderr: `${stderr}${reason}: child process did not exit after SIGKILL`,
        });
      }, 4_000);
    };
    const onAbort = () => terminate("aborted");
    termTimer = setTimeout(() => terminate("timeout"), timeoutMs);
    if (signal?.aborted) onAbort();
    else signal?.addEventListener("abort", onAbort, { once: true });
    proc.stdout.setEncoding("utf8");
    proc.stderr.setEncoding("utf8");
    proc.stdout.on("data", (chunk) => { stdout += chunk; });
    proc.stderr.on("data", (chunk) => { stderr += chunk; });
    proc.on("error", (error) => {
      finish({ code: -1, stdout, stderr: `${stderr}${error.message}` });
    });
    proc.on("exit", (code, signal) => {
      finish({ code: code ?? -1, signal, stdout, stderr, terminationReason });
    });
    if (input !== undefined) proc.stdin.end(input);
    else proc.stdin.end();
  });
}

function sshArgs(target, command) {
  return [...SSH_BASE, target, command];
}

async function sshRun(target, command, options = {}) {
  return runCaptured("ssh", sshArgs(target, command), options);
}

function waitForReady(proc, label, timeoutMs = 60_000) {
  return new Promise((resolvePromise, rejectPromise) => {
    let stdoutBuffer = "";
    let stderrTail = "";
    let settled = false;
    const finish = (fn, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      fn(value);
    };
    const inspect = (line) => {
      const trimmed = line.trim();
      if (!trimmed.startsWith("{")) return;
      try {
        const frame = JSON.parse(trimmed);
        if (frame.event === "ready" && Number.isInteger(frame.port) && frame.port > 0) {
          finish(resolvePromise, frame);
        }
      } catch {}
    };
    proc.stdout.setEncoding("utf8");
    proc.stderr.setEncoding("utf8");
    proc.stdout.on("data", (chunk) => {
      stdoutBuffer += chunk;
      for (;;) {
        const newline = stdoutBuffer.indexOf("\n");
        if (newline < 0) break;
        inspect(stdoutBuffer.slice(0, newline));
        stdoutBuffer = stdoutBuffer.slice(newline + 1);
      }
    });
    proc.stderr.on("data", (chunk) => {
      stderrTail = `${stderrTail}${chunk}`.slice(-4_096);
    });
    proc.on("exit", (code, signal) => {
      finish(rejectPromise, new Error(`${label} exited before ready (code=${code} signal=${signal})`));
    });
    proc.on("error", (error) => finish(rejectPromise, error));
    const timer = setTimeout(
      () => finish(rejectPromise, new Error(`${label} did not become ready; stderr was suppressed (${stderrTail.length} bytes captured)`)),
      timeoutMs,
    );
  });
}

async function freeLocalPort() {
  const server = createTcpServer();
  await new Promise((resolvePromise, rejectPromise) => {
    server.once("error", rejectPromise);
    server.listen(0, "127.0.0.1", resolvePromise);
  });
  const port = server.address().port;
  await new Promise((resolvePromise) => server.close(resolvePromise));
  return port;
}

class RpcClient {
  constructor(label) {
    this.label = label;
    this.nextId = 1;
    this.pending = new Map();
    this.ws = null;
    this.closed = false;
  }

  async connect(baseHttp, token, timeoutMs = 30_000) {
    const wsBase = baseHttp.replace(/^http/, "ws").replace(/\/$/, "");
    const url = `${wsBase}/ws?token=${encodeURIComponent(token)}`;
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      try {
        const ws = new WebSocket(url);
        await new Promise((resolvePromise, rejectPromise) => {
          ws.onopen = resolvePromise;
          ws.onerror = () => rejectPromise(new Error("websocket connection failed"));
        });
        this.ws = ws;
        ws.onmessage = (event) => {
          const frame = JSON.parse(String(event.data));
          if (frame.push) return;
          const pending = this.pending.get(frame.id);
          if (pending) {
            this.pending.delete(frame.id);
            pending.resolve(frame);
          }
        };
        ws.onclose = () => {
          for (const pending of this.pending.values()) {
            pending.reject(new Error(`${this.label} websocket closed`));
          }
          this.pending.clear();
        };
        return;
      } catch (error) {
        if (Date.now() >= deadline) throw error;
        await sleep(200);
      }
    }
  }

  callRaw(method, params = {}, timeoutMs = 60_000) {
    const id = this.nextId++;
    const timer = setTimeout(() => {
      const pending = this.pending.get(id);
      if (pending) {
        this.pending.delete(id);
        pending.reject(new Error(`${method} timed out after ${timeoutMs}ms`));
      }
    }, timeoutMs);
    const promise = new Promise((resolvePromise, rejectPromise) => {
      this.pending.set(id, {
        resolve: (frame) => { clearTimeout(timer); resolvePromise(frame); },
        reject: (error) => { clearTimeout(timer); rejectPromise(error); },
      });
    });
    this.ws.send(JSON.stringify({ id, method, params }));
    return promise;
  }

  async call(method, params = {}, timeoutMs = 60_000) {
    const frame = await this.callRaw(method, params, timeoutMs);
    if (frame.ok !== true) {
      const error = new Error(`${method} failed with ${frame.error?.code ?? "unknown"}`);
      error.code = frame.error?.code;
      throw error;
    }
    return frame.result;
  }

  close() {
    this.closed = true;
    try { this.ws?.close(); } catch {}
  }
}

async function sessionToken(baseHttp) {
  const response = await fetch(`${baseHttp.replace(/\/$/, "")}/api/session`, {
    headers: { Origin: baseHttp.replace(/\/$/, "") },
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok) throw new Error(`session endpoint returned HTTP ${response.status}`);
  const body = await response.json();
  if (typeof body.token !== "string" || body.token.length < 32) {
    throw new Error("session endpoint did not return an auth token");
  }
  return body.token;
}

async function startLocalPeer({ role, binary, loopback, runId, resources, secrets }) {
  const dataDir = mkdtempSync(join(tmpdir(), `jeliya-v050-${runId}-${role}-`));
  const args = ["--no-open", "--port", "0", "--data-dir", dataDir];
  if (loopback) args.unshift("--loopback");
  const proc = spawn(binary, args, { stdio: ["pipe", "pipe", "pipe"] });
  const peer = {
    role,
    kind: "local",
    proc,
    ready: null,
    dataDir,
    baseHttp: null,
    token: null,
    client: null,
    logs: attachLogCollectors(proc),
  };
  resources.peers.push(peer);
  try {
    const ready = await waitForReady(proc, `local peer ${role}`);
    peer.ready = ready;
    const baseHttp = `http://127.0.0.1:${ready.port}`;
    peer.baseHttp = baseHttp;
    const token = await sessionToken(baseHttp);
    secrets.add(token);
    peer.token = token;
    const client = new RpcClient(role.toUpperCase());
    peer.client = client;
    await client.connect(baseHttp, token);
    return peer;
  } catch (error) {
    throw error;
  }
}

async function tcpPortOpen(port) {
  return new Promise((resolvePromise) => {
    const socket = createConnection({ host: "127.0.0.1", port });
    const finish = (open) => {
      socket.destroy();
      resolvePromise(open);
    };
    socket.setTimeout(750, () => finish(false));
    socket.once("connect", () => finish(true));
    socket.once("error", () => finish(false));
  });
}

async function startSshTunnel(target, remotePort, resources, { daemonHealth = true } = {}) {
  const localPort = await freeLocalPort();
  const proc = spawn(
    "ssh",
    [
      ...SSH_BASE,
      "-o", "ExitOnForwardFailure=yes",
      "-N",
      "-L", `127.0.0.1:${localPort}:127.0.0.1:${remotePort}`,
      target,
    ],
    { stdio: ["ignore", "ignore", "pipe"] },
  );
  resources.tunnels.push(proc);
  await pollUntil(async () => {
    try {
      if (!daemonHealth) return await tcpPortOpen(localPort);
      const response = await fetch(`http://127.0.0.1:${localPort}/api/health`, {
        signal: AbortSignal.timeout(1_000),
      });
      return response.ok;
    } catch {
      if (proc.exitCode !== null) throw new Error(`SSH tunnel to ${target} exited early`);
      return false;
    }
  }, 20_000, `SSH tunnel to ${target}`);
  return { proc, localPort };
}

async function provisionRemoteBinary({
  target,
  role,
  runId,
  remoteRun,
  binary,
  expectedSha,
  expectedVersion,
  expectedRelayOnly,
  signal,
}) {
  const runDir = remoteRunDir(runId, role);
  if (remoteRun.runDir !== runDir || remoteRun.target !== target) {
    throw new Error("remote run ownership record does not match provisioning target");
  }
  const inventory = await sshRun(
    target,
    "set -eu; uname -s; uname -m; remote_uid=$(id -u); printf 'uid=%s\\n' \"$remote_uid\"; for tool in sha256sum awk od tr id; do command -v \"$tool\" >/dev/null || exit 97; done; printf 'tools-ok\\n'; if [ \"$remote_uid\" -eq 0 ]; then command -v setpriv >/dev/null; command -v chown >/dev/null; drop_uid=$(id -u nobody); drop_gid=$(id -g nobody); [ \"$drop_uid\" -ne 0 ]; [ \"$drop_gid\" -ne 0 ]; printf 'drop=%s:%s\\n' \"$drop_uid\" \"$drop_gid\"; else printf 'drop=none\\n'; fi; ( . /etc/os-release 2>/dev/null && printf '%s\\n' \"$PRETTY_NAME\" ) || printf 'Linux\\n'",
    { signal },
  );
  if (inventory.code !== 0) throw new Error(`read-only inventory failed for ${target}`);
  const lines = inventory.stdout.trim().split(/\r?\n/);
  const uidMatch = /^uid=(\d+)$/.exec(lines[2] ?? "");
  const dropMatch = /^drop=(\d+):(\d+)$/.exec(lines[4] ?? "");
  if (lines[0] !== "Linux" || lines[1] !== "x86_64" || !uidMatch || lines[3] !== "tools-ok") {
    throw new Error(`${target} is not a supported Linux x86_64 host with the required verification tools`);
  }
  const remoteUid = uidMatch[1];
  const privilegeDrop = remoteUid === "0"
    ? validatePrivilegeDrop(dropMatch ? { uid: dropMatch[1], gid: dropMatch[2] } : null)
    : null;
  if (remoteUid === "0" && !privilegeDrop) {
    throw new Error(`${target} is root-capable but cannot drop the daemon to an unprivileged account`);
  }

  const cleanup = async () => {
    const command = remoteOwnedDirectoryCleanupCommand(runId, runDir);
    await sshRun(target, command, { timeoutMs: 20_000 });
  };
  let createdRunDir = false;
  try {
    const create = await sshRun(target, remoteCreateCommand(runId, runDir), { signal });
    createdRunDir = create.stdout.trim() === "RUN_DIR_CREATED";
    remoteRun.created = createdRunDir;
    if (create.code !== 0 || !createdRunDir) {
      throw new Error(`could not create isolated remote run directory on ${target}`);
    }

    const copy = await runCaptured(
      "scp",
      ["-q", ...SSH_BASE, binary, `${target}:${runDir}/jeliyad`],
      { timeoutMs: 120_000, signal },
    );
    if (copy.code !== 0) throw new Error(`could not copy the verified binary to ${target}`);

    if (privilegeDrop) {
      const ownership = await sshRun(
        target,
        `chown -R '${privilegeDrop.uid}:${privilegeDrop.gid}' -- '${runDir}'`,
        { signal },
      );
      if (ownership.code !== 0) {
        throw new Error(`could not confine the root-capable host run to an unprivileged uid on ${target}`);
      }
    }

    const verify = await sshRun(
      target,
      remoteBinaryVerificationCommand(runId, runDir, expectedSha, privilegeDrop),
      { timeoutMs: 30_000, signal },
    );
    if (verify.code !== 0) throw new Error(`remote binary verification failed on ${target}`);
    const binaryValidation = parseRemoteBinaryVerification(verify.stdout, {
      expectedSha,
      expectedVersion,
      expectedExecutionUid: privilegeDrop?.uid ?? remoteUid,
      expectedRelayOnly,
    });
    return {
      runDir,
      os: lines[5] || "Linux",
      architecture: "x86_64",
      binaryValidation,
      privilegeDrop,
      processPrivilege: privilegeDrop ? "dropped-to-unprivileged-system-uid" : "ssh-account-uid",
    };
  } catch (error) {
    // Never delete a colliding/pre-existing path. Cleanup is permitted only
    // after the remote mkdir command proved this run created the directory.
    if (createdRunDir) {
      try { await cleanup(); } catch {}
      remoteRun.created = false;
    }
    throw error;
  }
}

async function startRemotePeer({
  target,
  role,
  runId,
  binary,
  expectedSha,
  expectedVersion,
  expectedRelayOnly,
  resources,
  secrets,
}) {
  const remoteRun = {
    target,
    runDir: remoteRunDir(runId, role),
    created: false,
    daemonStarted: false,
  };
  // Register the predetermined, nonce-bound path before any remote mutation.
  // Cleanup still requires the exact owner marker, so a collision is preserved.
  resources.remoteRuns.push(remoteRun);
  const provisioned = await provisionRemoteBinary({
    target,
    role,
    runId,
    remoteRun,
    binary,
    expectedSha,
    expectedVersion,
    expectedRelayOnly,
    signal: resources.abortSignal,
  });
  const command = remoteDaemonCommand(runId, provisioned.runDir, provisioned.privilegeDrop);
  const proc = spawn("ssh", [...SSH_BASE, "-T", target, command], {
    stdio: ["pipe", "pipe", "pipe"],
  });
  const peer = {
    role,
    kind: "remote",
    target,
    proc,
    ready: null,
    runDir: provisioned.runDir,
    baseHttp: null,
    token: null,
    client: null,
    logs: attachLogCollectors(proc),
    os: provisioned.os,
    architecture: provisioned.architecture,
    binaryValidation: provisioned.binaryValidation,
    processPrivilege: provisioned.processPrivilege,
  };
  // Register before waiting for readiness, the tunnel, the session token, or
  // the WebSocket. Any failure after spawn must still be visible to cleanup.
  resources.peers.push(peer);
  remoteRun.daemonStarted = true;
  const ready = await waitForReady(proc, `remote peer ${role}`);
  peer.ready = ready;
  const tunnel = await startSshTunnel(target, ready.port, resources);
  const baseHttp = `http://127.0.0.1:${tunnel.localPort}`;
  peer.baseHttp = baseHttp;
  const token = await sessionToken(baseHttp);
  secrets.add(token);
  peer.token = token;
  const client = new RpcClient(role.toUpperCase());
  peer.client = client;
  await client.connect(baseHttp, token);
  return peer;
}

async function ensureIdentity(peer) {
  const status = await peer.client.call("daemon.status");
  return status.identity ?? peer.client.call("identity.create");
}

async function waitTimeline(peer, roomId, predicate, what, timeoutMs = WAIT_MS) {
  return pollUntil(async () => {
    const { events } = await peer.client.call("room.timeline", { room_id: roomId });
    return events.find(predicate) ?? null;
  }, timeoutMs, what, 500);
}

async function waitPath(peer, roomId, expected, expectedIdentityIds) {
  let consecutive = 0;
  return pollUntil(async () => {
    const { peers } = await peer.client.call("peers.status", { room_id: roomId });
    if (!pathMatchesExpectedIdentities(peers, expected, expectedIdentityIds)) {
      consecutive = 0;
      return null;
    }
    consecutive += 1;
    if (consecutive < 3) return null;
    return {
      expected_identities: expectedIdentityIds.length,
      consecutive_observations: consecutive,
      expected_path: expected,
    };
  }, WAIT_MS, `${peer.role} to report ${expected} path`, 1_000);
}

async function downloadLocalFile(peer, roomId, fileId) {
  const url = new URL("/api/files/local", peer.baseHttp);
  url.searchParams.set("room_id", roomId);
  url.searchParams.set("file_id", fileId);
  const response = await fetch(url, {
    headers: { Authorization: `Bearer ${peer.token}` },
    signal: AbortSignal.timeout(30_000),
  });
  if (!response.ok) throw new Error(`local file endpoint returned HTTP ${response.status}`);
  return Buffer.from(await response.arrayBuffer());
}

async function expectRoomUnknown(client, method, params) {
  const frame = await client.callRaw(method, params);
  if (frame.ok !== false || frame.error?.code !== "room_unknown") {
    throw new Error(`${method} disclosed a foreign room or returned ${frame.error?.code ?? "success"}`);
  }
}

async function foreignLocalFileIsDenied(peer, roomId, fileId) {
  const url = new URL("/api/files/local", peer.baseHttp);
  url.searchParams.set("room_id", roomId);
  url.searchParams.set("file_id", fileId);
  const response = await fetch(url, {
    headers: { Authorization: `Bearer ${peer.token}` },
    signal: AbortSignal.timeout(10_000),
  });
  const body = await response.json().catch(() => null);
  return !response.ok && body?.error?.code === "room_unknown";
}

async function pipeHttpThroughPeer(peer, localAddr, resources) {
  if (peer.kind === "local") return `http://${localAddr}/`;
  const match = /^127\.0\.0\.1:(\d+)$/.exec(localAddr);
  if (!match) throw new Error(`remote pipe returned unsafe local address: ${localAddr}`);
  const tunnel = await startSshTunnel(peer.target, Number(match[1]), resources, {
    daemonHealth: false,
  });
  return `http://127.0.0.1:${tunnel.localPort}/`;
}

async function waitProcessExit(proc, timeoutMs = 5_000) {
  if (proc.exitCode !== null || proc.signalCode !== null) return true;
  return new Promise((resolvePromise) => {
    const onExit = () => {
      clearTimeout(timer);
      resolvePromise(true);
    };
    const timer = setTimeout(() => {
      proc.off("exit", onExit);
      resolvePromise(false);
    }, timeoutMs);
    proc.once("exit", onExit);
  });
}

async function terminateChildProcess(proc) {
  if (proc.exitCode !== null || proc.signalCode !== null) return true;
  try { proc.kill("SIGTERM"); } catch {}
  if (await waitProcessExit(proc)) return true;
  try { proc.kill("SIGKILL"); } catch {}
  return waitProcessExit(proc, 2_000);
}

async function runFlow({ peers, expectedPath, runId, resources, record }) {
  const [a, b, c] = peers;
  const identities = [];
  for (const peer of peers) {
    identities.push(await record(`${peer.role}: identity ready`, () => ensureIdentity(peer)));
  }

  const room = await record("A: room created", () => a.client.call("room.create", {
    name: `v0.5.0 network evidence ${runId}`,
  }));
  const roomId = room.room_id;
  const openedA = await record("A: room opened", async () => {
    const first = await a.client.call("room.open", { room_id: roomId });
    if (expectedPath === "relay") return first;
    return pollUntil(async () => {
      const opened = await a.client.call("room.open", { room_id: roomId });
      return typeof opened.endpoint?.addr === "string" && opened.endpoint.addr.includes("@")
        ? opened
        : null;
    }, 30_000, "A dialable direct address", 500);
  });
  const aDialHints = typeof openedA.endpoint?.addr === "string" ? [openedA.endpoint.addr] : [];

  async function joinPeer(peer, identity) {
    const invite = await a.client.call("invite.create", {
      room_id: roomId,
      identity_id: identity.identity_id,
      role: "member",
    });
    resources.secrets.add(invite.ticket);
    await record(`${peer.role}: targeted room join`, () => peer.client.call("room.join", {
      ticket: invite.ticket,
      peers: aDialHints,
    }, WAIT_MS));
    await record(`${peer.role}: joined room opened`, () => peer.client.call("room.open", {
      room_id: roomId,
      peers: aDialHints,
    }));
    await record(`A: observes ${peer.role} membership`, () => waitTimeline(
      a,
      roomId,
      (event) => event.kind === "member_joined" && event.member?.identity_id === identity.identity_id,
      `${peer.role} membership on A`,
    ));
  }

  await joinPeer(b, identities[1]);
  if (c) await joinPeer(c, identities[2]);

  const pathEvidence = {};
  pathEvidence.a = await record(`A: ${expectedPath} path settled`, () => waitPath(
    a,
    roomId,
    expectedPath,
    identities.slice(1).map((identity) => identity.identity_id),
  ));
  pathEvidence.b = await record(`B: ${expectedPath} path settled`, () => waitPath(
    b,
    roomId,
    expectedPath,
    [identities[0].identity_id],
  ));
  if (c) {
    pathEvidence.c = await record(`C: ${expectedPath} path settled`, () => waitPath(
      c,
      roomId,
      expectedPath,
      [identities[0].identity_id],
    ));
  }

  const aBody = `network-a-${runId}`;
  const bBody = `network-b-${runId}`;
  await record("A to B message authored", () => a.client.call("message.send", { room_id: roomId, body: aBody }));
  await record("B receives A message", () => waitTimeline(b, roomId, (event) => event.kind === "message" && event.body === aBody, "A message on B"));
  await record("B to A message authored", () => b.client.call("message.send", { room_id: roomId, body: bBody }));
  await record("A receives B message", () => waitTimeline(a, roomId, (event) => event.kind === "message" && event.body === bBody, "B message on A"));
  if (c) {
    await record("C receives both messages", async () => {
      await waitTimeline(c, roomId, (event) => event.kind === "message" && event.body === aBody, "A message on C");
      return waitTimeline(c, roomId, (event) => event.kind === "message" && event.body === bBody, "B message on C");
    });
    const cBody = `network-c-${runId}`;
    await record("C message converges to A and B", async () => {
      await c.client.call("message.send", { room_id: roomId, body: cBody });
      await waitTimeline(a, roomId, (event) => event.kind === "message" && event.body === cBody, "C message on A");
      return waitTimeline(b, roomId, (event) => event.kind === "message" && event.body === cBody, "C message on B");
    });
  }

  const payload = Buffer.concat([
    Buffer.from(`jeliya-v0.5.0-${runId}\n`),
    randomBytes(256 * 1024),
  ]);
  const payloadPath = join(a.dataDir, `payload-${runId}.bin`);
  writeFileSync(payloadPath, payload, { mode: 0o600 });
  const shared = await record("A shares candidate payload", () => a.client.call("file.share", {
    room_id: roomId,
    path: payloadPath,
    name: `payload-${runId}.bin`,
    mime: "application/octet-stream",
  }, WAIT_MS));
  await record("B lists candidate payload as available", () => pollUntil(async () => {
    const { files } = await b.client.call("file.list", { room_id: roomId });
    const row = files.find((file) => file.file_id === shared.file_id);
    return row?.available ? row : null;
  }, WAIT_MS, "B file availability", 500));
  const fetched = await record("B fetches and BLAKE3-verifies payload", () => b.client.call(
    "file.fetch",
    { room_id: roomId, file_id: shared.file_id },
    600_000,
  ));
  if (fetched.verified !== true || fetched.bytes !== payload.length) {
    throw new Error("file.fetch did not report verified bytes");
  }
  const expectedPayloadSha256 = createHash("sha256").update(payload).digest("hex");
  const fileEvidence = await record("B fetched bytes are byte-identical", async () => {
    const downloaded = await downloadLocalFile(b, roomId, shared.file_id);
    if (!downloaded.equals(payload)) throw new Error("downloaded bytes differ from shared bytes");
    const actualSha256 = createHash("sha256").update(downloaded).digest("hex");
    if (actualSha256 !== expectedPayloadSha256) throw new Error("downloaded SHA-256 differs from source");
    return {
      bytes_expected: payload.length,
      bytes_actual: downloaded.length,
      engine_verified: fetched.verified,
      expected_sha256: expectedPayloadSha256,
      actual_sha256: actualSha256,
      sha256_equal: true,
    };
  });

  const pipeBody = `pipe-${runId}`;
  let targetConnections = 0;
  let targetRequests = 0;
  const httpServer = createHttpServer((_request, response) => {
    targetRequests += 1;
    response.writeHead(200, { "content-type": "text/plain" });
    response.end(pipeBody);
  });
  httpServer.on("connection", () => { targetConnections += 1; });
  await new Promise((resolvePromise, rejectPromise) => {
    httpServer.once("error", rejectPromise);
    httpServer.listen(0, "127.0.0.1", resolvePromise);
  });
  resources.servers.push(httpServer);
  const targetPort = httpServer.address().port;
  const exposed = await record("A exposes one-peer pipe", () => a.client.call("pipe.expose", {
    room_id: roomId,
    target: `127.0.0.1:${targetPort}`,
    peer_identity: identities[1].identity_id,
  }));
  let unauthorizedPipeEvidence = null;
  if (c) {
    unauthorizedPipeEvidence = await record("C pipe gate forwards zero bytes to the target", async () => {
      // The upstream API creates the local forwarder before the owner evaluates
      // the per-stream allowlist. Therefore RPC success is expected here; the
      // security assertion is that the owner rejects the first stream and never
      // opens the protected loopback target.
      const connected = await c.client.call("pipe.connect", {
        room_id: roomId,
        pipe_id: exposed.pipe_id,
      }, WAIT_MS);
      const url = await pipeHttpThroughPeer(c, connected.local_addr, resources);
      let responseReceived = false;
      try {
        const response = await fetch(url, { signal: AbortSignal.timeout(10_000) });
        responseReceived = true;
        await response.arrayBuffer();
      } catch {
        // An EOF/reset is the expected connector-side result of owner rejection.
      }
      await sleep(250);
      if (responseReceived || targetConnections !== 0 || targetRequests !== 0) {
        throw new Error("unauthorized pipe stream reached the protected loopback target");
      }
      return {
        local_forwarder_created: true,
        response_received: false,
        target_connections: 0,
        target_requests: 0,
      };
    });
  }
  const pipeEvidence = await record("B observes and connects authorized pipe", async () => {
    await pollUntil(async () => {
      const { pipes } = await b.client.call("pipe.list", { room_id: roomId });
      return pipes.find((pipe) => pipe.pipe_id === exposed.pipe_id && pipe.state === "open") ?? null;
    }, WAIT_MS, "open pipe on B", 500);
    const connected = await b.client.call("pipe.connect", { room_id: roomId, pipe_id: exposed.pipe_id }, WAIT_MS);
    const url = await pipeHttpThroughPeer(b, connected.local_addr, resources);
    const response = await fetch(url, { signal: AbortSignal.timeout(30_000) });
    const body = await response.text();
    if (response.status !== 200 || body !== pipeBody) throw new Error("pipe response mismatch");
    if (targetConnections < 1 || targetRequests < 1) {
      throw new Error("authorized pipe did not reach the protected loopback target");
    }
    return {
      http_status: response.status,
      bytes: Buffer.byteLength(body),
      target_connections: targetConnections,
      target_requests: targetRequests,
    };
  });
  await record("A closes pipe and B observes closure", async () => {
    await a.client.call("pipe.close", { room_id: roomId, pipe_id: exposed.pipe_id });
    return pollUntil(async () => {
      const { pipes } = await b.client.call("pipe.list", { room_id: roomId });
      return pipes.find((pipe) => pipe.pipe_id === exposed.pipe_id && pipe.state === "closed") ?? null;
    }, WAIT_MS, "closed pipe on B", 500);
  });

  await record("B closes live room session", () => b.client.call("room.close", { room_id: roomId }));
  const offlineBody = `offline-resync-${runId}`;
  await record("A authors message while B session is closed", () => a.client.call("message.send", {
    room_id: roomId,
    body: offlineBody,
  }));
  await record("B reopens and receives the offline message", async () => {
    await b.client.call("room.open", { room_id: roomId, peers: aDialHints });
    return waitTimeline(b, roomId, (event) => event.kind === "message" && event.body === offlineBody, "offline message after reopen");
  });
  const reconnectPath = await record(`B reconnects over ${expectedPath}`, () => waitPath(
    b,
    roomId,
    expectedPath,
    [identities[0].identity_id],
  ));

  const foreign = await record("A creates isolated foreign room", () => a.client.call("room.create", {
    name: `foreign-${runId}`,
  }));
  const foreignOpened = await a.client.call("room.open", { room_id: foreign.room_id });
  await a.client.call("message.send", { room_id: foreign.room_id, body: `foreign-secret-${runId}` });
  const foreignPayloadPath = join(a.dataDir, `foreign-${runId}.bin`);
  writeFileSync(foreignPayloadPath, randomBytes(512), { mode: 0o600 });
  const foreignFile = await a.client.call("file.share", {
    room_id: foreign.room_id,
    path: foreignPayloadPath,
    name: `foreign-${runId}.bin`,
  });
  let foreignPipeId = "00".repeat(16);
  const foreignAgentIdentity = c ? identities[2].identity_id : identities[0].identity_id;
  const foreignAgentLabel = `foreign-agent-${runId}`;
  const foreignAgentMessage = `foreign-agent-message-${runId}`;
  if (c) {
    const foreignInvite = await a.client.call("invite.create", {
      room_id: foreign.room_id,
      identity_id: foreignAgentIdentity,
      role: "agent",
    });
    resources.secrets.add(foreignInvite.ticket);
    const foreignHints = typeof foreignOpened.endpoint?.addr === "string"
      ? [foreignOpened.endpoint.addr]
      : [];
    await c.client.call("room.join", { ticket: foreignInvite.ticket, peers: foreignHints }, WAIT_MS);
    await c.client.call("room.open", { room_id: foreign.room_id, peers: foreignHints });
    await c.client.call("status.post", {
      room_id: foreign.room_id,
      label: foreignAgentLabel,
      message: foreignAgentMessage,
      progress: 37,
    });
    const foreignPipe = await a.client.call("pipe.expose", {
      room_id: foreign.room_id,
      target: "127.0.0.1:9",
      peer_identity: foreignAgentIdentity,
    });
    foreignPipeId = foreignPipe.pipe_id;
  }
  const foreignScopedMethods = [
    ["room.open", { room_id: foreign.room_id }],
    ["room.close", { room_id: foreign.room_id }],
    ["room.leave", { room_id: foreign.room_id }],
    ["room.timeline", { room_id: foreign.room_id }],
    ["room.members", { room_id: foreign.room_id }],
    ["invite.create", {
      room_id: foreign.room_id,
      identity_id: foreignAgentIdentity,
      role: "member",
    }],
    ["message.send", { room_id: foreign.room_id, body: "must-be-denied" }],
    ["status.post", { room_id: foreign.room_id, label: "must-be-denied" }],
    ["file.share", { room_id: foreign.room_id, path: "/nonexistent-jeliya-foreign-file" }],
    ["file.list", { room_id: foreign.room_id }],
    ["file.fetch", { room_id: foreign.room_id, file_id: foreignFile.file_id }],
    ["pipe.expose", {
      room_id: foreign.room_id,
      target: "127.0.0.1:9",
      peer_identity: foreignAgentIdentity,
    }],
    ["pipe.list", { room_id: foreign.room_id }],
    ["pipe.connect", { room_id: foreign.room_id, pipe_id: foreignPipeId }],
    ["pipe.close", { room_id: foreign.room_id, pipe_id: foreignPipeId }],
    ["peers.status", { room_id: foreign.room_id }],
    ["agent.history", { room_id: foreign.room_id, identity_id: foreignAgentIdentity }],
  ];
  await record("B public room-scoped RPCs do not disclose a foreign room ID", async () => {
    for (const [method, params] of foreignScopedMethods) {
      await expectRoomUnknown(b.client, method, params);
    }
    return { denied_methods: foreignScopedMethods.map(([method]) => method) };
  });
  await record("B local-file HTTP endpoint does not disclose a foreign room ID", async () => {
    if (!(await foreignLocalFileIsDenied(b, foreign.room_id, foreignFile.file_id))) {
      throw new Error("foreign local-file endpoint did not return room_unknown");
    }
    return true;
  });
  await record("B aggregate reads omit the foreign room and agent projection", async () => {
    const listed = await b.client.call("room.list");
    const fleet = await b.client.call("agents.fleet");
    const encoded = JSON.stringify({ listed, fleet });
    const forbidden = [
      foreign.room_id,
      `foreign-${runId}`,
      foreignAgentLabel,
      foreignAgentMessage,
      ...(c ? [foreignAgentIdentity] : []),
    ];
    if (forbidden.some((sentinel) => encoded.includes(sentinel))) {
      throw new Error("aggregate read disclosed the foreign room or agent projection");
    }
    return { agent_projection_exercised: Boolean(c) };
  });

  return {
    path_evidence: pathEvidence,
    functional_evidence: {
      file: fileEvidence,
      pipe: {
        ...pipeEvidence,
        unauthorized_third_peer: unauthorizedPipeEvidence,
      },
      reconnect: {
        session_closed: true,
        message_authored_while_closed: true,
        offline_message_resynchronized: true,
        settled_path: reconnectPath,
      },
      foreign_room_non_disclosure: {
        rpc_methods_denied: foreignScopedMethods.map(([method]) => method),
        local_file_http_denied: true,
        aggregate_reads_filtered: true,
        foreign_agent_projection_exercised: Boolean(c),
        synchronization_isolation_claimed: false,
      },
      multi_peer: c ? { peers: 3, convergence_verified: true } : { peers: 2, convergence_verified: true },
    },
  };
}

function createRunLifecycle(resources) {
  const controller = new AbortController();
  let signalName = null;
  let cleanupPromise = null;
  const cleanup = () => {
    cleanupPromise ??= stopResources(resources);
    return cleanupPromise;
  };
  const handlers = new Map();
  for (const name of ["SIGINT", "SIGTERM"]) {
    const handler = () => {
      if (signalName) return;
      signalName = name;
      controller.abort(new Error(`network verification interrupted by ${name}`));
      void cleanup();
    };
    handlers.set(name, handler);
    process.on(name, handler);
  }
  return {
    abortSignal: controller.signal,
    cleanup,
    get signalName() { return signalName; },
    dispose() {
      for (const [name, handler] of handlers) process.off(name, handler);
    },
  };
}

async function closeRunOwnedServer(server) {
  let closed = false;
  const closePromise = new Promise((resolvePromise) => {
    server.close(() => {
      closed = true;
      resolvePromise(true);
    });
  });
  const first = await Promise.race([
    closePromise,
    sleep(2_000).then(() => false),
  ]);
  if (first) return true;
  try { server.closeAllConnections?.(); } catch {}
  const second = await Promise.race([
    closePromise,
    sleep(2_000).then(() => false),
  ]);
  return Boolean(second || closed);
}

async function stopResources(resources) {
  const failures = [];
  for (const peer of resources.peers) {
    if (peer.client) {
      try { await peer.client.call("daemon.shutdown", {}, 10_000); } catch { failures.push(`${peer.role}:shutdown`); }
      peer.client.close();
    }
  }
  for (const server of resources.servers) {
    try {
      if (!(await closeRunOwnedServer(server))) failures.push("http-server:close-timeout");
    } catch {
      failures.push("http-server:close");
    }
  }
  for (const tunnel of resources.tunnels.reverse()) {
    if (!(await terminateChildProcess(tunnel))) failures.push("ssh-tunnel:process-remains");
  }
  for (const peer of resources.peers) {
    try { peer.proc.stdin.end(); } catch {}
    if (!(await terminateChildProcess(peer.proc))) failures.push(`${peer.role}:process-remains`);
  }
  for (const peer of resources.peers) {
    if (peer.kind === "local") {
      if (peer.dataDir.includes(`jeliya-v050-${resources.runId}-`)) {
        try { rmSync(peer.dataDir, { recursive: true, force: true }); } catch { failures.push(`${peer.role}:local-artifact-cleanup`); }
        if (existsSync(peer.dataDir)) failures.push(`${peer.role}:local-artifact-remains`);
      }
    }
  }
  const seenRemoteDirs = new Set();
  for (const remote of resources.remoteRuns) {
    const key = `${remote.target}\0${remote.runDir}`;
    if (seenRemoteDirs.has(key)) continue;
    seenRemoteDirs.add(key);
    const role = remote.runDir.at(-1);
    try {
      // This independently checks the remote daemon PID and executable. The
      // local SSH process exiting is never accepted as proof that jeliyad
      // stopped, and the run directory is deleted only after /proc confirms it.
      const cleanup = remote.daemonStarted
        ? remoteCleanupCommand(resources.runId, remote.runDir)
        : remoteOwnedDirectoryCleanupCommand(resources.runId, remote.runDir);
      const result = await sshRun(remote.target, cleanup, { timeoutMs: 20_000 });
      if (result.code !== 0) {
        if (remote.daemonStarted && result.code !== 95) {
          failures.push(`${role}:remote-process-unconfirmed:${result.code}`);
        }
        failures.push(`${role}:remote-artifact-cleanup:${result.code}`);
      }
    } catch {
      if (remote.daemonStarted) failures.push(`${role}:remote-process-unconfirmed`);
      failures.push(`${role}:remote-artifact-cleanup`);
    }
  }
  return {
    completed: failures.length === 0,
    processes_stopped: !failures.some((failure) => failure.includes("process") || failure.includes("shutdown")),
    temporary_artifacts_removed: !failures.some((failure) => failure.includes("artifact")),
    failure_codes: failures,
  };
}

function boundedSignal(signal, timeoutMs) {
  const timeout = AbortSignal.timeout(timeoutMs);
  return signal ? AbortSignal.any([signal, timeout]) : timeout;
}

async function publicAddressLocal(signal) {
  try {
    const response = await fetch("https://api.ipify.org", {
      signal: boundedSignal(signal, 8_000),
    });
    return response.ok ? (await response.text()).trim() : "";
  } catch {
    return "";
  }
}

async function publicAddressRemote(target, signal) {
  const result = await sshRun(
    target,
    "curl -fsS --max-time 8 https://api.ipify.org || true",
    { signal },
  );
  return result.stdout.trim();
}

export function parseRipeAsn(payload) {
  const values = payload?.data?.asns;
  if (!Array.isArray(values) || values.length !== 1) return null;
  const raw = String(values[0]);
  return /^\d{1,10}$/.test(raw) ? `AS${raw}` : null;
}

export function certificationEligible({
  localDryrun,
  buildFromSource,
  sourceReleaseable,
  expectedPath,
  topologyProven,
}) {
  return !localDryrun
    && buildFromSource
    && sourceReleaseable
    && expectedPath !== "any"
    && topologyProven;
}

async function publicAsn(address, signal) {
  try {
    const response = await fetch(
      `https://stat.ripe.net/data/network-info/data.json?resource=${encodeURIComponent(address)}`,
      { signal: boundedSignal(signal, 8_000) },
    );
    if (!response.ok) return null;
    return parseRipeAsn(await response.json());
  } catch {
    return null;
  }
}

export function parseCli(argv) {
  const args = parseArgs(argv);
  const localDryrun = Boolean(args["local-dryrun"]);
  const buildFromSource = Boolean(args["build-from-source"]);
  const expectedPath = String(args["expect-path"] ?? (localDryrun ? "direct" : "direct"));
  if (!new Set(["direct", "relay", "any"]).has(expectedPath)) {
    die("--expect-path must be direct, relay, or any");
  }
  const relayOnlyBuild = Boolean(args["relay-only-build"]);
  if ((expectedPath === "relay") !== relayOnlyBuild) {
    die("a certifying relay run requires both --expect-path relay and --relay-only-build");
  }
  if (!localDryrun && !validSshTarget(args.remote)) {
    die("remote mode requires a safe --remote user@host target");
  }
  if (!localDryrun && !validSshTarget(args["third-remote"])) {
    die("certifying remote mode requires a second safe --third-remote user@host target");
  }
  if (!localDryrun && String(args.remote) === String(args["third-remote"])) {
    die("--remote and --third-remote must be distinct SSH targets");
  }
  if (buildFromSource && (args["local-bin"] || args["linux-bin"] || args["linux-sha256"])) {
    die("--build-from-source cannot be combined with prebuilt binary arguments");
  }
  if (buildFromSource && !args["zig-sha256"]) {
    die("--build-from-source requires an independently supplied --zig-sha256");
  }
  return {
    args,
    localDryrun,
    expectedPath,
    relayOnlyBuild,
    buildFromSource,
    remote: args.remote ? String(args.remote) : null,
    thirdRemote: args["third-remote"] ? String(args["third-remote"]) : null,
    withThird: localDryrun ? Boolean(args["with-third"]) : true,
    allowDirty: Boolean(args["allow-dirty"] || localDryrun),
    allowSharedEgress: Boolean(args["allow-shared-egress"] || args["allow-same-network"] || localDryrun),
    expectedVersion: String(args["expected-version"] ?? "0.5.0"),
    localBin: String(args["local-bin"] ?? (localDryrun ? DEFAULT_DEBUG_BIN : DEFAULT_LOCAL_BIN)),
    linuxBin: args["linux-bin"] ? String(args["linux-bin"]) : null,
    linuxSha256: args["linux-sha256"] ? String(args["linux-sha256"]) : null,
    zigSha256: args["zig-sha256"] ? String(args["zig-sha256"]) : null,
  };
}

async function main(argv = process.argv.slice(2)) {
  const config = parseCli(argv);
  const now = new Date();
  const runId = `${now.toISOString().replace(/[-:]/g, "").replace(/\.\d{3}Z$/, "Z")}-${randomBytes(4).toString("hex")}`;
  const source = sourceIdentity();
  if (source.dirty && !config.allowDirty) {
    die("the working tree is dirty; commit the exact candidate or use --allow-dirty for a non-certifying machinery run");
  }
  if (!config.localDryrun && !config.buildFromSource && (!config.linuxBin || !config.linuxSha256)) {
    die("remote prebuilt mode requires --linux-bin and an independently supplied --linux-sha256");
  }

  let sourceBuild = null;
  if (config.buildFromSource) {
    sourceBuild = buildCandidateFromSource({
      runId,
      relayOnlyBuild: config.relayOnlyBuild,
      zigSha256: config.zigSha256,
      sourceCommit: source.commit,
    });
  }
  let localBinary;
  let linuxBinary = null;
  let expectedLinuxSha = null;
  try {
    localBinary = verifyLocalBinary(
      sourceBuild?.localPath ?? config.localBin,
      config.expectedVersion,
      config.relayOnlyBuild,
    );
    if (!config.localDryrun) {
      expectedLinuxSha = sourceBuild?.linuxSha256 ?? parseExpectedSha256(config.linuxSha256);
      // The Linux musl artifact is intentionally not executed on the local
      // operator platform. Its version and relay-only attestation are checked
      // independently after transfer on every Linux execution host.
      linuxBinary = verifyBinaryFile(sourceBuild?.linuxPath ?? config.linuxBin);
      if (linuxBinary.sha256 !== expectedLinuxSha) {
        die("the Linux binary does not match its independently established SHA-256");
      }
    }
  } catch (error) {
    if (sourceBuild) {
      try { removeBuildDirectory(sourceBuild.targetDir, runId); } catch {}
    }
    throw error;
  }

  const certificationInputs = {
    localDryrun: config.localDryrun,
    buildFromSource: config.buildFromSource,
    sourceReleaseable: source.releaseable,
    expectedPath: config.expectedPath,
  };
  const resources = {
    runId,
    peers: [],
    remoteRuns: [],
    tunnels: [],
    servers: [],
    secrets: new Set(),
    abortSignal: null,
  };
  const lifecycle = createRunLifecycle(resources);
  resources.abortSignal = lifecycle.abortSignal;
  const assertions = [];
  const record = async (name, fn) => {
    const started = Date.now();
    try {
      if (resources.abortSignal.aborted) {
        throw resources.abortSignal.reason ?? new Error("network verification interrupted");
      }
      const value = await fn();
      if (resources.abortSignal.aborted) {
        throw resources.abortSignal.reason ?? new Error("network verification interrupted");
      }
      assertions.push({ name, result: "pass", duration_ms: Date.now() - started });
      console.log(`network-evidence: ok — ${name}`);
      return value;
    } catch (error) {
      assertions.push({ name, result: "fail", duration_ms: Date.now() - started, error_code: error.code ?? "assertion_failed" });
      throw error;
    }
  };

  const evidence = {
    schema: 1,
    run_id: runId,
    started_at_utc: now.toISOString(),
    ended_at_utc: null,
    result: "running",
    // A remote run becomes certifiable only after the sanitized topology gate
    // proves distinct egress across at least two BGP origin ASNs.
    certifiable: false,
    mode: config.localDryrun ? "local-loopback-machinery" : config.relayOnlyBuild ? "remote-relay-only-build" : "remote-real-network",
    expected_path: config.expectedPath,
    source,
    build: sourceBuild?.evidence ?? {
      mode: "external-prebuilt",
      source_bound: false,
      locked: null,
      toolchain: null,
    },
    binaries: {
      local: {
        filename: localBinary.filename,
        sha256: localBinary.sha256,
        version: localBinary.version,
        relay_only_attested: localBinary.relay_only_attested,
      },
      remote: linuxBinary ? {
        filename: linuxBinary.filename,
        sha256: linuxBinary.sha256,
        expected_version: config.expectedVersion,
        expected_relay_only: config.relayOnlyBuild,
        execution_validation: "verified independently on every Linux execution host after transfer",
      } : null,
    },
    hosts: [],
    distinct_public_egress: config.localDryrun ? {
      evaluated: false,
      reason: "all roles use local loopback",
      independent_network_topology_proven: false,
    } : null,
    assertions,
    cleanup: { completed: false, processes_stopped: false, temporary_artifacts_removed: false, failure_codes: [] },
  };

  let flowResult = null;
  let failure = null;
  try {
    const a = await startLocalPeer({
      role: "a", binary: localBinary.path, loopback: config.localDryrun, runId, resources, secrets: resources.secrets,
    });
    evidence.hosts.push({ role: "a", host: "operator-local", os: process.platform, architecture: process.arch });
    let b;
    if (config.localDryrun) {
      b = await startLocalPeer({
        role: "b", binary: localBinary.path, loopback: true, runId, resources, secrets: resources.secrets,
      });
      evidence.hosts.push({ role: "b", host: "operator-local", os: process.platform, architecture: process.arch });
    } else {
      const localAddress = await publicAddressLocal(resources.abortSignal);
      const remoteAddress = await publicAddressRemote(config.remote, resources.abortSignal);
      const thirdAddress = await publicAddressRemote(config.thirdRemote, resources.abortSignal);
      if ([localAddress, remoteAddress, thirdAddress].some((address) => isIP(address) === 0)) {
        throw new Error("could not validate all three public egress observations as IP addresses");
      }
      resources.secrets.add(localAddress);
      resources.secrets.add(remoteAddress);
      resources.secrets.add(thirdAddress);
      const [operatorAsn, remoteAsn, thirdAsn] = await Promise.all([
        publicAsn(localAddress, resources.abortSignal),
        publicAsn(remoteAddress, resources.abortSignal),
        publicAsn(thirdAddress, resources.abortSignal),
      ]);
      const pairwiseEgress = {
        operator_to_b: classifyNetworks(localAddress, remoteAddress),
        operator_to_c: classifyNetworks(localAddress, thirdAddress),
        b_to_c: classifyNetworks(remoteAddress, thirdAddress),
      };
      const allDifferent = Object.values(pairwiseEgress).every(
        (entry) => entry.status === "different",
      );
      const asns = [operatorAsn, remoteAsn, thirdAsn];
      const routingDomainsProven = asns.every(Boolean) && new Set(asns).size >= 2;
      const topologyProven = allDifferent && routingDomainsProven;
      evidence.certifiable = certificationEligible({
        ...certificationInputs,
        topologyProven,
      });
      evidence.distinct_public_egress = {
        evaluated: true,
        pairwise: pairwiseEgress,
        all_observed_addresses_different: allDifferent,
        autonomous_systems: {
          operator: operatorAsn,
          role_b: remoteAsn,
          role_c: thirdAsn,
        },
        distinct_autonomous_system_count: new Set(asns.filter(Boolean)).size,
        independent_network_topology_proven: topologyProven,
        claim: "distinct public egress plus at least two independently resolved BGP origin ASNs; no IP address is persisted",
      };
      if (!allDifferent && !config.allowSharedEgress) {
        throw new Error("the three test roles do not have distinct observed public egress addresses");
      }
      b = await startRemotePeer({
        target: config.remote,
        role: "b",
        runId,
        binary: linuxBinary.path,
        expectedSha: expectedLinuxSha,
        expectedVersion: config.expectedVersion,
        expectedRelayOnly: config.relayOnlyBuild,
        resources,
        secrets: resources.secrets,
      });
      evidence.hosts.push({
        role: "b",
        host: config.remote,
        os: b.os,
        architecture: b.architecture,
        process_privilege: b.processPrivilege,
        binary_validation: b.binaryValidation,
      });
    }

    const peers = [a, b];
    if (config.withThird) {
      let c;
      if (config.localDryrun) {
        c = await startLocalPeer({
          role: "c", binary: localBinary.path, loopback: true, runId, resources, secrets: resources.secrets,
        });
        evidence.hosts.push({ role: "c", host: "operator-local", os: process.platform, architecture: process.arch });
      } else {
        c = await startRemotePeer({
          target: config.thirdRemote,
          role: "c",
          runId,
          binary: linuxBinary.path,
          expectedSha: expectedLinuxSha,
          expectedVersion: config.expectedVersion,
          expectedRelayOnly: config.relayOnlyBuild,
          resources,
          secrets: resources.secrets,
        });
        evidence.hosts.push({
          role: "c",
          host: config.thirdRemote,
          os: c.os,
          architecture: c.architecture,
          process_privilege: c.processPrivilege,
          binary_validation: c.binaryValidation,
        });
      }
      peers.push(c);
    }
    flowResult = await runFlow({ peers, expectedPath: config.expectedPath, runId, resources, record });
    evidence.path_observations = flowResult.path_evidence;
    evidence.functional_evidence = flowResult.functional_evidence;
    evidence.result = assertions.every((assertion) => assertion.result === "pass") ? "pass" : "fail";
  } catch (error) {
    failure = error;
    evidence.result = "fail";
    evidence.failure_code = error.code ?? "verification_failed";
  } finally {
    if (lifecycle.signalName) {
      const interrupted = new Error(`network verification interrupted by ${lifecycle.signalName}`);
      interrupted.code = `interrupted_${lifecycle.signalName.toLowerCase()}`;
      interrupted.exitCode = lifecycle.signalName === "SIGINT" ? 130 : 143;
      failure = interrupted;
      evidence.result = "fail";
      evidence.failure_code = interrupted.code;
    }
    evidence.cleanup = await lifecycle.cleanup();
    evidence.sanitized_logs = {
      policy: "bounded excerpt with exact in-memory secrets, credential labels, and long hex/base64-like tokens redacted; no raw logs persisted",
      excerpt_limit_characters: 1_024,
      roles: resources.peers.map((peer) => ({
        role: peer.role,
        transport: peer.kind === "remote" ? "supervised-ssh" : "local-child",
        streams: {
          stdout: summarizeLogCollector(peer.logs.stdout, resources.secrets),
          stderr: summarizeLogCollector(peer.logs.stderr, resources.secrets),
        },
      })),
    };
    if (sourceBuild) {
      try {
        if (!removeBuildDirectory(sourceBuild.targetDir, runId)) {
          throw new Error("source build directory remains");
        }
      } catch {
        evidence.cleanup.completed = false;
        evidence.cleanup.temporary_artifacts_removed = false;
        evidence.cleanup.failure_codes.push("source-build-artifact-cleanup");
      }
    }
    if (!evidence.cleanup.completed) {
      evidence.result = "fail";
      evidence.failure_code ??= "cleanup_failed";
    }
    evidence.ended_at_utc = new Date().toISOString();
    mkdirSync(EVIDENCE_ROOT, { recursive: true, mode: 0o700 });
    assertEvidenceContainsNoSecrets(evidence, resources.secrets);
    const evidencePath = join(EVIDENCE_ROOT, `${runId}.json`);
    writeFileSync(evidencePath, `${JSON.stringify(evidence, null, 2)}\n`, { mode: 0o600 });
    console.log(`network-evidence: result=${evidence.result} certifiable=${evidence.certifiable}`);
    console.log(`network-evidence: sanitized evidence ${evidencePath}`);
    lifecycle.dispose();
  }
  if (failure) throw failure;
  if (evidence.result !== "pass") throw new Error("network verification failed");
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH) {
  main().catch((error) => {
    console.error(`network-evidence: FAIL — ${error.message}`);
    process.exitCode = error.exitCode ?? 1;
  });
}
