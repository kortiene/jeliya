import assert from "node:assert/strict";
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, test } from "node:test";

import {
  assertEvidenceContainsNoSecrets,
  certificationEligible,
  classifyNetworks,
  commitPublishedAtOrigin,
  dependencyIdentity,
  isPublicGitSource,
  parseExpectedSha256,
  parseCli,
  parseRelayBuildAttestation,
  parseRipeAsn,
  pathMatches,
  pathMatchesExpectedIdentities,
  pathSummary,
  redactLogExcerpt,
  remoteBinaryVerificationCommand,
  remoteCleanupCommand,
  remoteCreateCommand,
  remoteDaemonCommand,
  remoteOwnedDirectoryCleanupCommand,
  remoteRunDir,
  parseRemoteBinaryVerification,
  validBuildDirectory,
  validRunId,
  validSshTarget,
} from "./realnet-evidence.mjs";

const temporary = [];
afterEach(() => {
  for (const dir of temporary.splice(0)) rmSync(dir, { recursive: true, force: true });
});

test("SSH targets and generated run directories reject shell syntax", () => {
  assert.equal(validSshTarget("user@kilo"), true);
  assert.equal(validSshTarget("stargate-03"), true);
  assert.equal(validSshTarget("-oProxyCommand=bad"), false);
  assert.equal(validSshTarget("user@host;touch /tmp/pwned"), false);
  const runId = "20260712T120000Z-0123abcd";
  assert.equal(validRunId(runId), true);
  assert.equal(remoteRunDir(runId, "b"), `/tmp/jeliya-v050-${runId}-b`);
  assert.throws(() => remoteRunDir("../bad", "b"));
});

test("remote wrapper records the exec-stable PID before starting jeliyad", () => {
  const runId = "20260712T120000Z-0123abcd";
  const runDir = remoteRunDir(runId, "b");
  const command = remoteDaemonCommand(runId, runDir);
  const pidWrite = command.indexOf(`> '${runDir}/jeliyad.pid'`);
  const daemonExec = command.indexOf(`exec '${runDir}/jeliyad'`);
  assert.ok(pidWrite >= 0);
  assert.ok(daemonExec > pidWrite);
  assert.match(command, /printf '%s\\n' "\$\$"/);
  assert.throws(() => remoteDaemonCommand(runId, `${runDir}; touch /tmp/unsafe`));
});

test("remote cleanup verifies the exact PID executable before signals and deletion", () => {
  const runId = "20260712T120000Z-0123abcd";
  const runDir = remoteRunDir(runId, "c");
  const command = remoteCleanupCommand(runId, runDir);
  const firstExecutableCheck = command.indexOf('[ "$actual_exe" = "$expected_exe" ]');
  const term = command.indexOf('kill -TERM "$pid"');
  const confirmedStop = command.indexOf('[ ! -e "/proc/$pid" ]');
  const deletion = command.indexOf('find "$run_dir" -depth -delete');
  assert.match(command, /\[ -d \/proc\/1 \] \|\| exit 96/);
  assert.ok(firstExecutableCheck >= 0);
  assert.ok(term > firstExecutableCheck);
  assert.ok(confirmedStop > term);
  assert.ok(deletion > confirmedStop);
  assert.match(command, /readlink "\/proc\/\$pid\/exe"/);
  assert.match(command, /kill -KILL "\$pid"/);
  assert.throws(() => remoteCleanupCommand(runId, "/tmp/jeliya-v050-other-b"));
});

function freshRemoteFixture(role = "b") {
  let runId;
  let runDir;
  do {
    runId = `20260712T120000Z-${randomBytes(4).toString("hex")}`;
    runDir = remoteRunDir(runId, role);
  } while (existsSync(runDir));
  temporary.push(runDir);
  return { runId, runDir };
}

test("a colliding remote path is neither modified nor eligible for cleanup", () => {
  const { runId, runDir } = freshRemoteFixture();
  mkdirSync(runDir, { mode: 0o700 });
  const sentinel = join(runDir, "belongs-to-someone-else");
  writeFileSync(sentinel, "preserve\n");
  assert.notEqual(spawnSync("sh", ["-c", remoteCreateCommand(runId, runDir)]).status, 0);
  assert.equal(readFileSync(sentinel, "utf8"), "preserve\n");
  assert.notEqual(spawnSync(
    "sh",
    ["-c", remoteOwnedDirectoryCleanupCommand(runId, runDir)],
  ).status, 0);
  assert.equal(readFileSync(sentinel, "utf8"), "preserve\n");
});

test("an owner-marked provisioning directory is removed by its bounded cleanup", () => {
  const { runId, runDir } = freshRemoteFixture("c");
  execFileSync("sh", ["-c", remoteCreateCommand(runId, runDir)]);
  assert.equal(existsSync(join(runDir, ".jeliya-run-owner")), true);
  execFileSync("sh", ["-c", remoteOwnedDirectoryCleanupCommand(runId, runDir)]);
  assert.equal(existsSync(runDir), false);
});

test("remote binary verification runs only inside the generated directory", () => {
  const runId = "20260712T120000Z-0123abcd";
  const runDir = remoteRunDir(runId, "b");
  const digest = "ab".repeat(32);
  const command = remoteBinaryVerificationCommand(runId, runDir, digest);
  assert.match(command, new RegExp(`${runDir}/jeliyad`));
  assert.ok(command.indexOf("sha256sum") < command.indexOf('version=$("$binary" --version)'));
  assert.ok(command.indexOf('version=$("$binary" --version)') < command.indexOf("--verification-relay-only-build"));
  assert.throws(() => remoteBinaryVerificationCommand(runId, `${runDir}; touch /tmp/unsafe`, digest));
  assert.throws(() => remoteBinaryVerificationCommand(runId, runDir, "not-a-digest"));
});

test("a digest mismatch prevents any execution of the transferred binary", () => {
  const { runId, runDir } = freshRemoteFixture();
  execFileSync("sh", ["-c", remoteCreateCommand(runId, runDir)]);
  const marker = join(runDir, "executed");
  const binary = join(runDir, "jeliyad");
  writeFileSync(binary, `#!/bin/sh\nprintf executed > '${marker}'\n`);
  chmodSync(binary, 0o700);
  assert.notEqual(spawnSync(
    "sh",
    ["-c", remoteBinaryVerificationCommand(runId, runDir, "ff".repeat(32))],
  ).status, 0);
  assert.equal(existsSync(marker), false);
});

test("remote binary record proves digest, version, and direct-build attestation rejection", () => {
  const digest = "ab".repeat(32);
  const record = [
    `SHA256=${digest}`,
    "VERSION=jeliyad 0.5.0",
    "RELAY_STATUS=2",
    "RELAY_STDOUT_HEX=",
  ].join("\n");
  assert.deepEqual(parseRemoteBinaryVerification(record, {
    expectedSha: digest,
    expectedVersion: "0.5.0",
    expectedRelayOnly: false,
  }), {
    sha256: digest,
    version: "0.5.0",
    relay_only_attested: false,
    relay_attestation_exit_status: 2,
  });
  assert.throws(() => parseRemoteBinaryVerification(record, {
    expectedSha: "cd".repeat(32),
    expectedVersion: "0.5.0",
    expectedRelayOnly: false,
  }), /digest mismatch/);
  assert.throws(() => parseRemoteBinaryVerification(record, {
    expectedSha: digest,
    expectedVersion: "0.5.1",
    expectedRelayOnly: false,
  }), /version mismatch/);
});

test("remote relay-only record requires the exact compile-time marker", () => {
  const digest = "cd".repeat(32);
  const markerHex = Buffer.from("jeliya-relay-only-test-build-v1", "utf8").toString("hex");
  const valid = [
    `SHA256=${digest}`,
    "VERSION=jeliyad 0.5.0",
    "RELAY_STATUS=0",
    `RELAY_STDOUT_HEX=${markerHex}`,
  ].join("\n");
  assert.equal(parseRemoteBinaryVerification(valid, {
    expectedSha: digest,
    expectedVersion: "0.5.0",
    expectedRelayOnly: true,
  }).relay_only_attested, true);
  assert.throws(() => parseRemoteBinaryVerification(valid.replace(markerHex, "00"), {
    expectedSha: digest,
    expectedVersion: "0.5.0",
    expectedRelayOnly: true,
  }), /lacks the compile-time relay-only attestation/);
  assert.throws(() => parseRemoteBinaryVerification(valid, {
    expectedSha: digest,
    expectedVersion: "0.5.0",
    expectedRelayOnly: false,
  }), /ordinary remote binary/);
});

test("certifying remote mode requires two distinct SSH targets", () => {
  assert.throws(() => parseCli(["--remote", "user@kilo"]));
  assert.throws(() => parseCli([
    "--remote", "user@kilo",
    "--third-remote", "user@kilo",
  ]));
  const config = parseCli([
    "--remote", "user@kilo",
    "--third-remote", "user@stargate-03",
  ]);
  assert.equal(config.withThird, true);
  assert.equal(config.remote, "user@kilo");
  assert.equal(config.thirdRemote, "user@stargate-03");
});

test("source-build mode rejects ambiguous binaries and requires Zig integrity input", () => {
  const remote = ["--remote", "user@kilo", "--third-remote", "user@stargate-03"];
  assert.throws(() => parseCli([...remote, "--build-from-source"]));
  assert.throws(() => parseCli([
    ...remote,
    "--build-from-source",
    "--zig-sha256", "ab".repeat(32),
    "--linux-bin", "/tmp/jeliyad",
  ]));
  const config = parseCli([
    ...remote,
    "--build-from-source",
    "--zig-sha256", "ab".repeat(32),
  ]);
  assert.equal(config.buildFromSource, true);
  assert.equal(config.linuxBin, null);
});

test("local dependency sources and build directories are never release provenance", () => {
  assert.equal(isPublicGitSource("https://github.com/kortiene/iroh-room"), true);
  assert.equal(isPublicGitSource("git@github.com:kortiene/iroh-room.git"), true);
  assert.equal(isPublicGitSource("file:///tmp/iroh-room-v050-pin"), false);
  assert.equal(isPublicGitSource("https://localhost/iroh-room"), false);
  assert.equal(isPublicGitSource("https://token@github.com/example/private"), false);
  assert.equal(isPublicGitSource("https://github.com/example/repo?token=secret"), false);
  assert.equal(isPublicGitSource("https://github.com/example/repo#credential"), false);
  assert.equal(isPublicGitSource("git@github.com:example/repo.git?token=secret"), false);
  const runId = "20260712T120000Z-0123abcd";
  assert.equal(validBuildDirectory(join(tmpdir(), `jeliya-v050-${runId}-build-abc123`), runId), true);
  assert.equal(validBuildDirectory("/tmp/unrelated", runId), false);
});

test("local git dependency URLs record their exact checkout but remain non-releaseable", () => {
  const dir = mkdtempSync(join(tmpdir(), "jeliya-local-git-source-"));
  temporary.push(dir);
  execFileSync("git", ["init", "-q"], { cwd: dir });
  writeFileSync(join(dir, "README.md"), "candidate\n");
  execFileSync("git", ["add", "README.md"], { cwd: dir });
  execFileSync("git", [
    "-c", "user.name=Jeliya Test",
    "-c", "user.email=test@localhost",
    "-c", "core.hooksPath=/dev/null",
    "commit", "-qm", "candidate",
  ], { cwd: dir });
  execFileSync("git", ["remote", "add", "origin", "https://github.com/example/upstream.git"], { cwd: dir });
  const commit = execFileSync("git", ["rev-parse", "HEAD"], { cwd: dir, encoding: "utf8" }).trim();
  const source = `file://${dir}`;
  const identity = dependencyIdentity(
    `iroh-rooms = { git = "${source}", rev = "${commit}", features = ["experimental"] }\n`,
    `source = "git+${source}?rev=${commit}#${commit}"\n`,
  );
  assert.equal(identity.kind, "local-git-url");
  assert.equal(identity.resolved_revision, commit);
  assert.equal(identity.local_checkout.commit, commit);
  assert.equal(identity.public_source, false);
  assert.equal(identity.releaseable, false);
});

test("dependency provenance rejects credential-bearing Git URLs", () => {
  const commit = "ab".repeat(20);
  assert.throws(() => dependencyIdentity(
    `iroh-rooms = { git = "https://token@github.com/example/private", rev = "${commit}" }\n`,
    `source = "git+https://token@github.com/example/private?rev=${commit}#${commit}"\n`,
  ), /must not contain URL credentials/);
  assert.throws(() => dependencyIdentity(
    `iroh-rooms = { git = "git@github.com:example/private.git?token=secret", rev = "${commit}" }\n`,
    "",
  ), /must not contain URL credentials/);
});

test("certification fails closed for unpublished source or unproven topology", () => {
  const base = {
    localDryrun: false,
    buildFromSource: true,
    sourceReleaseable: true,
    expectedPath: "direct",
    topologyProven: true,
  };
  assert.equal(certificationEligible(base), true);
  assert.equal(certificationEligible({ ...base, sourceReleaseable: false }), false);
  assert.equal(certificationEligible({ ...base, topologyProven: false }), false);
  assert.equal(certificationEligible({ ...base, localDryrun: true }), false);
  assert.equal(commitPublishedAtOrigin("file:///tmp/local-only", "ab".repeat(20)), false);
});

test("checksum parser accepts an exact digest or strict sidecar", () => {
  const digest = "ab".repeat(32);
  assert.equal(parseExpectedSha256(digest), digest);
  const dir = mkdtempSync(join(tmpdir(), "jeliya-sha-test-"));
  temporary.push(dir);
  const sidecar = join(dir, "jeliyad.sha256");
  writeFileSync(sidecar, `${digest}  jeliyad-v0.5.0-x86_64-unknown-linux-musl.tar.gz\n`);
  assert.equal(parseExpectedSha256(sidecar), digest);
  writeFileSync(sidecar, `${digest} jeliyad\n`);
  assert.throws(() => parseExpectedSha256(sidecar));
});

test("relay attestation accepts only the exact feature-build marker", () => {
  assert.equal(parseRelayBuildAttestation(0, "jeliya-relay-only-test-build-v1\n"), true);
  assert.equal(parseRelayBuildAttestation(0, "jeliyad 0.5.0\n"), false);
  assert.equal(parseRelayBuildAttestation(2, "jeliya-relay-only-test-build-v1\n"), false);
  assert.equal(parseRelayBuildAttestation(0, "prefix jeliya-relay-only-test-build-v1"), false);
});

test("network classification records only distinction and family", () => {
  assert.deepEqual(classifyNetworks("198.51.100.10", "203.0.113.20"), {
    status: "different",
    family: "ipv4",
  });
  assert.deepEqual(classifyNetworks("198.51.100.10", "198.51.100.10"), {
    status: "same",
    family: "ipv4",
  });
  assert.deepEqual(classifyNetworks("", "203.0.113.20"), {
    status: "indeterminate",
    family: null,
  });
  assert.deepEqual(classifyNetworks("not-an-ip", "203.0.113.20"), {
    status: "indeterminate",
    family: null,
  });
  assert.deepEqual(classifyNetworks("2001:db8::1", "203.0.113.20"), {
    status: "different",
    family: "mixed",
  });
  assert.deepEqual(classifyNetworks("2001:db8:0:0::1", "2001:db8::2"), {
    status: "same",
    family: "ipv6",
  });
  assert.equal(parseRipeAsn({ data: { asns: [24940] } }), "AS24940");
  assert.equal(parseRipeAsn({ data: { asns: [1, 2] } }), null);
});

test("path gate requires every expected connected peer", () => {
  const direct = [
    { state: "connected", path: "direct" },
    { state: "connected", path: "direct" },
  ];
  assert.deepEqual(pathSummary(direct), { connected: 2, direct: 2, relay: 0, other: 0 });
  assert.equal(pathMatches(direct, "direct", 2), true);
  assert.equal(pathMatches(direct, "relay", 2), false);
  assert.equal(pathMatches([{ state: "connected", path: "relay" }], "relay"), true);
  assert.equal(pathMatches([{ state: "connecting", path: null }], "any"), false);
  const identified = [
    { identity_id: "alice", state: "connected", path: "direct" },
    { identity_id: "bob", state: "connected", path: "direct" },
    { identity_id: "unknown", state: "connected", path: "relay" },
  ];
  assert.equal(pathMatchesExpectedIdentities(identified, "direct", ["alice", "bob"]), true);
  assert.equal(pathMatchesExpectedIdentities(identified, "relay", ["alice", "bob"]), false);
  assert.equal(pathMatchesExpectedIdentities(identified, "direct", ["alice", "missing"]), false);
});

test("evidence writer rejects secret values and forbidden secret-shaped keys", () => {
  const token = "top-secret-token-value-0123456789";
  assert.doesNotThrow(() => assertEvidenceContainsNoSecrets({ result: "pass" }, new Set([token])));
  assert.throws(() => assertEvidenceContainsNoSecrets({ notes: token }, new Set([token])));
  assert.throws(() => assertEvidenceContainsNoSecrets({ auth_token: "redacted" }, new Set()));
  assert.throws(() => assertEvidenceContainsNoSecrets({ invite_ticket: "redacted" }, new Set()));
});

test("bounded log excerpts redact known and credential-shaped values", () => {
  const secret = "known-runtime-secret-value-0123456789";
  const longHex = "ab".repeat(32);
  const excerpt = redactLogExcerpt(
    `Bearer bearer-value ticket=${secret} private_key=unsafe {"identity_seed":"short-sensitive-value"} ${longHex} ${"x".repeat(2_000)}`,
    new Set([secret]),
  );
  assert.equal(excerpt.includes(secret), false);
  assert.equal(excerpt.includes("bearer-value"), false);
  assert.equal(excerpt.includes("unsafe"), false);
  assert.equal(excerpt.includes("short-sensitive-value"), false);
  assert.equal(excerpt.includes(longHex), false);
  assert.ok(excerpt.length <= 1_024);
});

test("SIGTERM produces failed evidence and completes run-owned cleanup", { timeout: 60_000 }, async () => {
  const binary = join(process.cwd(), "target", "debug", "jeliyad");
  assert.equal(existsSync(binary), true, "build target/debug/jeliyad before the lifecycle test");
  const evidenceDir = mkdtempSync(join(tmpdir(), "jeliya-signal-evidence-"));
  temporary.push(evidenceDir);
  const child = spawn(
    process.execPath,
    [join(process.cwd(), "scripts", "realnet-evidence.mjs"), "--local-dryrun", "--with-third"],
    {
      cwd: process.cwd(),
      env: { ...process.env, JELIYA_EVIDENCE_ROOT: evidenceDir },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  let stdout = "";
  let stderr = "";
  let terminationSent = false;
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    stdout += chunk;
    if (!terminationSent && stdout.includes("network-evidence: ok — a: identity ready")) {
      terminationSent = true;
      child.kill("SIGTERM");
    }
  });
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  const { code, signal } = await new Promise((resolvePromise, rejectPromise) => {
    child.once("error", rejectPromise);
    child.once("exit", (exitCode, exitSignal) => resolvePromise({ code: exitCode, signal: exitSignal }));
  });
  assert.equal(signal, null, `signal handler was bypassed; stderr=${stderr}`);
  assert.equal(code, 143, `unexpected interrupted exit; stderr=${stderr}`);
  const files = readdirSync(evidenceDir).filter((name) => name.endsWith(".json"));
  assert.equal(files.length, 1, `expected one interruption evidence file; stdout=${stdout}`);
  const evidence = JSON.parse(readFileSync(join(evidenceDir, files[0]), "utf8"));
  assert.equal(evidence.result, "fail");
  assert.equal(evidence.failure_code, "interrupted_sigterm");
  assert.equal(evidence.cleanup.completed, true, JSON.stringify(evidence.cleanup));
  assert.equal(evidence.cleanup.processes_stopped, true);
  assert.equal(evidence.cleanup.temporary_artifacts_removed, true);
});
