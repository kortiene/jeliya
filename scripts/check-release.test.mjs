import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  expectedNetworkAssertionNames,
  expectedArtifactNames,
  irohRoomsReleaseIdentity,
  validateArtifactSet,
  validateBuildToolIntegrity,
  validateEvidenceReadiness,
  validateNetworkEvidenceManifest,
  validateSourceVersions,
} from "./check-release.mjs";
import {
  SOURCE_BUILD_ALLOWED_AMBIENT_NAMES,
  SOURCE_BUILD_ENVIRONMENT_POLICY,
} from "./realnet-evidence.mjs";

const ciWorkflow = readFileSync(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8");
const releaseWorkflow = readFileSync(
  new URL("../.github/workflows/release.yml", import.meta.url),
  "utf8",
);
const releaseFinalizer = readFileSync(
  new URL("./finalize-release.sh", import.meta.url),
  "utf8",
);

test("checked-in release surfaces share one dated version", () => {
  const result = validateSourceVersions();
  assert.equal(result.tag, `v${result.version}`);
  assert.equal(new Set(Object.values(result.versions)).size, 1);
});

test("Android build tools are pinned and verified before execution", () => {
  const result = validateBuildToolIntegrity();
  assert.equal(
    result.distributionSha256,
    "61ad310d3c7d3e5da131b76bbf22b5a4c0786e9d892dae8c1658d4b484de3caa",
  );

  const root = mkdtempSync(join(tmpdir(), "jeliya-gradle-integrity-"));
  try {
    const wrapper = join(root, "app", "android", "gradle", "wrapper");
    mkdirSync(wrapper, { recursive: true });
    writeFileSync(join(wrapper, "gradle-wrapper.properties"), [
      "distributionUrl=https\\://services.gradle.org/distributions/gradle-8.14-bin.zip",
      `distributionSha256Sum=${"00".repeat(32)}`,
      "",
    ].join("\n"));
    assert.throws(
      () => validateBuildToolIntegrity({ root }),
      /URL and SHA-256 are not the reviewed release pair/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }

  const setupJava = ciWorkflow.indexOf(
    "actions/setup-java@be666c2fcd27ec809703dec50e508c2fdc7f6654",
  );
  const download = ciWorkflow.indexOf("https://services.gradle.org/distributions/$archive");
  const verify = ciWorkflow.indexOf("sha256sum -c -", download);
  const extract = ciWorkflow.indexOf('unzip -q "$RUNNER_TEMP/$archive"', verify);
  const execute = ciWorkflow.indexOf("gradle -p android", extract);
  assert.ok(setupJava >= 0);
  assert.ok(download > setupJava);
  assert.ok(verify > download);
  assert.ok(extract > verify);
  assert.ok(execute > extract);
  assert.doesNotMatch(ciWorkflow, /\.\/android\/gradlew -p android/);
});

function networkManifest(path, commit, upstream) {
  const relay = path === "relay";
  const digest = "ef".repeat(32);
  const toolDigest = "12".repeat(32);
  const features = relay ? ["embed-ui", "relay-only-test"] : ["embed-ui"];
  const featureArgument = features.join(",");
  const deniedMethods = [
    "room.open", "room.close", "room.leave", "room.timeline", "room.members",
    "invite.create", "message.send", "status.post", "file.share", "file.list",
    "file.fetch", "pipe.expose", "pipe.list", "pipe.connect", "pipe.close",
    "peers.status", "agent.history",
  ];
  return {
    schema: 2,
    run_id: `20260712T12000${relay ? "1" : "0"}Z-0123abcd`,
    started_at_utc: "2026-07-12T12:00:00.000Z",
    ended_at_utc: "2026-07-12T12:10:00.000Z",
    result: "pass",
    certifiable: true,
    expected_path: path,
    mode: relay ? "remote-relay-only-build" : "remote-real-network",
    source: {
      commit,
      dirty: false,
      origin: "https://github.com/kortiene/jeliya.git",
      public_source: true,
      published_at_origin: true,
      releaseable: true,
      iroh_rooms_revision: upstream,
      iroh_rooms: {
        kind: "git",
        source: "https://github.com/kortiene/iroh-room",
        requested_revision: upstream,
        resolved_revision: upstream,
        public_source: true,
        local_checkout: null,
        published_at_origin: true,
        releaseable: true,
      },
    },
    build: {
      mode: "from-source",
      source_bound: true,
      source_snapshot_commit: commit,
      locked: true,
      environment: {
        policy: SOURCE_BUILD_ENVIRONMENT_POLICY,
        allowed_names: [...SOURCE_BUILD_ALLOWED_AMBIENT_NAMES],
        inherited_names: [],
        isolated_home: true,
        isolated_cargo_home: true,
        isolated_temp: true,
        controlled_path: true,
        ambient_build_controls_rejected: true,
        unlisted_ambient_removed: true,
      },
      embedded_ui: {
        built_from_source: true,
        package_lock_sha256: "ab".repeat(32),
      },
      features,
      targets: ["x86_64-apple-darwin", "x86_64-unknown-linux-musl"],
      commands: [
        "git clone --bare --no-local <candidate repository>",
        `git archive ${commit}`,
        "node <recorded npm-cli> ci",
        "node <recorded npm-cli> run build",
        `cargo build --locked --release -p jeliyad --features ${featureArgument}`,
        `cargo-zigbuild zigbuild --locked --release -p jeliyad --features ${featureArgument} --target x86_64-unknown-linux-musl`,
      ],
      toolchain: {
        identity_policy: "tools execute by resolved absolute path; evidence records filename, version, and observed SHA-256; the complete Zig installation archive is independently digest-verified",
        independently_verified: ["zig-installation-archive"],
        execution_binding: "npm is executed by the recorded Node binary; cargo-zigbuild is invoked directly with recorded Cargo and Zig paths; Python ziglang discovery is disabled",
        rustc: {
          filename: "rustc",
          version: [
            "rustc 1.91.0 (f8297e351 2025-10-28)",
            "binary: rustc",
            "commit-hash: f8297e351a40c1439a467bbbb6879088047f50b3",
            "commit-date: 2025-10-28",
            "host: x86_64-apple-darwin",
            "release: 1.91.0",
            "LLVM version: 21.1.2",
          ].join("\n"),
          sha256: toolDigest,
        },
        cargo: {
          filename: "cargo",
          version: "cargo 1.91.0 (ea2d97820 2025-10-10)",
          sha256: toolDigest,
        },
        rustup: {
          filename: "rustup",
          version: "rustup 1.28.2 (e4f3ad6f8 2025-04-28)",
          sha256: toolDigest,
        },
        node: { filename: "node", version: "v22.22.3", sha256: toolDigest },
        npm: { filename: "npm", version: "10.9.8", sha256: toolDigest },
        zig: {
          filename: "zig",
          version: "0.15.2",
          sha256: toolDigest,
          archive_sha256: "375b6909fc1495d16fc2c7db9538f707456bfc3373b14ee83fdd3e22b3d43f7f",
          expected_archive_sha256: "375b6909fc1495d16fc2c7db9538f707456bfc3373b14ee83fdd3e22b3d43f7f",
          archive_integrity_verified: true,
          installation_root_bound: true,
          lib_dir_bound: true,
        },
        cargo_zigbuild: {
          filename: "cargo-zigbuild",
          version: "cargo-zigbuild 0.23.0",
          sha256: toolDigest,
        },
        git: { filename: "git", version: "git version 2.50.1", sha256: toolDigest },
        tar: { filename: "tar", version: "bsdtar 3.7.7", sha256: toolDigest },
        cargo_build_jobs: 2,
        installed_cross_target: "x86_64-unknown-linux-musl",
      },
    },
    distinct_public_egress: {
      evaluated: true,
      all_observed_addresses_different: true,
      distinct_autonomous_system_count: 2,
      independent_network_topology_proven: true,
      pairwise: {
        operator_to_b: { status: "different", family: "ipv4" },
        operator_to_c: { status: "different", family: "ipv4" },
        b_to_c: { status: "different", family: "ipv4" },
      },
      autonomous_systems: {
        operator: "AS11426",
        role_b: "AS24940",
        role_c: "AS24940",
      },
      claim: "distinct public egress plus at least two independently resolved BGP origin ASNs; no IP address is persisted",
    },
    assertions: expectedNetworkAssertionNames(path)
      .map((name) => ({ name, result: "pass", duration_ms: 1 })),
    path_observations: Object.fromEntries(["a", "b", "c"].map((role) => [role, {
      expected_path: path,
      expected_identities: role === "a" ? 2 : 1,
      consecutive_observations: 3,
    }])),
    functional_evidence: {
      file: {
        bytes_expected: 262184,
        bytes_actual: 262184,
        engine_verified: true,
        sha256_equal: true,
        expected_sha256: digest,
        actual_sha256: digest,
      },
      pipe: {
        http_status: 200,
        bytes: 30,
        target_connections: 2,
        target_requests: 1,
        unauthorized_third_peer: {
          local_forwarder_created: true,
          response_received: false,
          target_connections: 0,
          target_requests: 0,
        },
      },
      reconnect: {
        session_closed: true,
        message_authored_while_closed: true,
        offline_message_resynchronized: true,
        settled_path: {
          expected_identities: 1,
          consecutive_observations: 3,
          expected_path: path,
        },
      },
      multi_peer: { peers: 3, convergence_verified: true },
      foreign_room_non_disclosure: {
        rpc_methods_denied: deniedMethods,
        local_file_http_denied: true,
        aggregate_reads_filtered: true,
        foreign_agent_projection_exercised: true,
        foreign_agent_join_attempts: 2,
        synchronization_isolation_claimed: false,
      },
    },
    cleanup: {
      completed: true,
      processes_stopped: true,
      temporary_artifacts_removed: true,
      failure_codes: [],
    },
    binaries: {
      local: {
        filename: "jeliyad",
        sha256: digest,
        version: "0.5.0",
        relay_only_attested: relay,
      },
      remote: {
        filename: "jeliyad",
        sha256: digest,
        expected_version: "0.5.0",
        expected_relay_only: relay,
        execution_validation: "verified independently on every Linux execution host after transfer",
      },
    },
    hosts: [
      { role: "a", host: "operator-local", os: "darwin", architecture: "x64" },
      ...["b", "c"].map((role) => ({
        role,
        host: role === "b" ? "root@demo1" : "root@demo2",
        os: "Ubuntu 22.04.5 LTS",
        architecture: "x86_64",
        process_privilege: "dropped-to-unprivileged-system-uid",
        binary_validation: {
          sha256: digest,
          version: "0.5.0",
          execution_uid: "65534",
          relay_only_attested: relay,
          relay_attestation_exit_status: relay ? 0 : 2,
        },
      })),
    ],
    sanitized_logs: {
      policy: "raw daemon logs are transient in run-owned data directories and removed after successful cleanup; retained log summaries store only per-stream line/byte counts and SHA-256 digests",
      roles: ["a", "b", "c"].map((role) => ({
        role,
        transport: role === "a" ? "local-child" : "supervised-ssh",
        streams: Object.fromEntries(["stdout", "stderr"].map((stream) => [stream, {
          lines: 1,
          bytes: 1,
          sha256: digest,
        }])),
      })),
    },
  };
}

test("certifying network evidence has a closed, secret-free schema", () => {
  const commit = "ab".repeat(20);
  const upstream = "cd".repeat(20);
  const validate = (manifest, path = "direct") => validateNetworkEvidenceManifest(manifest, {
    expectedPath: path,
    candidateCommit: commit,
    upstreamRevision: upstream,
  });

  assert.deepEqual(validate(networkManifest("direct", commit, upstream)), { valid: true });
  assert.deepEqual(validate(networkManifest("relay", commit, upstream), "relay"), { valid: true });

  const negativeCases = [
    {
      name: "top-level auth token",
      mutate: (manifest) => { manifest.auth_token = "opaque-fixture"; },
      error: /forbidden secret-bearing/,
    },
    {
      name: "nested auth token",
      mutate: (manifest) => { manifest.source.iroh_rooms.auth_token = "opaque-fixture"; },
      error: /forbidden secret-bearing/,
    },
    {
      name: "nested raw logs",
      mutate: (manifest) => { manifest.sanitized_logs.raw_logs = []; },
      error: /forbidden secret-bearing or log-excerpt field/,
    },
    {
      name: "unknown deep field",
      mutate: (manifest) => { manifest.functional_evidence.file.unexpected = true; },
      error: /closed evidence schema: unknown unexpected/,
    },
    {
      name: "missing toolchain",
      mutate: (manifest) => { delete manifest.build.toolchain; },
      error: /closed evidence schema: missing toolchain/,
    },
    {
      name: "missing embedded UI provenance",
      mutate: (manifest) => { delete manifest.build.embedded_ui; },
      error: /closed evidence schema: missing embedded_ui/,
    },
    {
      name: "legacy schema cannot certify",
      mutate: (manifest) => {
        manifest.schema = 1;
        delete manifest.build.environment;
        for (const name of [
          "identity_policy",
          "independently_verified",
          "execution_binding",
          "rustup",
          "git",
          "tar",
        ]) {
          delete manifest.build.toolchain[name];
        }
        const zigSha256 = manifest.build.toolchain.zig.sha256;
        manifest.build.toolchain.zig = {
          filename: "zig",
          version: "0.15.2",
          sha256: zigSha256,
          expected_sha256: zigSha256,
          integrity_verified: true,
        };
      },
      error: /certifying evidence requires the isolated-build schema 2/,
    },
    {
      name: "missing isolated build environment",
      mutate: (manifest) => { delete manifest.build.environment; },
      error: /closed evidence schema: missing environment/,
    },
    {
      name: "expanded ambient allowlist",
      mutate: (manifest) => { manifest.build.environment.allowed_names.push("AWS_SECRET_ACCESS_KEY"); },
      error: /required isolated source-build environment/,
    },
    {
      name: "unapproved inherited environment name",
      mutate: (manifest) => { manifest.build.environment.inherited_names.push("GITHUB_TOKEN"); },
      error: /required isolated source-build environment/,
    },
    {
      name: "uncontrolled build path",
      mutate: (manifest) => { manifest.build.environment.controlled_path = false; },
      error: /required isolated source-build environment/,
    },
    {
      name: "non-HTTPS Jeliya provenance",
      mutate: (manifest) => { manifest.source.origin = "git@github.com:kortiene/jeliya.git"; },
      error: /releaseable network-qualified commit/,
    },
    {
      name: "non-git upstream provenance",
      mutate: (manifest) => { manifest.source.iroh_rooms.kind = "local-git-url"; },
      error: /releaseable upstream revision/,
    },
    {
      name: "non-public upstream source",
      mutate: (manifest) => { manifest.source.iroh_rooms.source = "file:///tmp/iroh-room"; },
      error: /releaseable upstream revision/,
    },
    {
      name: "local upstream checkout",
      mutate: (manifest) => { manifest.source.iroh_rooms.local_checkout = {}; },
      error: /local_checkout violates the closed evidence schema/,
    },
    {
      name: "malformed embedded UI lock digest",
      mutate: (manifest) => { manifest.build.embedded_ui.package_lock_sha256 = "00"; },
      error: /built source-bound with the lockfile/,
    },
    {
      name: "missing operator role A",
      mutate: (manifest) => { manifest.hosts.shift(); },
      error: /must contain exactly roles a, b, and c/,
    },
    {
      name: "missing deep log digest",
      mutate: (manifest) => { delete manifest.sanitized_logs.roles[0].streams.stdout.sha256; },
      error: /closed evidence schema: missing sha256/,
    },
    {
      name: "excerpt limit metadata",
      mutate: (manifest) => { manifest.sanitized_logs.excerpt_limit_characters = 1024; },
      error: /forbidden secret-bearing or log-excerpt field/,
    },
    {
      name: "redacted log excerpt",
      mutate: (manifest) => {
        manifest.sanitized_logs.roles[0].streams.stdout.redacted_excerpt = "[redacted]";
      },
      error: /forbidden secret-bearing or log-excerpt field/,
    },
    {
      name: "excerpt truncation marker",
      mutate: (manifest) => {
        manifest.sanitized_logs.roles[0].streams.stdout.excerpt_truncated = false;
      },
      error: /forbidden secret-bearing or log-excerpt field/,
    },
    {
      name: "PEM material",
      mutate: (manifest) => {
        manifest.source.origin = ["-----BEGIN", "PRIVATE", "KEY-----"].join(" ");
      },
      error: /forbidden PEM material/,
    },
    {
      name: "bearer material",
      mutate: (manifest) => { manifest.hosts[1].host = "Bearer abc.def.ghi"; },
      error: /forbidden bearer material/,
    },
    {
      name: "IPv4 literal",
      mutate: (manifest) => { manifest.hosts[1].host = "root@192.0.2.1"; },
      error: /forbidden literal IP address/,
    },
    {
      name: "IPv6 literal",
      mutate: (manifest) => { manifest.hosts[1].host = "2001:db8::1"; },
      error: /forbidden literal IP address/,
    },
    {
      name: "wrong Rust version",
      mutate: (manifest) => { manifest.build.toolchain.rustc.version = "rustc 1.92.0"; },
      error: /pinned and fully identified release toolchain/,
    },
    {
      name: "wrong Cargo version",
      mutate: (manifest) => { manifest.build.toolchain.cargo.version = "cargo 1.92.0"; },
      error: /pinned and fully identified release toolchain/,
    },
    {
      name: "wrong Node version",
      mutate: (manifest) => { manifest.build.toolchain.node.version = "v22.22.2"; },
      error: /pinned and fully identified release toolchain/,
    },
    {
      name: "wrong npm version",
      mutate: (manifest) => { manifest.build.toolchain.npm.version = "10.9.7"; },
      error: /pinned and fully identified release toolchain/,
    },
    {
      name: "wrong Zig version",
      mutate: (manifest) => { manifest.build.toolchain.zig.version = "0.15.1"; },
      error: /pinned and fully identified release toolchain/,
    },
    {
      name: "mismatched Zig archive digest",
      mutate: (manifest) => {
        manifest.build.toolchain.zig.expected_archive_sha256 = "34".repeat(32);
      },
      error: /pinned and fully identified release toolchain/,
    },
    {
      name: "malformed tool digest",
      mutate: (manifest) => { manifest.build.toolchain.cargo.sha256 = "00"; },
      error: /pinned and fully identified release toolchain/,
    },
    {
      name: "wrong cargo-zigbuild version",
      mutate: (manifest) => {
        manifest.build.toolchain.cargo_zigbuild.version = "cargo-zigbuild 0.24.0";
      },
      error: /pinned and fully identified release toolchain/,
    },
    {
      name: "missing archive tool identity",
      mutate: (manifest) => { delete manifest.build.toolchain.tar; },
      error: /closed evidence schema: missing tar/,
    },
    {
      name: "malformed git tool identity",
      mutate: (manifest) => { manifest.build.toolchain.git.sha256 = "00"; },
      error: /pinned and fully identified release toolchain/,
    },
    {
      name: "wrong build jobs",
      mutate: (manifest) => { manifest.build.toolchain.cargo_build_jobs = 3; },
      error: /pinned and fully identified release toolchain/,
    },
    {
      name: "wrong cross target",
      mutate: (manifest) => {
        manifest.build.toolchain.installed_cross_target = "aarch64-unknown-linux-musl";
      },
      error: /pinned and fully identified release toolchain/,
    },
  ];

  for (const { name, mutate, error } of negativeCases) {
    const manifest = networkManifest("direct", commit, upstream);
    mutate(manifest);
    assert.throws(() => validate(manifest), error, name);
  }
});

test("retained historical schema 1 network evidence remains explicitly non-certifying", () => {
  const verificationDoc = readFileSync(
    new URL("../docs/verification-evidence.md", import.meta.url),
    "utf8",
  );
  // The certifying direct.json/relay.json are validated by the release gate
  // (check-release --publish). The retained historical schema 1 manifests keep
  // their own filenames so this guard proves they stay honestly non-certifying.
  for (const path of ["direct", "relay"]) {
    const manifestUrl = new URL(`../docs/evidence/v0.5.0/historical-schema1-${path}.json`, import.meta.url);
    const contents = readFileSync(manifestUrl);
    const manifest = JSON.parse(contents);
    assert.equal(manifest.certifiable, false);
    assert.equal(manifest.schema, 1);
    assert.equal(manifest.result, "pass");
    assert.equal(manifest.expected_path, path);
    assert.equal(manifest.source.releaseable, false);
    assert.equal(manifest.source.published_at_origin, false);
    assert.equal(manifest.source.iroh_rooms.releaseable, false);
    assert.equal(manifest.source.iroh_rooms.published_at_origin, false);
    assert.equal(manifest.source.iroh_rooms.kind, "local-git-url");
    assert.match(manifest.source.iroh_rooms.source, /^file:\/\/\//);
    assert.equal(
      manifest.source.iroh_rooms.local_checkout.commit,
      manifest.source.iroh_rooms_revision,
    );
    assert.equal(manifest.source.iroh_rooms.local_checkout.dirty, false);
    const digest = createHash("sha256").update(contents).digest("hex");
    assert.equal(
      [...verificationDoc.matchAll(new RegExp(digest, "g"))].length,
      1,
      `${path} manifest digest must appear exactly once in the verification ledger`,
    );
    assert.throws(() => validateNetworkEvidenceManifest(manifest, {
      expectedPath: path,
      candidateCommit: manifest.source.commit,
      upstreamRevision: manifest.source.iroh_rooms_revision,
    }), /not a passing certifying run/);

    const unsafe = structuredClone(manifest);
    unsafe.sanitized_logs.raw_logs = [];
    assert.throws(() => validateNetworkEvidenceManifest(unsafe, {
      expectedPath: path,
      candidateCommit: unsafe.source.commit,
      upstreamRevision: unsafe.source.iroh_rooms_revision,
    }), /forbidden secret-bearing or log-excerpt field/);

    for (const mutate of [
      (value) => { value.sanitized_logs.policy = "unverified policy"; },
      (value) => { value.sanitized_logs.roles[0].streams.stdout.sha256 = "00"; },
      (value) => { value.sanitized_logs.roles[1].role = "a"; },
      (value) => { value.sanitized_logs.roles[2].streams.stderr.bytes = -1; },
      (value) => {
        value.sanitized_logs.roles[0].streams.stdout.bytes = 0;
        value.sanitized_logs.roles[0].streams.stdout.lines = 999;
      },
      (value) => {
        value.sanitized_logs.roles[1].streams.stdout.bytes = 2;
        value.sanitized_logs.roles[1].streams.stdout.lines = 0;
      },
      (value) => {
        value.sanitized_logs.roles[2].streams.stderr.bytes = 0;
        value.sanitized_logs.roles[2].streams.stderr.lines = 0;
        value.sanitized_logs.roles[2].streams.stderr.sha256 = "12".repeat(32);
      },
    ]) {
      const corrupted = structuredClone(manifest);
      mutate(corrupted);
      assert.throws(() => validateNetworkEvidenceManifest(corrupted, {
        expectedPath: path,
        candidateCommit: corrupted.source.commit,
        upstreamRevision: corrupted.source.iroh_rooms_revision,
      }), /lacks sanitized per-role log integrity records/);
    }
  }
});

test("publication is blocked until retained direct and relay evidence is exact", () => {
  const root = mkdtempSync(join(tmpdir(), "jeliya-evidence-gate-"));
  try {
    const commit = "ab".repeat(20);
    const upstream = "cd".repeat(20);
    mkdirSync(join(root, "docs", "evidence", "v0.5.0"), { recursive: true });
    mkdirSync(join(root, "release"));
    const { publicKey, privateKey } = generateKeyPairSync("ed25519");
    writeFileSync(
      join(root, "release", "evidence-ed25519-public.pem"),
      publicKey.export({ type: "spki", format: "pem" }),
    );
    writeFileSync(join(root, "docs", "verification-evidence.md"), `---
implementation_status: "implemented"
verification_status: "verified"
---

## Candidate identity

| Field | Value |
|---|---|
| Network-qualified commit | \`${commit}\` |
| Candidate upstream remediation revision | \`${upstream}\` |
| Release evidence gate | READY |

## Evidence ledger
`);
    const writeSignedManifest = (
      path,
      manifest = networkManifest(path, commit, upstream),
    ) => {
      const relativePath = join("docs", "evidence", "v0.5.0", `${path}.json`);
      const contents = Buffer.from(`${JSON.stringify(manifest, null, 2)}\n`);
      writeFileSync(join(root, relativePath), contents);
      writeFileSync(
        join(root, `${relativePath}.sig`),
        `${sign(null, contents, privateKey).toString("base64")}\n`,
      );
      return contents;
    };
    const directContents = writeSignedManifest("direct");
    writeSignedManifest("relay");
    const context = {
      headCommit: "12".repeat(20),
      candidatePackageLockSha256: "ab".repeat(32),
      upstreamRequestedRevision: upstream,
      upstreamResolvedRevision: upstream,
      upstreamPublic: true,
      candidateIsAncestor: true,
      changedPaths: ["docs/verification-evidence.md", "docs/evidence/v0.5.0/direct.json"],
    };
    assert.deepEqual(validateEvidenceReadiness({ root, context }), {
      ready: true,
      candidateCommit: commit,
      upstreamRevision: upstream,
    });
    assert.throws(
      () => validateEvidenceReadiness({ root, context, version: "0.6.0" }),
      /docs\/evidence\/v0\.6\.0\/direct\.json/,
    );

    const wrongUiLock = networkManifest("direct", commit, upstream);
    wrongUiLock.build.embedded_ui.package_lock_sha256 = "cd".repeat(32);
    writeSignedManifest("direct", wrongUiLock);
    assert.throws(
      () => validateEvidenceReadiness({ root, context }),
      /UI lockfile digest does not match the network-qualified commit/,
    );
    writeSignedManifest("direct");

    const relayToolDrift = networkManifest("relay", commit, upstream);
    relayToolDrift.build.toolchain.git.sha256 = "34".repeat(32);
    writeSignedManifest("relay", relayToolDrift);
    assert.throws(
      () => validateEvidenceReadiness({ root, context }),
      /not built with the same recorded toolchain/,
    );
    writeSignedManifest("relay");

    writeFileSync(
      join(root, "docs", "evidence", "v0.5.0", "direct.json"),
      Buffer.concat([directContents, Buffer.from(" ")]),
    );
    assert.throws(() => validateEvidenceReadiness({ root, context }), /invalidly signed/);
    writeFileSync(join(root, "docs", "evidence", "v0.5.0", "direct.json"), directContents);

    const relay = networkManifest("relay", commit, upstream);
    relay.hosts[1].binary_validation.relay_only_attested = false;
    assert.throws(() => validateNetworkEvidenceManifest(relay, {
      expectedPath: "relay",
      candidateCommit: commit,
      upstreamRevision: upstream,
    }), /operator and two independently verified remote environments/);

    const forged = networkManifest("direct", commit, upstream);
    forged.assertions[35] = { ...forged.assertions[0] };
    assert.throws(() => validateNetworkEvidenceManifest(forged, {
      expectedPath: "direct",
      candidateCommit: commit,
      upstreamRevision: upstream,
    }), /complete all-pass assertion set/);
    forged.assertions = networkManifest("direct", commit, upstream).assertions;
    forged.build.source_snapshot_commit = "00".repeat(20);
    assert.throws(() => validateNetworkEvidenceManifest(forged, {
      expectedPath: "direct",
      candidateCommit: commit,
      upstreamRevision: upstream,
    }), /not built source-bound/);
    const duplicatedHost = networkManifest("direct", commit, upstream);
    duplicatedHost.hosts[2].host = duplicatedHost.hosts[1].host;
    assert.throws(() => validateNetworkEvidenceManifest(duplicatedHost, {
      expectedPath: "direct",
      candidateCommit: commit,
      upstreamRevision: upstream,
    }), /operator and two independently verified remote environments/);
    const rootExecution = networkManifest("direct", commit, upstream);
    delete rootExecution.hosts[1].binary_validation.execution_uid;
    assert.throws(() => validateNetworkEvidenceManifest(rootExecution, {
      expectedPath: "direct",
      candidateCommit: commit,
      upstreamRevision: upstream,
    }), /closed evidence schema/);
    const incompleteTopology = networkManifest("direct", commit, upstream);
    delete incompleteTopology.distinct_public_egress.pairwise;
    assert.throws(() => validateNetworkEvidenceManifest(incompleteTopology, {
      expectedPath: "direct",
      candidateCommit: commit,
      upstreamRevision: upstream,
    }), /closed evidence schema: missing pairwise/);

    assert.throws(() => validateEvidenceReadiness({
      root,
      context: { ...context, changedPaths: ["crates/jeliyad/src/main.rs"] },
    }), /changed after network qualification/);
    assert.throws(() => validateEvidenceReadiness({
      root,
      context: { ...context, upstreamResolvedRevision: "00".repeat(20) },
    }), /does not match Cargo.toml and Cargo.lock/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("the checked-in iroh-rooms release identity is public and resolution-exact", () => {
  const identity = irohRoomsReleaseIdentity();
  assert.equal(identity.publicSource, true);
  assert.equal(identity.requestedRevision, identity.resolvedRevision);
  assert.match(identity.requestedRevision, /^[0-9a-f]{40}$/);
});

test("external Actions are immutable and only publish receives write authority", () => {
  for (const [name, workflow] of [
    ["ci.yml", ciWorkflow],
    ["release.yml", releaseWorkflow],
  ]) {
    for (const match of workflow.matchAll(/^\s*uses:\s*([^\s#]+)/gm)) {
      const action = match[1];
      assert.ok(
        action.startsWith("./") || /@[0-9a-f]{40}$/.test(action),
        `${name} contains a mutable action reference: ${action}`,
      );
    }
  }

  assert.match(releaseWorkflow, /^permissions:\n  contents: read$/m);
  assert.equal((releaseWorkflow.match(/contents: write/g) ?? []).length, 1);
  assert.match(
    releaseWorkflow,
    /publish:\n[\s\S]*?permissions:\n      contents: write/,
  );

  const validateStart = releaseWorkflow.indexOf("  validate-release:");
  const smokeStart = releaseWorkflow.indexOf("  smoke-release:");
  const publishStart = releaseWorkflow.indexOf("  publish:");
  const validateJob = releaseWorkflow.slice(validateStart, smokeStart);
  const smokeJob = releaseWorkflow.slice(smokeStart, publishStart);
  const publishJob = releaseWorkflow.slice(publishStart);
  assert.ok(validateStart > 0 && smokeStart > validateStart && publishStart > smokeStart);
  assert.match(publishJob, /name: Publish the sealed asset set at one visibility boundary/);
  assert.doesNotMatch(publishJob, /atomically/i);
  assert.match(publishJob, /not\n\s+# transactional tag-plus-release atomicity/);
  assert.match(validateJob, /permissions:\n      contents: read/);
  assert.doesNotMatch(validateJob, /"\$stage\/jeliyad" --version/);
  assert.match(validateJob, /release-receipt\.mjs create/);
  assert.match(smokeJob, /permissions:\n      contents: read/);
  assert.match(smokeJob, /"\$stage\/jeliyad" --version/);
  assert.match(publishJob, /release-receipt\.mjs" verify/);
  assert.doesNotMatch(publishJob, /"\$stage\/jeliyad" --version/);
  assert.doesNotMatch(publishJob, /uses: actions\/checkout@/);
  assert.match(publishJob, /git -c credential\.helper= -C "\$source_dir" fetch/);
  assert.equal((releaseWorkflow.match(/GH_TOKEN:/g) ?? []).length, 1);
  assert.equal((releaseWorkflow.match(/\$\{\{ github\.token \}\}/g) ?? []).length, 1);
  assert.doesNotMatch(publishJob.split("- name: Create, verify, and publish")[0], /GH_TOKEN:/);
  assert.match(
    publishJob,
    /- name: Create, verify, and publish the complete release\n        env:\n          GH_TOKEN:/,
  );
  assert.match(
    publishJob,
    /run: bash "\$RELEASE_SOURCE\/scripts\/finalize-release\.sh" dist/,
  );
});

test("the complete CI matrix can be dispatched without publishing", () => {
  assert.match(ciWorkflow, /^  workflow_dispatch:$/m);
  assert.match(ciWorkflow, /^  workflow_call:$/m);
  assert.doesNotMatch(ciWorkflow, /contents:\s*write/);
});

test("release promotion requires two clean CI runs before the sole write boundary", () => {
  assert.match(releaseWorkflow, /^  workflow_dispatch:/m);
  assert.doesNotMatch(releaseWorkflow, /^  push:\n\s+tags:/m);
  assert.match(releaseWorkflow, /concurrency:\n  group: release-\$\{\{ inputs\.version \}\}/);
  assert.doesNotMatch(releaseWorkflow, /group: release-[^\n]*github\.sha/);
  assert.equal(
    (releaseWorkflow.match(/uses: \.\/\.github\/workflows\/ci\.yml/g) ?? []).length,
    2,
    "the exact release revision must pass two independent reusable CI runs",
  );
  assert.equal(
    (releaseWorkflow.match(/fetch-depth: 0/g) ?? []).length,
    2,
    "the evidence-gate checkouts need full history; matrix builds stay shallow",
  );
  assert.doesNotMatch(releaseWorkflow, /git fetch[^\n]*--depth=1/);
  assert.equal(
    (releaseWorkflow.match(/git ls-remote --exit-code --heads origin/g) ?? []).length,
    3,
    "embedded UI, artifact validation, and receipt sealing must compare the public default-branch tip",
  );
  assert.match(
    releaseWorkflow,
    /embedded-ui:[\s\S]*?needs:\n      - verify-first\n      - verify-second/,
  );
  assert.match(
    releaseWorkflow,
    /build:[\s\S]*?needs:\n      - verify-first\n      - verify-second\n      - embedded-ui/,
  );
  assert.match(
    releaseWorkflow,
    /validate-release:[\s\S]*?needs:\n      - verify-first\n      - verify-second\n      - build/,
  );
  assert.match(
    releaseWorkflow,
    /smoke-release:[\s\S]*?needs:\n      - validate-release/,
  );
  assert.match(
    releaseWorkflow,
    /publish:[\s\S]*?needs:\n      - validate-release\n      - smoke-release/,
  );
  assert.equal(
    (releaseWorkflow.match(/check-release\.mjs --source --publish --tag/g) ?? []).length,
    2,
    "both the pre-build and read-only validation boundary must require READY evidence",
  );
  assert.match(
    releaseWorkflow,
    /x86_64-pc-windows-msvc[\s\S]*?jeliyad\.exe" --version/,
    "the native Windows release binary must be smoke-tested before packaging",
  );
  const receiptVerify = releaseWorkflow.indexOf('release-receipt.mjs" verify');
  const finalizerInvocation = releaseWorkflow.indexOf("scripts/finalize-release.sh");
  const finalTip = releaseFinalizer.lastIndexOf('final_default_tip="$(git ls-remote');
  const publicPatch = releaseFinalizer.indexOf("-F draft=false", finalTip);
  assert.ok(
    receiptVerify > 0
      && finalizerInvocation > receiptVerify
      && finalTip > 0
      && publicPatch > finalTip,
    "receipt and final default-branch tip must verify before the release becomes public",
  );
});

test("failed finalization cleans only run-owned draft and tag", () => {
  assert.match(releaseFinalizer, /trap cleanup_failed_publication EXIT/);
  assert.match(releaseFinalizer, /trap 'exit 130' INT/);
  assert.match(releaseFinalizer, /trap 'exit 143' TERM/);
  assert.match(releaseFinalizer, /draft_state=.*--jq '\.draft'/s);
  assert.match(releaseFinalizer, /if \[ "\$draft_state" = "false" \]/);
  assert.match(releaseFinalizer, /elif \[ "\$draft_state" = "true" \]/);
  assert.match(releaseFinalizer, /safe_to_delete_tag=0/);
  assert.match(releaseFinalizer, /\[ "\$safe_to_delete_tag" -eq 1 \]/);
  assert.match(releaseFinalizer, /\[ "\$created_tag" -eq 1 \]/);
  assert.match(releaseFinalizer, /run_marker="jeliya-release-run:\$\{GITHUB_RUN_ID\}:\$\{GITHUB_RUN_ATTEMPT\}"/);
  assert.match(releaseFinalizer, /grep -Fq "<!-- \$run_marker -->"/);
  assert.match(releaseFinalizer, /\[ "\$created_tag" -ne 1 \]/);
  assert.match(releaseFinalizer, /--notes-file "\$notes_file"/);
  assert.match(releaseFinalizer, /if \[ "\$current_sha" = "\$GITHUB_SHA" \]/);
  assert.match(releaseFinalizer, /published=1/);
  const privateValidation = releaseWorkflow.indexOf("node scripts/check-release.mjs --artifacts");
  const finalizerInvocation = releaseWorkflow.indexOf("scripts/finalize-release.sh");
  const existingRefusal = releaseFinalizer.indexOf('release $tag already exists');
  const trapActivation = releaseFinalizer.indexOf("trap cleanup_failed_publication EXIT");
  assert.ok(
    privateValidation > 0 && finalizerInvocation > privateValidation,
    "the complete private artifact set must validate before tag creation",
  );
  assert.ok(
    existingRefusal > 0 && trapActivation > existingRefusal,
    "cleanup ownership must start only after pre-existing release/tag refusal checks",
  );
});

test("artifact gate rejects both tampering and unexpected files", () => {
  const tag = "v9.8.7";
  const dir = mkdtempSync(join(tmpdir(), "jeliya-release-gate-"));
  const work = mkdtempSync(join(tmpdir(), "jeliya-release-payload-"));
  try {
    const archives = expectedArtifactNames(tag).filter((name) => !name.endsWith(".sha256"));
    for (const [index, archive] of archives.entries()) {
      const archivePath = join(dir, archive);
      const payload = join(work, String(index));
      mkdirSync(payload);
      const member = archive.endsWith(".zip") ? "jeliyad.exe" : "jeliyad";
      writeFileSync(join(payload, member), `fixture:${archive}`);
      if (archive.endsWith(".zip")) {
        execFileSync("zip", ["-q", "-j", archivePath, join(payload, member)]);
      } else {
        execFileSync("tar", ["-czf", archivePath, "-C", payload, member]);
      }
      const bytes = readFileSync(archivePath);
      const digest = createHash("sha256").update(bytes).digest("hex");
      writeFileSync(`${archivePath}.sha256`, `${digest}  ${archive}\n`);
    }
    assert.equal(validateArtifactSet(dir, tag).count, 10);

    writeFileSync(join(dir, archives[0]), "tampered");
    assert.throws(
      () => validateArtifactSet(dir, tag, { inspectArchives: false }),
      /does not match its published checksum/,
    );

    rmSync(join(dir, `${archives[0]}.sha256`));
    assert.throws(
      () => validateArtifactSet(dir, tag, { inspectArchives: false }),
      /artifact names differ/,
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
    rmSync(work, { recursive: true, force: true });
  }
});
