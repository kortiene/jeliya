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
import { isIP } from "node:net";
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

const TRUSTED_GRADLE_DISTRIBUTION = Object.freeze({
  url: "https\\://services.gradle.org/distributions/gradle-8.14-bin.zip",
  sha256: "61ad310d3c7d3e5da131b76bbf22b5a4c0786e9d892dae8c1658d4b484de3caa",
});

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

export function validateBuildToolIntegrity({ root = repoRoot } = {}) {
  const properties = readText("app/android/gradle/wrapper/gradle-wrapper.properties", root);
  const property = (name) => {
    const matches = [...properties.matchAll(new RegExp(`^${name}=(.+)$`, "gm"))];
    if (matches.length !== 1) fail(`Gradle wrapper must contain exactly one ${name}`);
    return matches[0][1].trim();
  };
  const distributionUrl = property("distributionUrl");
  const distributionSha256 = property("distributionSha256Sum");
  if (distributionUrl !== TRUSTED_GRADLE_DISTRIBUTION.url
      || distributionSha256 !== TRUSTED_GRADLE_DISTRIBUTION.sha256) {
    fail("Gradle wrapper distribution URL and SHA-256 are not the reviewed release pair");
  }
  return { distributionUrl, distributionSha256 };
}

export function validateSourceVersions({ root = repoRoot, tag = "" } = {}) {
  validateBuildToolIntegrity({ root });
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

  const releaseContext = context ?? releaseEvidenceContext(root, candidateCommit);
  if (!/^[0-9a-f]{64}$/.test(releaseContext.candidatePackageLockSha256 ?? "")) {
    fail("network-qualified UI lockfile digest is unavailable");
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
  for (const [path, manifest] of Object.entries(manifests)) {
    if (manifest.build.embedded_ui.package_lock_sha256
        !== releaseContext.candidatePackageLockSha256) {
      fail(`${path} evidence UI lockfile digest does not match the network-qualified commit`);
    }
  }

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

function publicGitHubGitUrl(value) {
  return strictPublicGitHubUrl(value)
    || /^git@github\.com:[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+(?:\.git)?$/.test(value ?? "");
}

function localFileGitUrl(value) {
  try {
    const url = new URL(value);
    return url.protocol === "file:" && !url.username && !url.password && !url.search && !url.hash;
  } catch {
    return false;
  }
}

const SANITIZED_LOG_POLICY = "raw daemon logs are transient in run-owned data directories and removed after successful cleanup; retained log summaries store only per-stream line/byte counts and SHA-256 digests";
const LOCAL_CHECKOUT_SCHEMA = Symbol("local-checkout-schema");

const CERTIFYING_NETWORK_SCHEMA = {
  schema: null,
  run_id: null,
  started_at_utc: null,
  ended_at_utc: null,
  result: null,
  certifiable: null,
  expected_path: null,
  mode: null,
  source: {
    commit: null,
    dirty: null,
    origin: null,
    public_source: null,
    published_at_origin: null,
    iroh_rooms_revision: null,
    iroh_rooms: {
      kind: null,
      source: null,
      requested_revision: null,
      resolved_revision: null,
      public_source: null,
      local_checkout: LOCAL_CHECKOUT_SCHEMA,
      releaseable: null,
      published_at_origin: null,
    },
    releaseable: null,
  },
  build: {
    mode: null,
    source_bound: null,
    source_snapshot_commit: null,
    locked: null,
    embedded_ui: {
      built_from_source: null,
      package_lock_sha256: null,
    },
    features: [],
    targets: [],
    commands: [],
    toolchain: {
      rustc: { filename: null, version: null, sha256: null },
      cargo: { filename: null, version: null, sha256: null },
      node: { filename: null, version: null, sha256: null },
      npm: { filename: null, version: null, sha256: null },
      zig: {
        filename: null,
        version: null,
        sha256: null,
        expected_sha256: null,
        integrity_verified: null,
      },
      cargo_zigbuild: { filename: null, version: null, sha256: null },
      cargo_build_jobs: null,
      installed_cross_target: null,
    },
  },
  distinct_public_egress: {
    evaluated: null,
    pairwise: {
      operator_to_b: { status: null, family: null },
      operator_to_c: { status: null, family: null },
      b_to_c: { status: null, family: null },
    },
    all_observed_addresses_different: null,
    autonomous_systems: {
      operator: null,
      role_b: null,
      role_c: null,
    },
    distinct_autonomous_system_count: null,
    independent_network_topology_proven: null,
    claim: null,
  },
  assertions: [],
  path_observations: {
    a: { expected_identities: null, consecutive_observations: null, expected_path: null },
    b: { expected_identities: null, consecutive_observations: null, expected_path: null },
    c: { expected_identities: null, consecutive_observations: null, expected_path: null },
  },
  functional_evidence: {
    file: {
      bytes_expected: null,
      bytes_actual: null,
      engine_verified: null,
      expected_sha256: null,
      actual_sha256: null,
      sha256_equal: null,
    },
    pipe: {
      http_status: null,
      bytes: null,
      target_connections: null,
      target_requests: null,
      unauthorized_third_peer: {
        local_forwarder_created: null,
        response_received: null,
        target_connections: null,
        target_requests: null,
      },
    },
    reconnect: {
      session_closed: null,
      message_authored_while_closed: null,
      offline_message_resynchronized: null,
      settled_path: {
        expected_identities: null,
        consecutive_observations: null,
        expected_path: null,
      },
    },
    foreign_room_non_disclosure: {
      rpc_methods_denied: [],
      local_file_http_denied: null,
      aggregate_reads_filtered: null,
      foreign_agent_projection_exercised: null,
      foreign_agent_join_attempts: null,
      synchronization_isolation_claimed: null,
    },
    multi_peer: { peers: null, convergence_verified: null },
  },
  cleanup: {
    completed: null,
    processes_stopped: null,
    temporary_artifacts_removed: null,
    failure_codes: [],
  },
  binaries: {
    local: {
      filename: null,
      sha256: null,
      version: null,
      relay_only_attested: null,
    },
    remote: {
      filename: null,
      sha256: null,
      expected_version: null,
      expected_relay_only: null,
      execution_validation: null,
    },
  },
  hosts: [],
  sanitized_logs: {
    policy: null,
    roles: [],
  },
};

const ASSERTION_SCHEMA = { name: null, result: null, duration_ms: null };
const OPERATOR_HOST_SCHEMA = {
  role: null,
  host: null,
  os: null,
  architecture: null,
};
const REMOTE_HOST_SCHEMA = {
  role: null,
  host: null,
  os: null,
  architecture: null,
  process_privilege: null,
  binary_validation: {
    sha256: null,
    version: null,
    execution_uid: null,
    relay_only_attested: null,
    relay_attestation_exit_status: null,
  },
};
const LOG_ROLE_SCHEMA = {
  role: null,
  transport: null,
  streams: {
    stdout: { lines: null, bytes: null, sha256: null },
    stderr: { lines: null, bytes: null, sha256: null },
  },
};

function closedSchema(value, schema, path) {
  if (schema === LOCAL_CHECKOUT_SCHEMA) {
    if (value === null) return;
    closedSchema(value, { commit: null, dirty: null, origin: null }, path);
    return;
  }
  if (Array.isArray(schema)) {
    if (!Array.isArray(value)) fail(`${path} must be an array`);
    return;
  }
  if (schema === null) {
    if (value !== null && typeof value === "object") {
      fail(`${path} must be a scalar or null`);
    }
    return;
  }
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail(`${path} must be an object`);
  }
  const expected = Object.keys(schema);
  const actual = Object.keys(value);
  const missing = expected.filter((key) => !Object.hasOwn(value, key));
  const unknown = actual.filter((key) => !Object.hasOwn(schema, key));
  if (missing.length > 0 || unknown.length > 0) {
    const details = [
      missing.length > 0 ? `missing ${missing.join(", ")}` : "",
      unknown.length > 0 ? `unknown ${unknown.join(", ")}` : "",
    ].filter(Boolean).join("; ");
    fail(`${path} violates the closed evidence schema: ${details}`);
  }
  for (const key of expected) closedSchema(value[key], schema[key], `${path}.${key}`);
}

function secretBearingEvidenceKey(key) {
  const normalized = key.toLowerCase().replace(/[^a-z0-9]+/g, "_");
  if (normalized === "excerpt_limit_characters") return true;
  if (/(?:^|_)(?:raw_logs?|raw_excerpt|redacted_excerpt|excerpt_truncated|log_excerpt|excerpts?|stdout_text|stderr_text)(?:_|$)/.test(normalized)) {
    return true;
  }
  return /(?:^|_)(?:token|auth_token|authorization|bearer_token|access_token|refresh_token|api_key|cookie|credential|credentials|secret|secrets|seed|identity_seed|private_key|password|room_ticket|invite_ticket|ticket)(?:_|$)/.test(normalized)
    || /(?:_token|_secret|_password|_private_key|_seed|_ticket)$/.test(normalized);
}

function containsIpLiteral(value) {
  for (const match of value.matchAll(/(?:^|[^\d.])((?:\d{1,3}\.){3}\d{1,3})(?=$|[^\d.])/g)) {
    if (isIP(match[1]) === 4) return true;
  }
  return (value.match(/[A-Fa-f0-9:.%]+/g) ?? []).some((candidate) => {
    const withoutZone = candidate.replace(/%.+$/, "");
    return isIP(withoutZone) === 6 || isIP(withoutZone.replace(/\.$/, "")) === 6;
  });
}

function rejectSecretBearingEvidence(value, path) {
  if (typeof value === "string") {
    if (/-----BEGIN [A-Z0-9][A-Z0-9 ]*-----/.test(value)) {
      fail(`${path} contains forbidden PEM material`);
    }
    if (/\bBearer\s+[A-Za-z0-9._~+/=-]+/i.test(value)) {
      fail(`${path} contains forbidden bearer material`);
    }
    if (containsIpLiteral(value)) {
      fail(`${path} contains a forbidden literal IP address`);
    }
    return;
  }
  if (value === null || typeof value !== "object") return;
  if (Array.isArray(value)) {
    value.forEach((entry, index) => rejectSecretBearingEvidence(entry, `${path}[${index}]`));
    return;
  }
  for (const [key, entry] of Object.entries(value)) {
    if (secretBearingEvidenceKey(key)) {
      fail(`${path}.${key} is a forbidden secret-bearing or log-excerpt field`);
    }
    rejectSecretBearingEvidence(entry, `${path}.${key}`);
  }
}

function validateClosedEvidenceSchema(manifest, relativePath) {
  closedSchema(manifest, CERTIFYING_NETWORK_SCHEMA, relativePath);
  for (const [index, assertion] of manifest.assertions.entries()) {
    closedSchema(assertion, ASSERTION_SCHEMA, `${relativePath}.assertions[${index}]`);
  }
  if (manifest.hosts.length !== 3) {
    fail(`${relativePath}.hosts must contain exactly roles a, b, and c`);
  }
  closedSchema(manifest.hosts[0], OPERATOR_HOST_SCHEMA, `${relativePath}.hosts[0]`);
  closedSchema(manifest.hosts[1], REMOTE_HOST_SCHEMA, `${relativePath}.hosts[1]`);
  closedSchema(manifest.hosts[2], REMOTE_HOST_SCHEMA, `${relativePath}.hosts[2]`);
  if (manifest.sanitized_logs.roles.length !== 3) {
    fail(`${relativePath}.sanitized_logs.roles must contain exactly roles a, b, and c`);
  }
  for (const [index, role] of manifest.sanitized_logs.roles.entries()) {
    closedSchema(role, LOG_ROLE_SCHEMA, `${relativePath}.sanitized_logs.roles[${index}]`);
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
  const candidatePackageLock = execFileSync(
    "git",
    ["show", `${candidateCommit}:ui/package-lock.json`],
    { cwd: root },
  );
  return {
    headCommit: head,
    candidatePackageLockSha256: createHash("sha256")
      .update(candidatePackageLock)
      .digest("hex"),
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
      || new Date(startedAt).toISOString() !== manifest?.started_at_utc
      || new Date(endedAt).toISOString() !== manifest?.ended_at_utc
      || endedAt <= startedAt) {
    fail(`${relativePath} lacks a valid run ID and bounded UTC evidence window`);
  }
  rejectSecretBearingEvidence(manifest, relativePath);
  validateClosedEvidenceSchema(manifest, relativePath);
  if (manifest?.schema !== 1
      || manifest.result !== "pass"
      || typeof manifest.certifiable !== "boolean") {
    fail(`${relativePath} is not a passing certifiable schema-1 run`);
  }
  const certifying = manifest.certifiable === true;
  if (manifest.expected_path !== expectedPath) {
    fail(`${relativePath} expected_path is not ${expectedPath}`);
  }
  const expectedMode = expectedPath === "relay" ? "remote-relay-only-build" : "remote-real-network";
  if (manifest.mode !== expectedMode) {
    fail(`${relativePath} mode is not ${expectedMode}`);
  }
  if (certifying) {
    if (manifest.source?.commit !== candidateCommit
        || manifest.source?.dirty !== false
        || !strictPublicGitHubUrl(manifest.source?.origin)
        || manifest.source?.public_source !== true
        || manifest.source?.published_at_origin !== true
        || manifest.source?.releaseable !== true) {
      fail(`${relativePath} does not bind to the releaseable network-qualified commit`);
    }
    if (manifest.source?.iroh_rooms_revision !== upstreamRevision
        || manifest.source?.iroh_rooms?.kind !== "git"
        || !strictPublicGitHubUrl(manifest.source?.iroh_rooms?.source)
        || manifest.source?.iroh_rooms?.requested_revision !== upstreamRevision
        || manifest.source?.iroh_rooms?.resolved_revision !== upstreamRevision
        || manifest.source?.iroh_rooms?.public_source !== true
        || manifest.source?.iroh_rooms?.local_checkout !== null
        || manifest.source?.iroh_rooms?.published_at_origin !== true
        || manifest.source?.iroh_rooms?.releaseable !== true) {
      fail(`${relativePath} does not bind to the releaseable upstream revision`);
    }
  } else {
    const localCheckout = manifest.source?.iroh_rooms?.local_checkout;
    if (manifest.source?.commit !== candidateCommit
        || manifest.source?.dirty !== false
        || !publicGitHubGitUrl(manifest.source?.origin)
        || manifest.source?.public_source !== true
        || manifest.source?.published_at_origin !== false
        || manifest.source?.releaseable !== false
        || manifest.source?.iroh_rooms_revision !== upstreamRevision
        || manifest.source?.iroh_rooms?.kind !== "local-git-url"
        || !localFileGitUrl(manifest.source?.iroh_rooms?.source)
        || manifest.source?.iroh_rooms?.requested_revision !== upstreamRevision
        || manifest.source?.iroh_rooms?.resolved_revision !== upstreamRevision
        || manifest.source?.iroh_rooms?.public_source !== false
        || localCheckout?.commit !== upstreamRevision
        || localCheckout?.dirty !== false
        || !publicGitHubGitUrl(localCheckout?.origin)
        || manifest.source?.iroh_rooms?.published_at_origin !== false
        || manifest.source?.iroh_rooms?.releaseable !== false) {
      fail(`${relativePath} does not bind to the retained unpublished source and upstream checkout`);
    }
  }
  const expectedFeatures = expectedPath === "relay"
    ? ["embed-ui", "relay-only-test"]
    : ["embed-ui"];
  const featureArgument = expectedFeatures.join(",");
  const expectedBuildCommands = [
    `git archive ${candidateCommit}`,
    "npm ci",
    "npm run build",
    `cargo +1.91.0 build --locked --release -p jeliyad --features ${featureArgument}`,
    `cargo +1.91.0 zigbuild --locked --release -p jeliyad --features ${featureArgument} --target x86_64-unknown-linux-musl`,
  ];
  if (manifest.build?.mode !== "from-source"
      || manifest.build?.source_bound !== true
      || manifest.build?.source_snapshot_commit !== candidateCommit
      || manifest.build?.locked !== true
      || manifest.build?.embedded_ui?.built_from_source !== true
      || !/^[0-9a-f]{64}$/.test(manifest.build?.embedded_ui?.package_lock_sha256 ?? "")
      || JSON.stringify(manifest.build?.features) !== JSON.stringify(expectedFeatures)
      || JSON.stringify(manifest.build?.targets)
        !== JSON.stringify(["x86_64-apple-darwin", "x86_64-unknown-linux-musl"])
      || JSON.stringify(manifest.build?.commands) !== JSON.stringify(expectedBuildCommands)) {
    fail(`${relativePath} was not built source-bound with the lockfile`);
  }
  const toolchain = manifest.build.toolchain;
  const toolDigestValid = (tool) => /^[0-9a-f]{64}$/.test(tool?.sha256 ?? "");
  const expectedRustcVersion = [
    "rustc 1.91.0 (f8297e351 2025-10-28)",
    "binary: rustc",
    "commit-hash: f8297e351a40c1439a467bbbb6879088047f50b3",
    "commit-date: 2025-10-28",
    "host: x86_64-apple-darwin",
    "release: 1.91.0",
    "LLVM version: 21.1.2",
  ].join("\n");
  if (toolchain.rustc.filename !== "rustc"
      || toolchain.rustc.version !== expectedRustcVersion
      || !toolDigestValid(toolchain.rustc)
      || toolchain.cargo.filename !== "cargo"
      || toolchain.cargo.version !== "cargo 1.91.0 (ea2d97820 2025-10-10)"
      || !toolDigestValid(toolchain.cargo)
      || toolchain.node.filename !== "node"
      || toolchain.node.version !== "v22.22.3"
      || !toolDigestValid(toolchain.node)
      || toolchain.npm.filename !== "npm"
      || toolchain.npm.version !== "10.9.8"
      || !toolDigestValid(toolchain.npm)
      || toolchain.zig.filename !== "zig"
      || toolchain.zig.version !== "0.15.2"
      || !toolDigestValid(toolchain.zig)
      || toolchain.zig.sha256 !== toolchain.zig.expected_sha256
      || toolchain.zig.integrity_verified !== true
      || toolchain.cargo_zigbuild.filename !== "cargo-zigbuild"
      || toolchain.cargo_zigbuild.version !== "cargo-zigbuild 0.23.0"
      || !toolDigestValid(toolchain.cargo_zigbuild)
      || toolchain.cargo_build_jobs !== 2
      || toolchain.installed_cross_target !== "x86_64-unknown-linux-musl") {
    fail(`${relativePath} lacks the exact verified release toolchain`);
  }
  const topology = manifest.distinct_public_egress;
  const pairwise = topology?.pairwise;
  const asns = topology?.autonomous_systems;
  if (topology?.evaluated !== true
      || topology?.all_observed_addresses_different !== true
      || topology?.independent_network_topology_proven !== true
      || topology?.claim
        !== "distinct public egress plus at least two independently resolved BGP origin ASNs; no IP address is persisted"
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
        || !Number.isInteger(assertion.duration_ms)
        || assertion.duration_ms < 0)
      || JSON.stringify(assertions.map((assertion) => assertion.name))
        !== JSON.stringify(expectedAssertions)) {
    fail(`${relativePath} does not contain a complete all-pass assertion set`);
  }
  const paths = manifest.path_observations;
  for (const role of ["a", "b", "c"]) {
    const observation = paths?.[role];
    if (observation?.expected_path !== expectedPath
        || !Number.isInteger(observation?.consecutive_observations)
        || observation.consecutive_observations < 3
        || !Number.isInteger(observation?.expected_identities)
        || observation.expected_identities !== (role === "a" ? 2 : 1)) {
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
  if (!Number.isInteger(functional?.file?.bytes_expected)
      || functional.file.bytes_expected < 1
      || functional.file.bytes_actual !== functional.file.bytes_expected
      || functional?.file?.engine_verified !== true
      || functional?.file?.sha256_equal !== true
      || !/^[0-9a-f]{64}$/.test(functional?.file?.expected_sha256 ?? "")
      || functional?.file?.expected_sha256 !== functional?.file?.actual_sha256
      || functional?.pipe?.http_status !== 200
      || !Number.isInteger(functional?.pipe?.bytes)
      || functional.pipe.bytes < 1
      || !Number.isInteger(functional?.pipe?.target_connections)
      || functional.pipe.target_connections < 1
      || !Number.isInteger(functional?.pipe?.target_requests)
      || functional.pipe.target_requests < 1
      || functional?.pipe?.unauthorized_third_peer?.local_forwarder_created !== true
      || functional?.pipe?.unauthorized_third_peer?.response_received !== false
      || functional?.pipe?.unauthorized_third_peer?.target_connections !== 0
      || functional?.pipe?.unauthorized_third_peer?.target_requests !== 0
      || functional?.reconnect?.session_closed !== true
      || functional?.reconnect?.message_authored_while_closed !== true
      || functional?.reconnect?.offline_message_resynchronized !== true
      || functional?.reconnect?.settled_path?.expected_path !== expectedPath
      || functional?.reconnect?.settled_path?.expected_identities !== 1
      || !Number.isInteger(functional?.reconnect?.settled_path?.consecutive_observations)
      || functional.reconnect.settled_path.consecutive_observations < 3
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
  const hostRoles = manifest.hosts.map((host) => host.role);
  const operatorHost = manifest.hosts[0];
  const remoteHosts = manifest.hosts.slice(1);
  const remoteDigest = manifest.binaries?.remote?.sha256;
  const expectedRelayOnly = expectedPath === "relay";
  const expectedRelayAttestationStatus = expectedRelayOnly ? 0 : 2;
  if (JSON.stringify(hostRoles) !== JSON.stringify(["a", "b", "c"])
      || operatorHost.host !== "operator-local"
      || operatorHost.os !== "darwin"
      || operatorHost.architecture !== "x64"
      || new Set(remoteHosts.map((host) => host.host)).size !== 2
      || !/^[0-9a-f]{64}$/.test(remoteDigest ?? "")
      || remoteHosts.some((host) => !/^[A-Za-z0-9][A-Za-z0-9._@-]*$/.test(host.host)
        || host.architecture !== "x86_64"
        || !/^Ubuntu [1-9]\d*\.\d+(?:\.\d+)?(?: LTS)?$/.test(host.os)
        || !["dropped-to-unprivileged-system-uid", "ssh-account-uid"].includes(host.process_privilege)
        || host.binary_validation?.sha256 !== remoteDigest
        || host.binary_validation?.version !== expectedVersion
        || !/^[1-9]\d*$/.test(host.binary_validation?.execution_uid ?? "")
        || host.binary_validation?.relay_only_attested !== expectedRelayOnly
        || host.binary_validation?.relay_attestation_exit_status
          !== expectedRelayAttestationStatus)) {
    fail(`${relativePath} lacks the exact operator and two independently verified remote environments`);
  }
  const logRoles = manifest.sanitized_logs?.roles;
  const emptyStreamSha256 = createHash("sha256").update("").digest("hex");
  if (manifest.sanitized_logs?.policy !== SANITIZED_LOG_POLICY
      || JSON.stringify(logRoles.map((role) => role.role)) !== JSON.stringify(["a", "b", "c"])
      || logRoles.some((role, index) => role.transport !== (index === 0 ? "local-child" : "supervised-ssh")
        || ["stdout", "stderr"].some((stream) => {
        const record = role.streams?.[stream];
        return !Number.isInteger(record?.lines)
          || record.lines < 0
          || !Number.isInteger(record?.bytes)
          || record.bytes < 0
          || !/^[0-9a-f]{64}$/.test(record?.sha256 ?? "")
          || (record.bytes === 0
            && (record.lines !== 0 || record.sha256 !== emptyStreamSha256))
          || (record.bytes > 0 && (record.lines < 1 || record.lines > record.bytes));
      }))) {
    fail(`${relativePath} lacks sanitized per-role log integrity records`);
  }
  const relayAttested = manifest.binaries?.local?.relay_only_attested === true
    && remoteHosts.every((host) => host.binary_validation?.relay_only_attested === true);
  if (expectedPath === "relay" && !relayAttested) {
    fail(`${relativePath} lacks compile-time relay-only attestation on every execution host`);
  }
  if (manifest.binaries?.local?.filename !== "jeliyad"
      || !/^[0-9a-f]{64}$/.test(manifest.binaries?.local?.sha256 ?? "")
      || manifest.binaries?.local?.version !== expectedVersion
      || manifest.binaries?.remote?.filename !== "jeliyad"
      || manifest.binaries?.remote?.expected_version !== expectedVersion
      || manifest.binaries?.remote?.expected_relay_only !== expectedRelayOnly
      || manifest.binaries?.remote?.execution_validation
        !== "verified independently on every Linux execution host after transfer"
      || (expectedPath === "direct" && (
        manifest.binaries.local.relay_only_attested !== false
        || remoteHosts.some((host) => host.binary_validation?.relay_only_attested !== false)
      ))) {
    fail(`${relativePath} binary versions or direct-build attestations are inconsistent`);
  }
  if (!certifying) {
    fail(`${relativePath} is not a passing certifiable schema-1 run`);
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
