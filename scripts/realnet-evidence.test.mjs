import assert from "node:assert/strict";
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, delimiter, join } from "node:path";
import { PassThrough } from "node:stream";
import { afterEach, test } from "node:test";

import {
  assertEvidenceContainsNoSecrets,
  attachLogCollectors,
  bindSourceBuildTools,
  certificationEligible,
  classifyNetworks,
  commitPublishedAtOrigin,
  createIsolatedSourceArchive,
  dependencyIdentity,
  isPublicGitSource,
  installVerifiedZig,
  parseExpectedSha256,
  parseCli,
  parseRelayBuildAttestation,
  parseRipeAsn,
  pathMatches,
  pathMatchesExpectedIdentities,
  pathObservationSummary,
  pathSummary,
  redactLogExcerpt,
  remoteBinaryVerificationCommand,
  remoteCleanupCommand,
  remoteCreateCommand,
  remoteDaemonCommand,
  remoteCopyTimeoutMs,
  remoteOwnedDirectoryCleanupCommand,
  remoteRunDir,
  resolveExecutableFromPath,
  joinRoomWithRetries,
  seedForeignIsolationFixture,
  SOURCE_BUILD_ALLOWED_AMBIENT_NAMES,
  SOURCE_BUILD_ENVIRONMENT_POLICY,
  settlePathChecks,
  sourceBuildEnvironment,
  summarizeLogCollector,
  topologyClaim,
  validateSourceBuildToolVersions,
  parseRemoteBinaryVerification,
  validBuildDirectory,
  validRunId,
  validSshTarget,
  validateZigInstallationBinding,
  waitForLogCollectors,
  waitForReady,
  waitPath,
  zigArchiveMembersValid,
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
  assert.equal(remoteRunDir(runId, "b"), `/tmp/jeliya-v060-${runId}-b`);
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
  const dropped = remoteDaemonCommand(runId, runDir, { uid: 65534, gid: 65534 });
  assert.match(dropped, /setpriv --reuid=65534 --regid=65534 --clear-groups/);
  assert.ok(dropped.indexOf("jeliyad.pid") < dropped.indexOf("setpriv"));
  assert.throws(() => remoteDaemonCommand(runId, runDir, { uid: 0, gid: 0 }));
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
  assert.throws(() => remoteCleanupCommand(runId, "/tmp/jeliya-v060-other-b"));
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
  assert.ok(command.indexOf("sha256sum") < command.indexOf('version=$(run_as_peer "$binary" --version)'));
  assert.ok(command.indexOf('version=$(run_as_peer "$binary" --version)') < command.indexOf("--verification-relay-only-build"));
  assert.match(
    remoteBinaryVerificationCommand(runId, runDir, digest, { uid: 65534, gid: 65534 }),
    /run_as_peer\(\) \{ setpriv --reuid=65534 --regid=65534 --clear-groups/,
  );
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
    "EXECUTION_UID=1000",
    "RELAY_STATUS=2",
    "RELAY_STDOUT_HEX=",
  ].join("\n");
  assert.deepEqual(parseRemoteBinaryVerification(record, {
    expectedSha: digest,
    expectedVersion: "0.5.0",
    expectedExecutionUid: "1000",
    expectedRelayOnly: false,
  }), {
    sha256: digest,
    version: "0.5.0",
    execution_uid: "1000",
    relay_only_attested: false,
    relay_attestation_exit_status: 2,
  });
  assert.throws(() => parseRemoteBinaryVerification(record, {
    expectedSha: "cd".repeat(32),
    expectedVersion: "0.5.0",
    expectedExecutionUid: "1000",
    expectedRelayOnly: false,
  }), /digest mismatch/);
  assert.throws(() => parseRemoteBinaryVerification(record, {
    expectedSha: digest,
    expectedVersion: "0.5.1",
    expectedExecutionUid: "1000",
    expectedRelayOnly: false,
  }), /version mismatch/);
});

test("remote relay-only record requires the exact compile-time marker", () => {
  const digest = "cd".repeat(32);
  const markerHex = Buffer.from("jeliya-relay-only-test-build-v1", "utf8").toString("hex");
  const valid = [
    `SHA256=${digest}`,
    "VERSION=jeliyad 0.5.0",
    "EXECUTION_UID=65534",
    "RELAY_STATUS=0",
    `RELAY_STDOUT_HEX=${markerHex}`,
  ].join("\n");
  assert.equal(parseRemoteBinaryVerification(valid, {
    expectedSha: digest,
    expectedVersion: "0.5.0",
    expectedExecutionUid: "65534",
    expectedRelayOnly: true,
  }).relay_only_attested, true);
  assert.throws(() => parseRemoteBinaryVerification(valid.replace(markerHex, "00"), {
    expectedSha: digest,
    expectedVersion: "0.5.0",
    expectedExecutionUid: "65534",
    expectedRelayOnly: true,
  }), /lacks the compile-time relay-only attestation/);
  assert.throws(() => parseRemoteBinaryVerification(valid, {
    expectedSha: digest,
    expectedVersion: "0.5.0",
    expectedExecutionUid: "65534",
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

test("remote binary copies have size-aware but bounded deadlines", () => {
  assert.equal(remoteCopyTimeoutMs(1), 120_000);
  assert.equal(remoteCopyTimeoutMs(53 * 1024 * 1024), 484_000);
  assert.equal(remoteCopyTimeoutMs(Number.MAX_SAFE_INTEGER), 30 * 60_000);
  for (const value of [0, -1, 1.5, Number.NaN, Number.POSITIVE_INFINITY]) {
    assert.throws(() => remoteCopyTimeoutMs(value), /positive safe byte count/);
  }
});

test("source-build mode rejects ambiguous binaries and requires a verified Zig archive", () => {
  const remote = ["--remote", "user@kilo", "--third-remote", "user@stargate-03"];
  assert.throws(() => parseCli([...remote, "--build-from-source"]));
  assert.throws(() => parseCli([
    ...remote,
    "--build-from-source",
    "--zig-sha256", "ab".repeat(32),
  ]), /cannot authenticate Zig's external installation resources/);
  assert.throws(() => parseCli([
    ...remote,
    "--build-from-source",
    "--zig-archive", "/tmp/zig.tar.xz",
    "--zig-archive-sha256", "ab".repeat(32),
    "--linux-bin", "/tmp/jeliyad",
  ]));
  const config = parseCli([
    ...remote,
    "--build-from-source",
    "--zig-archive", "/tmp/zig.tar.xz",
    "--zig-archive-sha256", "ab".repeat(32),
  ]);
  assert.equal(config.buildFromSource, true);
  assert.equal(config.linuxBin, null);
  assert.equal(config.zigArchive, "/tmp/zig.tar.xz");
});

test("certifying source builds fail fast on every pinned tool version", () => {
  const exact = {
    rustcVersion: "rustc 1.91.0 (f8297e351 2025-10-28)\nbinary: rustc",
    cargoVersion: "cargo 1.91.0 (ea2d97820 2025-10-10)",
    nodeVersion: "v22.22.3",
    npmVersion: "10.9.8",
    cargoZigbuildVersion: "cargo-zigbuild 0.23.0",
  };
  assert.doesNotThrow(() => validateSourceBuildToolVersions(exact));
  for (const [name, value, expected] of [
    ["rustcVersion", "rustc 1.92.0", /rustc 1\.91\.0 is required/],
    ["cargoVersion", "cargo 1.92.0", /cargo 1\.91\.0 is required/],
    ["nodeVersion", "v22.22.2", /Node v22\.22\.3 is required/],
    ["npmVersion", "10.9.7", /npm 10\.9\.8 is required/],
    ["cargoZigbuildVersion", "cargo-zigbuild 0.22.1", /cargo-zigbuild 0\.23\.0 is required/],
  ]) {
    assert.throws(
      () => validateSourceBuildToolVersions({ ...exact, [name]: value }),
      expected,
      name,
    );
  }
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
  assert.equal(validBuildDirectory(join(tmpdir(), `jeliya-v060-${runId}-build-abc123`), runId), true);
  assert.equal(validBuildDirectory("/tmp/unrelated", runId), false);
});

test("certifying source builds use an isolated allowlisted environment", () => {
  const root = mkdtempSync(join(tmpdir(), "jeliya-source-build-env-"));
  temporary.push(root);
  const cargoTargetDir = join(root, "target");
  const rustcPath = join(root, "tools", "rustc");
  const cargoPath = join(root, "tools", "cargo");
  const source = {
    PATH: "/untrusted/bin",
    HOME: "/untrusted/home",
    CARGO_HOME: "/untrusted/cargo",
    UNRELATED_VALUE: "must-not-reach-build",
    CI_REGISTRY_PASSWORD: "must-not-reach-build-either",
    SSH_AUTH_SOCK: "/private/tmp/agent.sock",
    HTTPS_PROXY: "https://proxy.example.test:8443",
    NO_PROXY: "localhost,.example.test",
    SSL_CERT_FILE: "/etc/ssl/cert.pem",
  };
  const result = sourceBuildEnvironment(source, {
    targetDir: root,
    cargoTargetDir,
    rustcPath,
    toolPaths: [rustcPath, cargoPath],
  });

  assert.equal(result.evidence.policy, SOURCE_BUILD_ENVIRONMENT_POLICY);
  assert.deepEqual(
    result.evidence.allowed_names,
    [...SOURCE_BUILD_ALLOWED_AMBIENT_NAMES],
  );
  assert.deepEqual(
    result.evidence.inherited_names,
    ["HTTPS_PROXY", "NO_PROXY", "SSL_CERT_FILE"],
  );
  assert.equal(result.env.HTTPS_PROXY, source.HTTPS_PROXY);
  assert.equal(result.env.NO_PROXY, source.NO_PROXY);
  assert.equal(result.env.SSL_CERT_FILE, source.SSL_CERT_FILE);
  assert.equal(result.env.UNRELATED_VALUE, undefined);
  assert.equal(result.env.CI_REGISTRY_PASSWORD, undefined);
  assert.equal(result.env.SSH_AUTH_SOCK, undefined);
  assert.equal(result.env.HOME, join(root, "home"));
  assert.equal(result.env.CARGO_HOME, join(root, "cargo-home"));
  assert.equal(result.env.CARGO_TARGET_DIR, cargoTargetDir);
  assert.equal(result.env.RUSTC, rustcPath);
  assert.equal(result.env.TMPDIR, join(root, "tmp"));
  assert.doesNotMatch(result.env.PATH, /untrusted/);
  assert.match(result.env.PATH, new RegExp(join(root, "tools").replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
});

test("executable discovery ignores ambient Windows extension overrides", () => {
  const root = mkdtempSync(join(tmpdir(), "jeliya-tool-path-"));
  temporary.push(root);
  const executable = join(root, "tool.EXE");
  writeFileSync(executable, "#!/bin/sh\nexit 0\n", { mode: 0o700 });
  chmodSync(executable, 0o700);
  const resolved = resolveExecutableFromPath(
    "tool",
    { PATH: root, PATHEXT: ".\\..\\..\\EVIL" },
    "win32",
  );
  assert.equal(basename(resolved), "tool.EXE");
});

test("absolute build bindings defeat symlinked PATH duplicate substitution", () => {
  const root = mkdtempSync(join(tmpdir(), "jeliya-tool-order-"));
  temporary.push(root);
  const first = join(root, "first");
  const second = join(root, "second");
  const zigReal = join(root, "zig-real");
  const nodeReal = join(root, "node-real");
  for (const directory of [first, second, zigReal, nodeReal]) mkdirSync(directory);
  for (const path of [
    join(zigReal, "zig"),
    join(nodeReal, "node"),
    join(nodeReal, "zig"),
    join(nodeReal, "npm-cli.js"),
    join(nodeReal, "cargo"),
    join(nodeReal, "python3"),
  ]) {
    writeFileSync(path, "#!/bin/sh\nexit 0\n", { mode: 0o700 });
    chmodSync(path, 0o700);
  }
  symlinkSync(join(zigReal, "zig"), join(first, "zig"));
  symlinkSync(join(nodeReal, "node"), join(second, "node"));
  const discoveredZig = resolveExecutableFromPath(
    "zig",
    { PATH: [first, second].join(delimiter) },
  );
  const result = sourceBuildEnvironment(
    { PATH: [first, second].join(delimiter) },
    {
      targetDir: root,
      cargoTargetDir: join(root, "target"),
      rustcPath: join(nodeReal, "cargo"),
      toolPaths: [join(nodeReal, "node"), join(nodeReal, "cargo")],
    },
  );
  // PATH now contains a different `zig` in node-real before any system tool.
  assert.notEqual(resolveExecutableFromPath("zig", result.env), discoveredZig);
  bindSourceBuildTools(result.env, {
    nodePath: join(nodeReal, "node"),
    npmPath: join(nodeReal, "npm-cli.js"),
    cargoPath: join(nodeReal, "cargo"),
    zigPath: discoveredZig,
  });
  assert.equal(result.env.CARGO_ZIGBUILD_ZIG_PATH, discoveredZig);
  assert.equal(
    result.env.CARGO_ZIGBUILD_PYTHON_PATH,
    "/dev/null/jeliya-python-zig-discovery-disabled",
  );
  if (process.platform !== "win32") {
    assert.throws(
      () => writeFileSync(result.env.CARGO_ZIGBUILD_PYTHON_PATH, "cannot exist"),
      /ENOTDIR|not a directory/i,
    );
  }
  assert.equal(result.env.NODE, join(nodeReal, "node"));
});

test("certifying source builds reject ambient controls by name only", () => {
  const root = mkdtempSync(join(tmpdir(), "jeliya-source-build-reject-"));
  temporary.push(root);
  const options = {
    targetDir: root,
    cargoTargetDir: join(root, "target"),
    rustcPath: join(root, "tools", "rustc"),
    toolPaths: [join(root, "tools", "cargo")],
  };
  for (const name of [
    "RUSTFLAGS",
    "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER",
    "CC_x86_64_unknown_linux_musl",
    "NODE_OPTIONS",
    "npm_config_registry",
    "GIT_CONFIG_COUNT",
  ]) {
    const secret = "opaque-secret-value";
    assert.throws(
      () => sourceBuildEnvironment({ [name]: secret }, options),
      (error) => error.message.includes(name) && !error.message.includes(secret),
      name,
    );
  }
  assert.throws(
    () => sourceBuildEnvironment({ HTTPS_PROXY: "https://user:password@proxy.example" }, options),
    /unsafe HTTPS_PROXY/,
  );
  for (const value of [
    "https://proxy.example/?token=secret",
    "https://proxy.example/path",
    "file:///tmp/proxy",
  ]) {
    assert.throws(
      () => sourceBuildEnvironment({ HTTPS_PROXY: value }, options),
      /unsafe HTTPS_PROXY/,
    );
  }
});

test("source archives ignore checkout-local attributes and bind the committed tree", () => {
  const repository = mkdtempSync(join(tmpdir(), "jeliya-archive-source-"));
  const target = mkdtempSync(join(tmpdir(), "jeliya-archive-target-"));
  temporary.push(repository, target);
  execFileSync("git", ["init", "-q"], { cwd: repository });
  writeFileSync(join(repository, "bound.txt"), "committed bytes\n");
  execFileSync("git", ["add", "bound.txt"], { cwd: repository });
  execFileSync("git", [
    "-c", "user.name=Jeliya Test",
    "-c", "user.email=test@localhost",
    "-c", "core.hooksPath=/dev/null",
    "commit", "-qm", "candidate",
  ], { cwd: repository });
  mkdirSync(join(repository, ".git", "info"), { recursive: true });
  writeFileSync(join(repository, ".git", "info", "attributes"), "bound.txt export-ignore\n");

  const sourceCommit = execFileSync(
    "git",
    ["rev-parse", "HEAD"],
    { cwd: repository, encoding: "utf8" },
  ).trim();
  const gitPath = execFileSync("which", ["git"], { encoding: "utf8" }).trim();
  const attributesFile = join(target, "empty-attributes");
  writeFileSync(attributesFile, "");
  const archive = createIsolatedSourceArchive({
    gitPath,
    gitEnv: {
      PATH: process.env.PATH,
      HOME: target,
      GIT_CONFIG_GLOBAL: attributesFile,
      GIT_CONFIG_NOSYSTEM: "1",
      GIT_ATTR_NOSYSTEM: "1",
      GIT_TERMINAL_PROMPT: "0",
      LC_ALL: "C",
      LANG: "C",
    },
    repositoryRoot: repository,
    targetDir: target,
    sourceCommit,
    attributesFile,
  });
  const extracted = join(target, "extracted");
  mkdirSync(extracted);
  execFileSync("tar", ["-xf", archive, "-C", extracted]);
  assert.equal(readFileSync(join(extracted, "bound.txt"), "utf8"), "committed bytes\n");
});

test("Zig archive verification fails before extraction on a digest mismatch", () => {
  const root = mkdtempSync(join(tmpdir(), "jeliya-zig-digest-"));
  temporary.push(root);
  const archive = join(root, "untrusted.tar.xz");
  writeFileSync(archive, "not the reviewed Zig archive");
  assert.throws(() => installVerifiedZig({
    archivePath: archive,
    expectedArchiveSha256: "375b6909fc1495d16fc2c7db9538f707456bfc3373b14ee83fdd3e22b3d43f7f",
    targetDir: root,
    tarPath: join(root, "tar-must-not-run"),
    env: {},
  }), /does not match the reviewed SHA-256/);
  assert.equal(existsSync(join(root, "zig-installation")), false);
});

test("Zig archive layout and installation roots fail closed", () => {
  assert.equal(zigArchiveMembersValid([
    "zig-x86_64-macos-0.15.2/",
    "zig-x86_64-macos-0.15.2/zig",
    "zig-x86_64-macos-0.15.2/lib/zig/std/std.zig",
  ]), true);
  for (const members of [
    ["other-root/zig"],
    ["zig-x86_64-macos-0.15.2/../../outside"],
    ["zig-x86_64-macos-0.15.2\\outside"],
  ]) {
    assert.equal(zigArchiveMembersValid(members), false);
  }

  const root = mkdtempSync(join(tmpdir(), "jeliya-zig-root-"));
  const outside = mkdtempSync(join(tmpdir(), "jeliya-zig-outside-"));
  temporary.push(root, outside);
  const executable = join(root, "zig");
  const library = join(root, "lib", "zig");
  writeFileSync(executable, "verified executable", { mode: 0o700 });
  mkdirSync(library, { recursive: true });
  const binding = validateZigInstallationBinding({
    installationRoot: root,
    executablePath: executable,
    reportedExecutable: executable,
    libDir: library,
    version: "0.15.2",
  });
  assert.equal(basename(binding.executable), "zig");
  assert.equal(basename(binding.libDir), "zig");
  assert.throws(() => validateZigInstallationBinding({
    installationRoot: root,
    executablePath: executable,
    reportedExecutable: executable,
    libDir: outside,
    version: "0.15.2",
  }), /root-bound 0.15.2 installation/);
  const externalExecutable = join(outside, "zig");
  writeFileSync(externalExecutable, "external executable", { mode: 0o700 });
  const linkedExecutable = join(root, "zig-link");
  symlinkSync(externalExecutable, linkedExecutable);
  assert.throws(() => validateZigInstallationBinding({
    installationRoot: root,
    executablePath: linkedExecutable,
    reportedExecutable: externalExecutable,
    libDir: library,
    version: "0.15.2",
  }), /root-bound 0.15.2 installation/);
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
  assert.match(topologyClaim({ topologyProven: true, allDifferent: true }), /^distinct public egress/);
  assert.match(topologyClaim({ topologyProven: false, allDifferent: true }), /fewer than two/);
  assert.match(topologyClaim({ topologyProven: false, allDifferent: false }), /shared public egress/);
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

test("path timeout diagnostics are bounded and omit peer identifiers", async () => {
  const peers = [
    { endpoint_id: "secret-endpoint", identity_id: "secret-identity", state: "connected", path: "relay" },
    { endpoint_id: "anonymous-endpoint", identity_id: null, state: "connecting", path: null },
  ];
  assert.deepEqual(pathObservationSummary(peers, ["secret-identity"]), {
    observed_peers: 2,
    expected_identities: 1,
    matched_identities: 1,
    duplicate_identities: 0,
    state_counts: { connected: 1, connecting: 0, offline: 0, other: 0 },
    path_counts: { direct: 0, relay: 1, none: 0, other: 0 },
  });
  const peer = {
    role: "b",
    client: { async call() { return { peers }; } },
  };
  await assert.rejects(
    () => waitPath(peer, "room", "direct", ["secret-identity"], {
      timeoutMs: 0,
      intervalMs: 0,
      sleepFn: async () => {},
    }),
    (error) => {
      assert.equal(error.code, "path_settlement_timeout");
      assert.match(error.message, /observations=1 best_consecutive=0/);
      assert.match(error.message, /"relay":1/);
      assert.doesNotMatch(error.message, /secret|endpoint|identity/);
      return true;
    },
  );
});

test("path checks start concurrently and retain declaration order", async () => {
  const started = [];
  const recorded = [];
  let release;
  const gate = new Promise((resolvePromise) => { release = resolvePromise; });
  const checks = ["a", "b", "c"].map((key) => ({
    key,
    name: key.toUpperCase(),
    run: async () => {
      started.push(key);
      if (started.length === 3) release();
      await gate;
      return `${key}-path`;
    },
  }));
  const result = await settlePathChecks(checks, async (name, run) => {
    recorded.push(name);
    return run();
  });
  assert.deepEqual(started, ["a", "b", "c"]);
  assert.deepEqual(recorded, ["A", "B", "C"]);
  assert.deepEqual(result, { a: "a-path", b: "b-path", c: "c-path" });
});

test("foreign-room security fixtures join the agent before non-membership events", async () => {
  const dataDir = mkdtempSync(join(tmpdir(), "jeliya-foreign-fixture-"));
  temporary.push(dataDir);
  const calls = [];
  const owner = {
    dataDir,
    client: {
      async call(method) {
        calls.push(`owner:${method}`);
        if (method === "room.create") return { room_id: "foreign-room" };
        if (method === "room.open") return { endpoint: { addr: "identity@127.0.0.1:1" } };
        if (method === "invite.create") return { ticket: "fixture-ticket" };
        if (method === "file.share") return { file_id: "foreign-file" };
        if (method === "pipe.expose") return { pipe_id: "foreign-pipe" };
        return {};
      },
    },
  };
  const agent = {
    client: {
      async call(method) {
        calls.push(`agent:${method}`);
        return {};
      },
    },
  };
  const resources = { secrets: new Set() };
  const result = await seedForeignIsolationFixture({
    owner,
    agent,
    agentIdentityId: "agent-id",
    runId: "20260712T120000Z-0123abcd",
    resources,
  });
  const joinCall = calls.indexOf("agent:room.join");
  assert.ok(joinCall >= 0);
  for (const nonMembershipCall of [
    "owner:message.send",
    "owner:file.share",
    "agent:status.post",
    "owner:pipe.expose",
  ]) {
    assert.ok(
      calls.indexOf(nonMembershipCall) > joinCall,
      `${nonMembershipCall} must occur after room.join: ${calls.join(", ")}`,
    );
  }
  assert.equal(result.foreign.room_id, "foreign-room");
  assert.equal(result.file.file_id, "foreign-file");
  assert.equal(result.pipeId, "foreign-pipe");
  assert.equal(result.joinAttempts, 1);
  assert.deepEqual([...resources.secrets], ["fixture-ticket"]);
});

test("room joins retry only bounded peer-unreachable failures", async () => {
  let calls = 0;
  const waits = [];
  const client = {
    async call() {
      calls += 1;
      if (calls < 3) {
        const error = new Error("transient dial window missed");
        error.code = "peer_unreachable";
        throw error;
      }
      return { room_id: "joined-room" };
    },
  };
  const joined = await joinRoomWithRetries(client, { ticket: "test" }, {
    attempts: 3,
    retryDelayMs: 7,
    wait: async (delay) => waits.push(delay),
  });
  assert.equal(joined.result.room_id, "joined-room");
  assert.equal(joined.attempts_used, 3);
  assert.deepEqual(waits, [7, 7]);

  const denied = new Error("invalid invite");
  denied.code = "invite_denied";
  let deniedCalls = 0;
  await assert.rejects(() => joinRoomWithRetries({
    async call() {
      deniedCalls += 1;
      throw denied;
    },
  }, {}, { attempts: 5, retryDelayMs: 0 }), /invalid invite/);
  assert.equal(deniedCalls, 1);
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

test("log digests wait for stream completion and include late bytes", async () => {
  const proc = { stdout: new PassThrough(), stderr: new PassThrough() };
  const logs = attachLogCollectors(proc);
  proc.stdout.write(Buffer.from("early "));
  assert.throws(
    () => summarizeLogCollector(logs.stdout),
    /before its stream closes/,
  );
  setTimeout(() => {
    proc.stdout.end(Buffer.from("late\n"));
    proc.stderr.end(Buffer.from("error\n"));
  }, 10);
  assert.equal(await waitForLogCollectors(logs, 1_000), true);
  const stdout = summarizeLogCollector(logs.stdout);
  assert.deepEqual(stdout, {
    lines: 1,
    bytes: Buffer.byteLength("early late\n"),
    sha256: createHash("sha256").update("early late\n").digest("hex"),
  });
  assert.equal(summarizeLogCollector(logs.stdout), stdout, "digest finalization must be idempotent");
});

test("ready parsing preserves raw non-UTF8 log bytes for hashing", async () => {
  const readyLine = `${JSON.stringify({ event: "ready", port: 7420 })}\n`;
  const raw = Buffer.concat([Buffer.from([0xff, 0x0a]), Buffer.from(readyLine)]);
  const child = spawn(process.execPath, ["-e", [
    `process.stdout.write(Buffer.from(${JSON.stringify([...raw])}));`,
    "setTimeout(() => {}, 50);",
  ].join("")], { stdio: ["ignore", "pipe", "pipe"] });
  const readyPromise = waitForReady(child, "raw-log-fixture", 5_000);
  const logs = attachLogCollectors(child);
  const ready = await readyPromise;
  assert.equal(ready.port, 7420);
  assert.equal(await waitForLogCollectors(logs, 5_000), true);
  const summary = summarizeLogCollector(logs.stdout);
  assert.equal(summary.bytes, raw.length);
  assert.equal(summary.sha256, createHash("sha256").update(raw).digest("hex"));
});

test("log collection times out fail-closed and freezes the digest", async () => {
  const proc = { stdout: new PassThrough(), stderr: new PassThrough() };
  const logs = attachLogCollectors(proc);
  proc.stdout.write("partial");
  assert.equal(await waitForLogCollectors(logs, 5), false);
  assert.doesNotThrow(() => summarizeLogCollector(logs.stdout));
  assert.equal(logs.stdout.stream.destroyed, true);
  assert.equal(logs.stderr.stream.destroyed, true);
});

test("log collection rejects streams destroyed before readable end", async () => {
  const proc = { stdout: new PassThrough(), stderr: new PassThrough() };
  const logs = attachLogCollectors(proc);
  proc.stdout.write("truncated");
  proc.stdout.destroy();
  proc.stderr.destroy();
  assert.equal(await waitForLogCollectors(logs, 1_000), false);
  assert.equal(logs.stdout.stream.readableEnded, false);
  assert.equal(logs.stdout.stream.readableAborted, true);
  assert.match(logs.stdout.streamError.message, /closed before its readable end/);
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
