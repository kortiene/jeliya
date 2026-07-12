import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
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
  const required = [
    `A: ${path} path settled`,
    `B: ${path} path settled`,
    `C: ${path} path settled`,
    "B receives A message",
    "A receives B message",
    "C receives both messages",
    "C message converges to A and B",
    "B fetches and BLAKE3-verifies payload",
    "B fetched bytes are byte-identical",
    "C pipe gate forwards zero bytes to the target",
    "B observes and connects authorized pipe",
    "B reopens and receives the offline message",
    `B reconnects over ${path}`,
    "A seeds isolated foreign-room fixtures",
    "B public room-scoped RPCs do not disclose a foreign room ID",
    "B local-file HTTP endpoint does not disclose a foreign room ID",
    "B aggregate reads omit the foreign room and agent projection",
  ];
  while (required.length < 36) required.push(`fixture assertion ${required.length + 1}`);
  const relay = path === "relay";
  return {
    schema: 1,
    result: "pass",
    certifiable: true,
    expected_path: path,
    source: {
      commit,
      releaseable: true,
      iroh_rooms_revision: upstream,
      iroh_rooms: { releaseable: true },
    },
    build: { source_bound: true, locked: true },
    distinct_public_egress: {
      all_observed_addresses_different: true,
      distinct_autonomous_system_count: 2,
      independent_network_topology_proven: true,
    },
    assertions: required.map((name) => ({ name, result: "pass" })),
    cleanup: {
      completed: true,
      processes_stopped: true,
      temporary_artifacts_removed: true,
      failure_codes: [],
    },
    binaries: { local: { relay_only_attested: relay } },
    hosts: ["b", "c"].map((role) => ({
      role,
      binary_validation: {
        sha256: "ef".repeat(32),
        relay_only_attested: relay,
      },
    })),
  };
}

test("publication is blocked until retained direct and relay evidence is exact", () => {
  assert.throws(() => validateEvidenceReadiness(), /not implemented|not verified|not READY/);
  const root = mkdtempSync(join(tmpdir(), "jeliya-evidence-gate-"));
  try {
    const commit = "ab".repeat(20);
    const upstream = "cd".repeat(20);
    mkdirSync(join(root, "docs", "evidence", "v0.5.0"), { recursive: true });
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
    writeFileSync(
      join(root, "docs", "evidence", "v0.5.0", "direct.json"),
      `${JSON.stringify(networkManifest("direct", commit, upstream), null, 2)}\n`,
    );
    writeFileSync(
      join(root, "docs", "evidence", "v0.5.0", "relay.json"),
      `${JSON.stringify(networkManifest("relay", commit, upstream), null, 2)}\n`,
    );
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

    const relay = networkManifest("relay", commit, upstream);
    relay.hosts[0].binary_validation.relay_only_attested = false;
    assert.throws(() => validateNetworkEvidenceManifest(relay, {
      expectedPath: "relay",
      candidateCommit: commit,
      upstreamRevision: upstream,
    }), /relay-only attestation/);

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
