#!/usr/bin/env node
// Release-integrity gate for source versions and the complete daemon artifact
// set. Node 22+, no npm dependencies.
//
//   node scripts/check-release.mjs --source [--publish] [--tag v0.5.0]
//   node scripts/check-release.mjs --artifacts dist --tag v0.5.0

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
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

export function validateEvidenceReadiness({ root = repoRoot } = {}) {
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
  if (!/^\| Candidate commit \| `[0-9a-f]{40}` \|$/m.test(candidateSection)) {
    fail("candidate evidence must name one exact 40-hex commit");
  }
  if (!/^\| Candidate upstream remediation revision \| `[0-9a-f]{40}` \|$/m.test(candidateSection)) {
    fail("candidate evidence must name one exact 40-hex upstream revision");
  }
  for (const gate of ["Direct different-network P2P", "Deliberately constrained relay"]) {
    const row = evidence.split(/\r?\n/).find((line) => line.startsWith(`| ${gate} |`));
    if (!row || !/\| passed(?:\s|`|;|\|)/i.test(row)) {
      fail(`${gate} evidence is not passed`);
    }
  }
  return { ready: true };
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

  if (checkSource) {
    const result = validateSourceVersions({ tag });
    console.log(`release-integrity: source versions match ${result.tag}`);
  }
  if (checkPublish) {
    validateEvidenceReadiness();
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
