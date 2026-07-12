#!/usr/bin/env node
// Release-integrity gate for source versions and the complete daemon artifact
// set. Node 22+, no npm dependencies.
//
//   node scripts/check-release.mjs --source [--publish] [--tag v0.5.0]
//   node scripts/check-release.mjs --artifacts dist --tag v0.5.0

import { execFileSync } from "node:child_process";
import { createHash, createPublicKey, verify as verifySignature } from "node:crypto";
import {
  lstatSync,
  readFileSync,
  readdirSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const TARGETS = Object.freeze([
  ["aarch64-apple-darwin", "tar.gz", "jeliyad"],
  ["x86_64-apple-darwin", "tar.gz", "jeliyad"],
  ["aarch64-unknown-linux-musl", "tar.gz", "jeliyad"],
  ["x86_64-unknown-linux-musl", "tar.gz", "jeliyad"],
  ["x86_64-pc-windows-msvc", "zip", "jeliyad.exe"],
]);

function fail(message) {
  throw new Error(`release-integrity: ${message}`);
}

function readText(relativePath, root = repoRoot) {
  return readFileSync(join(root, relativePath), "utf8");
}

function tomlPackageVersion(relativePath, root = repoRoot) {
  const match = readText(relativePath, root).match(/^version\s*=\s*"([^"]+)"/m);
  if (!match) fail(`${relativePath} has no package version`);
  return match[1];
}

function cargoLockVersion(packageName, root = repoRoot) {
  const escaped = packageName.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = readText("Cargo.lock", root).match(
    new RegExp(`\\[\\[package\\]\\]\\nname = "${escaped}"\\nversion = "([^"]+)"`),
  );
  if (!match) fail(`Cargo.lock has no ${packageName} package entry`);
  return match[1];
}

function normalizedTag(tag, fallbackVersion) {
  const value = tag || `v${fallbackVersion}`;
  if (!/^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(value)) {
    fail(`tag ${JSON.stringify(value)} is not v<semver>`);
  }
  return value;
}

export function validateSourceVersions({ root = repoRoot, tag = "" } = {}) {
  const versions = {
    daemon: tomlPackageVersion("crates/jeliyad/Cargo.toml", root),
    core: tomlPackageVersion("crates/jeliya-core/Cargo.toml", root),
    daemonLock: cargoLockVersion("jeliyad", root),
    coreLock: cargoLockVersion("jeliya-core", root),
    ui: JSON.parse(readText("ui/package.json", root)).version,
  };
  const packageLock = JSON.parse(readText("ui/package-lock.json", root));
  versions.uiLock = packageLock.version;
  versions.uiLockRoot = packageLock.packages?.[""]?.version;

  const releaseTag = normalizedTag(tag, versions.daemon);
  const releaseVersion = releaseTag.slice(1);
  for (const [surface, version] of Object.entries(versions)) {
    if (version !== releaseVersion) {
      fail(`${surface} version ${JSON.stringify(version)} does not match ${releaseTag}`);
    }
  }

  const escapedVersion = releaseVersion.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  if (!new RegExp(`^## \\[${escapedVersion}\\] - \\d{4}-\\d{2}-\\d{2}$`, "m").test(
    readText("CHANGELOG.md", root),
  )) {
    fail(`CHANGELOG.md has no dated ## [${releaseVersion}] release heading`);
  }

  return { tag: releaseTag, version: releaseVersion, versions };
}

export function validateEvidenceReadiness({
  root = repoRoot,
  context = null,
  version = "0.5.0",
} = {}) {
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
    fail(`invalid evidence version ${JSON.stringify(version)}`);
  }
  const evidence = readText("docs/verification-evidence.md", root);
  if (!/^implementation_status: "implemented"$/m.test(evidence)) {
    fail("verification evidence implementation_status is not implemented");
  }
  if (!/^verification_status: "verified"$/m.test(evidence)) {
    fail("verification evidence is not verified");
  }
  if (!/^\| Release evidence gate \| READY \|$/m.test(evidence)) {
    fail("release evidence gate is not READY");
  }
  const candidateSection = evidence.match(/## Candidate identity\n([\s\S]*?)(?=\n## )/)?.[1] ?? "";
  if (/\bpending\b/i.test(candidateSection)) {
    fail("candidate identity still contains pending provenance");
  }
  const tableValue = (label) => {
    const escaped = label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const matches = [...candidateSection.matchAll(
      new RegExp(`^\\| ${escaped} \\| ` + "`([^`]+)`" + ` \\|$`, "gm"),
    )];
    if (matches.length !== 1) fail(`candidate evidence must contain exactly one ${label} row`);
    return matches[0][1];
  };
  const candidateCommit = tableValue("Network-qualified commit");
  const upstreamRevision = tableValue("Candidate upstream remediation revision");
  if (!/^[0-9a-f]{40}$/.test(candidateCommit)) {
    fail("network-qualified commit must be exact 40-hex");
  }
  if (!/^[0-9a-f]{40}$/.test(upstreamRevision)) {
    fail("candidate upstream remediation revision must be exact 40-hex");
  }

  const manifests = Object.fromEntries(["direct", "relay"].map((path) => {
    const relativePath = `docs/evidence/v${version}/${path}.json`;
    let manifest;
    try {
      const bytes = readFileSync(join(root, relativePath));
      validateEvidenceSignature(root, relativePath, bytes);
      manifest = JSON.parse(bytes.toString("utf8"));
    } catch {
      fail(`${relativePath} is missing, unsigned, invalidly signed, or not valid JSON`);
    }
    validateNetworkEvidenceManifest(manifest, {
      expectedPath: path,
      candidateCommit,
      upstreamRevision,
      expectedVersion: version,
      relativePath,
    });
    return [path, manifest];
  }));

  if (manifests.direct.source.commit !== manifests.relay.source.commit) {
    fail("direct and relay evidence refer to different Jeliya commits");
  }

  const releaseContext = context ?? releaseEvidenceContext(root, candidateCommit);
  if (releaseContext.upstreamRequestedRevision !== upstreamRevision
      || releaseContext.upstreamResolvedRevision !== upstreamRevision) {
    fail("documented upstream revision does not match Cargo.toml and Cargo.lock");
  }
  if (!releaseContext.upstreamPublic) {
    fail("iroh-rooms release dependency is not an immutable public HTTPS Git source");
  }
  if (!releaseContext.candidateIsAncestor) {
    fail("network-qualified commit is not an ancestor of the release checkout");
  }
  const disallowed = releaseContext.changedPaths.filter((path) => !path.startsWith("docs/"));
  if (disallowed.length > 0) {
    fail(`runtime or release inputs changed after network qualification: ${disallowed.join(", ")}`);
  }
  return { ready: true, candidateCommit, upstreamRevision };
}

export function validateEvidenceSignature(root, relativePath, contents) {
  let publicKey;
  let signatureText;
  try {
    const publicKeyPem = readText("release/evidence-ed25519-public.pem", root);
    if (!/^-----BEGIN PUBLIC KEY-----\n(?:[A-Za-z0-9+/]{64}\n)*[A-Za-z0-9+/=]+\n-----END PUBLIC KEY-----\n?$/.test(publicKeyPem)
        || /PRIVATE KEY/.test(publicKeyPem)) {
      fail("release evidence key must be one canonical public SPKI PEM");
    }
    publicKey = createPublicKey(publicKeyPem);
    const canonical = publicKey.export({ type: "spki", format: "pem" }).toString();
    if (`${publicKeyPem.trim()}\n` !== canonical) {
      fail("release evidence public key is not canonical SPKI PEM");
    }
    signatureText = readText(`${relativePath}.sig`, root).trim();
  } catch {
    fail("retained network evidence requires the pinned Ed25519 release-evidence public key and detached signature");
  }
  if (publicKey.asymmetricKeyType !== "ed25519"
      || !/^[A-Za-z0-9+/]{86}==$/.test(signatureText)) {
    fail(`${relativePath}.sig is not one canonical Ed25519 base64 signature`);
  }
  const signature = Buffer.from(signatureText, "base64");
  if (signature.length !== 64
      || signature.toString("base64") !== signatureText
      || !verifySignature(null, contents, publicKey, signature)) {
    fail(`${relativePath}.sig does not verify against the pinned evidence key`);
  }
  return { valid: true };
}

function strictPublicGitHubUrl(value) {
  try {
    const url = new URL(value);
    return url.protocol === "https:"
      && url.hostname === "github.com"
      && !url.username
      && !url.password
      && !url.search
      && !url.hash
      && /^\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+(?:\.git)?$/.test(url.pathname);
  } catch {
    return false;
  }
}

export function irohRoomsReleaseIdentity(root = repoRoot) {
  const dependencyLine = readText("Cargo.toml", root)
    .split(/\r?\n/)
    .find((line) => /^iroh-rooms\s*=/.test(line));
  const gitUrl = dependencyLine?.match(/\bgit\s*=\s*"([^"]+)"/)?.[1] ?? "";
  const requestedRevision = dependencyLine?.match(/\brev\s*=\s*"([0-9a-f]{40})"/)?.[1] ?? "";
  if (!strictPublicGitHubUrl(gitUrl) || !requestedRevision) {
    fail("Cargo.toml iroh-rooms must use a public GitHub HTTPS URL and exact revision");
  }

  const packageBlock = readText("Cargo.lock", root)
    .split(/\n(?=\[\[package\]\])/)
    .find((block) => /^\[\[package\]\]\nname = "iroh-rooms"$/m.test(block));
  const source = packageBlock?.match(/^source = "([^"]+)"$/m)?.[1] ?? "";
  const sourceMatch = source.match(/^git\+(https:\/\/github\.com\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+(?:\.git)?)\?rev=([0-9a-f]{40})#([0-9a-f]{40})$/);
  if (!sourceMatch || !strictPublicGitHubUrl(sourceMatch[1])) {
    fail("Cargo.lock iroh-rooms source is not an exact public GitHub revision");
  }
  const [, resolvedUrl, lockedRequest, resolvedRevision] = sourceMatch;
  if (resolvedUrl.replace(/\.git$/, "") !== gitUrl.replace(/\.git$/, "")) {
    fail("Cargo.toml and Cargo.lock iroh-rooms repositories differ");
  }
  if (lockedRequest !== requestedRevision || resolvedRevision !== requestedRevision) {
    fail("Cargo.toml requested and Cargo.lock resolved iroh-rooms revisions differ");
  }
  return {
    gitUrl,
    requestedRevision,
    resolvedRevision,
    publicSource: true,
  };
}

export function releaseEvidenceContext(root, candidateCommit) {
  const identity = irohRoomsReleaseIdentity(root);
  const head = execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: root,
    encoding: "utf8",
  }).trim();
  const ancestor = execFileSync(
    "git",
    ["merge-base", "--is-ancestor", candidateCommit, head],
    { cwd: root, stdio: "ignore" },
  );
  void ancestor;
  const changed = execFileSync(
    "git",
    ["diff", "--name-only", `${candidateCommit}..${head}`],
    { cwd: root, encoding: "utf8" },
  ).trim();
  return {
    headCommit: head,
    upstreamRequestedRevision: identity.requestedRevision,
    upstreamResolvedRevision: identity.resolvedRevision,
    upstreamPublic: identity.publicSource,
    candidateIsAncestor: true,
    changedPaths: changed ? changed.split(/\r?\n/) : [],
  };
}

export function expectedNetworkAssertionNames(path) {
  return [
    "a: identity ready",
    "b: identity ready",
    "c: identity ready",
    "A: room created",
    "A: room opened",
    "b: targeted room join",
    "b: joined room opened",
    "A: observes b membership",
    "c: targeted room join",
    "c: joined room opened",
    "A: observes c membership",
    `A: ${path} path settled`,
    `B: ${path} path settled`,
    `C: ${path} path settled`,
    "A to B message authored",
    "B receives A message",
    "B to A message authored",
    "A receives B message",
    "C receives both messages",
    "C message converges to A and B",
    "A shares candidate payload",
    "B lists candidate payload as available",
    "B fetches and BLAKE3-verifies payload",
    "B fetched bytes are byte-identical",
    "A exposes one-peer pipe",
    "C pipe gate forwards zero bytes to the target",
    "B observes and connects authorized pipe",
    "A closes pipe and B observes closure",
    "B closes live room session",
    "A authors message while B session is closed",
    "B reopens and receives the offline message",
    `B reconnects over ${path}`,
    "A seeds isolated foreign-room fixtures",
    "B public room-scoped RPCs do not disclose a foreign room ID",
    "B local-file HTTP endpoint does not disclose a foreign room ID",
    "B aggregate reads omit the foreign room and agent projection",
  ];
}

export function validateNetworkEvidenceManifest(manifest, {
  expectedPath,
  candidateCommit,
  upstreamRevision,
  expectedVersion = "0.5.0",
  relativePath = "evidence manifest",
}) {
  const startedAt = Date.parse(manifest?.started_at_utc ?? "");
  const endedAt = Date.parse(manifest?.ended_at_utc ?? "");
  if (!/^\d{8}T\d{6}Z-[0-9a-f]{8}$/.test(manifest?.run_id ?? "")
      || !Number.isFinite(startedAt)
      || !Number.isFinite(endedAt)
      || endedAt <= startedAt) {
    fail(`${relativePath} lacks a valid run ID and bounded UTC evidence window`);
  }
  if (manifest?.schema !== 1 || manifest.result !== "pass" || manifest.certifiable !== true) {
    fail(`${relativePath} is not a passing certifiable schema-1 run`);
  }
  if (manifest.expected_path !== expectedPath) {
    fail(`${relativePath} expected_path is not ${expectedPath}`);
  }
  const expectedMode = expectedPath === "relay" ? "remote-relay-only-build" : "remote-real-network";
  if (manifest.mode !== expectedMode) {
    fail(`${relativePath} mode is not ${expectedMode}`);
  }
  if (manifest.source?.commit !== candidateCommit
      || manifest.source?.dirty !== false
      || manifest.source?.published_at_origin !== true
      || manifest.source?.releaseable !== true) {
    fail(`${relativePath} does not bind to the releaseable network-qualified commit`);
  }
  if (manifest.source?.iroh_rooms_revision !== upstreamRevision
      || manifest.source?.iroh_rooms?.requested_revision !== upstreamRevision
      || manifest.source?.iroh_rooms?.resolved_revision !== upstreamRevision
      || manifest.source?.iroh_rooms?.public_source !== true
      || manifest.source?.iroh_rooms?.published_at_origin !== true
      || manifest.source?.iroh_rooms?.releaseable !== true) {
    fail(`${relativePath} does not bind to the releaseable upstream revision`);
  }
  if (manifest.build?.mode !== "from-source"
      || manifest.build?.source_bound !== true
      || manifest.build?.source_snapshot_commit !== candidateCommit
      || manifest.build?.locked !== true
      || !Array.isArray(manifest.build?.features)
      || !manifest.build.features.includes("embed-ui")
      || !Array.isArray(manifest.build?.targets)
      || !manifest.build.targets.includes("x86_64-apple-darwin")
      || !manifest.build.targets.includes("x86_64-unknown-linux-musl")) {
    fail(`${relativePath} was not built source-bound with the lockfile`);
  }
  const topology = manifest.distinct_public_egress;
  const pairwise = topology?.pairwise;
  const asns = topology?.autonomous_systems;
  if (topology?.all_observed_addresses_different !== true
      || topology?.independent_network_topology_proven !== true
      || !Number.isInteger(topology?.distinct_autonomous_system_count)
      || topology.distinct_autonomous_system_count < 2
      || !pairwise
      || Object.values(pairwise).length !== 3
      || ["operator_to_b", "operator_to_c", "b_to_c"].some((name) => (
        pairwise[name]?.status !== "different"
        || !["ipv4", "ipv6", "mixed"].includes(pairwise[name]?.family)
      ))
      || !asns
      || ["operator", "role_b", "role_c"].some((role) => !/^AS[1-9]\d*$/.test(asns[role] ?? ""))
      || new Set([asns?.operator, asns?.role_b, asns?.role_c]).size
        !== topology.distinct_autonomous_system_count) {
    fail(`${relativePath} does not prove the required sanitized topology`);
  }
  const assertions = manifest.assertions;
  const expectedAssertions = expectedNetworkAssertionNames(expectedPath);
  if (!Array.isArray(assertions)
      || assertions.some((assertion) => assertion.result !== "pass"
        || !Number.isFinite(assertion.duration_ms)
        || assertion.duration_ms < 0)
      || JSON.stringify(assertions.map((assertion) => assertion.name))
        !== JSON.stringify(expectedAssertions)) {
    fail(`${relativePath} does not contain a complete all-pass assertion set`);
  }
  const paths = manifest.path_observations;
  for (const role of ["a", "b", "c"]) {
    const observation = paths?.[role];
    if (observation?.expected_path !== expectedPath
        || observation?.consecutive_observations < 3
        || !Number.isInteger(observation?.expected_identities)
        || observation.expected_identities < 1) {
      fail(`${relativePath} lacks stable ${expectedPath} observations for role ${role}`);
    }
  }
  const functional = manifest.functional_evidence;
  const deniedMethods = [
    "room.open", "room.close", "room.leave", "room.timeline", "room.members",
    "invite.create", "message.send", "status.post", "file.share", "file.list",
    "file.fetch", "pipe.expose", "pipe.list", "pipe.connect", "pipe.close",
    "peers.status", "agent.history",
  ];
  if (functional?.file?.engine_verified !== true
      || functional?.file?.sha256_equal !== true
      || functional?.file?.expected_sha256 !== functional?.file?.actual_sha256
      || functional?.pipe?.unauthorized_third_peer?.target_connections !== 0
      || functional?.pipe?.unauthorized_third_peer?.target_requests !== 0
      || functional?.reconnect?.offline_message_resynchronized !== true
      || functional?.reconnect?.settled_path?.expected_path !== expectedPath
      || functional?.multi_peer?.peers !== 3
      || functional?.multi_peer?.convergence_verified !== true
      || JSON.stringify(functional?.foreign_room_non_disclosure?.rpc_methods_denied)
        !== JSON.stringify(deniedMethods)
      || functional?.foreign_room_non_disclosure?.local_file_http_denied !== true
      || functional?.foreign_room_non_disclosure?.aggregate_reads_filtered !== true
      || functional?.foreign_room_non_disclosure?.foreign_agent_projection_exercised !== true
      || !Number.isInteger(functional?.foreign_room_non_disclosure?.foreign_agent_join_attempts)
      || functional.foreign_room_non_disclosure.foreign_agent_join_attempts < 1
      || functional.foreign_room_non_disclosure.foreign_agent_join_attempts > 5
      || functional?.foreign_room_non_disclosure?.synchronization_isolation_claimed !== false) {
    fail(`${relativePath} lacks complete functional and authorization evidence`);
  }
  if (manifest.cleanup?.completed !== true
      || manifest.cleanup?.processes_stopped !== true
      || manifest.cleanup?.temporary_artifacts_removed !== true
      || manifest.cleanup?.failure_codes?.length !== 0) {
    fail(`${relativePath} cleanup is incomplete`);
  }
  const remoteHosts = Array.isArray(manifest.hosts)
    ? manifest.hosts.filter((host) => host.role === "b" || host.role === "c")
    : [];
  const remoteDigest = manifest.binaries?.remote?.sha256;
  const expectedFeatures = expectedPath === "relay"
    ? ["embed-ui", "relay-only-test"]
    : ["embed-ui"];
  if (JSON.stringify(manifest.build.features) !== JSON.stringify(expectedFeatures)) {
    fail(`${relativePath} has an unexpected source-build feature set`);
  }
  if (remoteHosts.length !== 2
      || new Set(remoteHosts.map((host) => host.role)).size !== 2
      || new Set(remoteHosts.map((host) => host.host)).size !== 2
      || !/^[0-9a-f]{64}$/.test(remoteDigest ?? "")
      || remoteHosts.some((host) => host.architecture !== "x86_64"
        || typeof host.os !== "string"
        || host.os.length < 3
        || !["dropped-to-unprivileged-system-uid", "ssh-account-uid"].includes(host.process_privilege)
        || host.binary_validation?.sha256 !== remoteDigest
        || host.binary_validation?.version !== expectedVersion
        || !/^[1-9]\d*$/.test(host.binary_validation?.execution_uid ?? ""))) {
    fail(`${relativePath} lacks two independently verified remote binaries`);
  }
  const logRoles = manifest.sanitized_logs?.roles;
  if (!Array.isArray(logRoles)
      || JSON.stringify(logRoles.map((role) => role.role)) !== JSON.stringify(["a", "b", "c"])
      || logRoles.some((role) => ["stdout", "stderr"].some((stream) => {
        const record = role.streams?.[stream];
        return !Number.isInteger(record?.lines)
          || record.lines < 0
          || !Number.isInteger(record?.bytes)
          || record.bytes < 0
          || !/^[0-9a-f]{64}$/.test(record?.sha256 ?? "");
      }))) {
    fail(`${relativePath} lacks sanitized per-role log integrity records`);
  }
  const relayAttested = manifest.binaries?.local?.relay_only_attested === true
    && remoteHosts.every((host) => host.binary_validation?.relay_only_attested === true);
  if (expectedPath === "relay" && !relayAttested) {
    fail(`${relativePath} lacks compile-time relay-only attestation on every execution host`);
  }
  if (manifest.binaries?.local?.version !== expectedVersion
      || manifest.binaries?.remote?.expected_version !== expectedVersion
      || (expectedPath === "direct" && (
        manifest.binaries.local.relay_only_attested !== false
        || remoteHosts.some((host) => host.binary_validation?.relay_only_attested !== false)
      ))) {
    fail(`${relativePath} binary versions or direct-build attestations are inconsistent`);
  }
  return { valid: true };
}

export function expectedArtifactNames(tag) {
  const releaseTag = normalizedTag(tag, "0.0.0");
  return TARGETS.flatMap(([target, extension]) => {
    const archive = `jeliyad-${releaseTag}-${target}.${extension}`;
    return [archive, `${archive}.sha256`];
  }).sort();
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function archiveMemberNames(path, extension) {
  const output = extension === "zip"
    ? execFileSync("unzip", ["-Z1", path], { encoding: "utf8" })
    : execFileSync("tar", ["-tzf", path], { encoding: "utf8" });
  return output.split(/\r?\n/).filter(Boolean);
}

export function validateArtifactSet(directory, tag, { inspectArchives = true } = {}) {
  const root = resolve(directory);
  const expected = expectedArtifactNames(tag);
  const entries = readdirSync(root, { withFileTypes: true });
  const actual = entries.map((entry) => entry.name).sort();

  if (entries.some((entry) => !entry.isFile())) {
    fail(`artifact directory must contain files only: ${root}`);
  }
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    fail(`artifact names differ\nexpected: ${expected.join(", ")}\nactual:   ${actual.join(", ")}`);
  }

  for (const [target, extension, expectedMember] of TARGETS) {
    const archive = `jeliyad-${normalizedTag(tag, "0.0.0")}-${target}.${extension}`;
    const archivePath = join(root, archive);
    const sidecarPath = `${archivePath}.sha256`;
    if (!lstatSync(archivePath).isFile() || !lstatSync(sidecarPath).isFile()) {
      fail(`${archive} or its checksum sidecar is not a regular file`);
    }

    const lines = readFileSync(sidecarPath, "utf8").trimEnd().split(/\r?\n/);
    if (lines.length !== 1) fail(`${archive}.sha256 must contain exactly one line`);
    const match = lines[0].match(/^([0-9a-fA-F]{64})  ([^\s]+)$/);
    if (!match || match[2] !== archive) {
      fail(`${archive}.sha256 must name exactly ${archive} with a 64-hex digest`);
    }
    const actualDigest = sha256(archivePath);
    if (match[1].toLowerCase() !== actualDigest) {
      fail(`${archive} does not match its published checksum`);
    }

    if (inspectArchives) {
      const members = archiveMemberNames(archivePath, extension);
      if (members.length !== 1 || members[0] !== expectedMember) {
        fail(`${archive} must contain exactly ${expectedMember}; got ${members.join(", ")}`);
      }
    }
  }

  return { files: expected, count: expected.length };
}

function flagValue(argv, name) {
  const index = argv.indexOf(name);
  if (index === -1) return "";
  if (!argv[index + 1] || argv[index + 1].startsWith("--")) {
    fail(`${name} requires a value`);
  }
  return argv[index + 1];
}

function main() {
  const argv = process.argv.slice(2);
  const artifacts = flagValue(argv, "--artifacts");
  const tag = flagValue(argv, "--tag") || process.env.GITHUB_REF_NAME || "";
  const checkSource = argv.includes("--source") || !artifacts;
  const checkPublish = argv.includes("--publish");

  let sourceResult = null;
  if (checkSource) {
    sourceResult = validateSourceVersions({ tag });
    console.log(`release-integrity: source versions match ${sourceResult.tag}`);
  }
  if (checkPublish) {
    validateEvidenceReadiness({
      version: sourceResult?.version ?? tomlPackageVersion("crates/jeliyad/Cargo.toml"),
    });
    console.log("release-integrity: evidence gate is READY");
  }
  if (artifacts) {
    if (!tag) fail("--artifacts requires --tag (or GITHUB_REF_NAME)");
    const result = validateArtifactSet(artifacts, tag);
    console.log(`release-integrity: ${result.count} artifact files verified for ${tag}`);
  }
}

if (resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}
