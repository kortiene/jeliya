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
  validateEvidenceReadiness,
  validateNetworkEvidenceManifest,
  validateSourceVersions,
} from "./check-release.mjs";

const ciWorkflow = readFileSync(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8");
const releaseWorkflow = readFileSync(
  new URL("../.github/workflows/release.yml", import.meta.url),
  "utf8",
);

test("checked-in release surfaces share one dated version", () => {
  const result = validateSourceVersions();
  assert.equal(result.tag, `v${result.version}`);
  assert.equal(new Set(Object.values(result.versions)).size, 1);
});

function networkManifest(path, commit, upstream) {
  const relay = path === "relay";
  const digest = "ef".repeat(32);
  const deniedMethods = [
    "room.open", "room.close", "room.leave", "room.timeline", "room.members",
    "invite.create", "message.send", "status.post", "file.share", "file.list",
    "file.fetch", "pipe.expose", "pipe.list", "pipe.connect", "pipe.close",
    "peers.status", "agent.history",
  ];
  return {
    schema: 1,
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
      published_at_origin: true,
      releaseable: true,
      iroh_rooms_revision: upstream,
      iroh_rooms: {
        requested_revision: upstream,
        resolved_revision: upstream,
        public_source: true,
        published_at_origin: true,
        releaseable: true,
      },
    },
    build: {
      mode: "from-source",
      source_bound: true,
      source_snapshot_commit: commit,
      locked: true,
      features: relay ? ["embed-ui", "relay-only-test"] : ["embed-ui"],
      targets: ["x86_64-apple-darwin", "x86_64-unknown-linux-musl"],
    },
    distinct_public_egress: {
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
        engine_verified: true,
        sha256_equal: true,
        expected_sha256: digest,
        actual_sha256: digest,
      },
      pipe: { unauthorized_third_peer: { target_connections: 0, target_requests: 0 } },
      reconnect: {
        offline_message_resynchronized: true,
        settled_path: { expected_path: path },
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
      local: { relay_only_attested: relay, version: "0.5.0" },
      remote: { sha256: digest, expected_version: "0.5.0" },
    },
    hosts: ["b", "c"].map((role) => ({
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
      },
    })),
    sanitized_logs: {
      roles: ["a", "b", "c"].map((role) => ({
        role,
        streams: Object.fromEntries(["stdout", "stderr"].map((stream) => [stream, {
          lines: 1,
          bytes: 1,
          sha256: digest,
        }])),
      })),
    },
  };
}

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
    const writeSignedManifest = (path) => {
      const relativePath = join("docs", "evidence", "v0.5.0", `${path}.json`);
      const contents = Buffer.from(`${JSON.stringify(networkManifest(path, commit, upstream), null, 2)}\n`);
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

    writeFileSync(
      join(root, "docs", "evidence", "v0.5.0", "direct.json"),
      Buffer.concat([directContents, Buffer.from(" ")]),
    );
    assert.throws(() => validateEvidenceReadiness({ root, context }), /invalidly signed/);
    writeFileSync(join(root, "docs", "evidence", "v0.5.0", "direct.json"), directContents);

    const relay = networkManifest("relay", commit, upstream);
    relay.hosts[0].binary_validation.relay_only_attested = false;
    assert.throws(() => validateNetworkEvidenceManifest(relay, {
      expectedPath: "relay",
      candidateCommit: commit,
      upstreamRevision: upstream,
    }), /relay-only attestation/);

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
    duplicatedHost.hosts[1] = { ...duplicatedHost.hosts[0] };
    assert.throws(() => validateNetworkEvidenceManifest(duplicatedHost, {
      expectedPath: "direct",
      candidateCommit: commit,
      upstreamRevision: upstream,
    }), /two independently verified remote binaries/);
    const rootExecution = networkManifest("direct", commit, upstream);
    delete rootExecution.hosts[0].binary_validation.execution_uid;
    assert.throws(() => validateNetworkEvidenceManifest(rootExecution, {
      expectedPath: "direct",
      candidateCommit: commit,
      upstreamRevision: upstream,
    }), /two independently verified remote binaries/);
    const incompleteTopology = networkManifest("direct", commit, upstream);
    delete incompleteTopology.distinct_public_egress.pairwise;
    assert.throws(() => validateNetworkEvidenceManifest(incompleteTopology, {
      expectedPath: "direct",
      candidateCommit: commit,
      upstreamRevision: upstream,
    }), /required sanitized topology/);

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
});

test("release promotion requires two clean CI runs before the sole write boundary", () => {
  assert.match(releaseWorkflow, /^  workflow_dispatch:/m);
  assert.doesNotMatch(releaseWorkflow, /^  push:\n\s+tags:/m);
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
    2,
    "both release boundaries must compare the public default-branch tip without re-shallowing history",
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
    /publish:[\s\S]*?needs:\n      - verify-first\n      - verify-second\n      - build/,
  );
  assert.equal(
    (releaseWorkflow.match(/check-release\.mjs --source --publish --tag/g) ?? []).length,
    2,
    "both the pre-build and final publish boundary must require READY evidence",
  );
});

test("failed finalization cleans only run-owned draft and tag", () => {
  assert.match(releaseWorkflow, /trap cleanup_failed_publication EXIT/);
  assert.match(releaseWorkflow, /trap 'exit 130' INT/);
  assert.match(releaseWorkflow, /trap 'exit 143' TERM/);
  assert.match(releaseWorkflow, /draft_state=.*--jq '\.draft'/s);
  assert.match(releaseWorkflow, /if \[ "\$draft_state" = "false" \]/);
  assert.match(releaseWorkflow, /elif \[ "\$draft_state" = "true" \]/);
  assert.match(releaseWorkflow, /safe_to_delete_tag=0/);
  assert.match(releaseWorkflow, /\[ "\$safe_to_delete_tag" -eq 1 \]/);
  assert.match(releaseWorkflow, /\[ "\$created_tag" -eq 1 \]/);
  assert.match(releaseWorkflow, /run_marker="jeliya-release-run:\$\{GITHUB_RUN_ID\}:\$\{GITHUB_RUN_ATTEMPT\}"/);
  assert.match(releaseWorkflow, /grep -Fq "<!-- \$run_marker -->"/);
  assert.match(releaseWorkflow, /\[ "\$created_tag" -ne 1 \]/);
  assert.match(releaseWorkflow, /--notes-file "\$notes_file"/);
  assert.match(releaseWorkflow, /if \[ "\$current_sha" = "\$GITHUB_SHA" \]/);
  assert.match(releaseWorkflow, /published=1/);
  const privateValidation = releaseWorkflow.indexOf("node scripts/check-release.mjs --artifacts");
  const tagCreation = releaseWorkflow.indexOf('ref="refs/tags/${tag}"');
  const existingRefusal = releaseWorkflow.indexOf('release $tag already exists');
  const trapActivation = releaseWorkflow.indexOf("trap cleanup_failed_publication EXIT");
  assert.ok(
    privateValidation > 0 && tagCreation > privateValidation,
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
