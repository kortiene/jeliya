import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";

const sh = readFileSync(new URL("../packaging/install.sh", import.meta.url), "utf8");
const ps = readFileSync(new URL("../packaging/install.ps1", import.meta.url), "utf8");

test("POSIX installer verifies the exact checksum sidecar before extraction", () => {
  const verify = sh.indexOf('[ "$actual" = "$expected" ]');
  const extract = sh.indexOf('tar -xzf "$tmp/$ASSET"');
  assert.ok(sh.includes('download "${URL}.sha256"'));
  assert.ok(sh.includes('[ "$listed" = "$ASSET" ]'));
  assert.ok(verify > 0 && extract > verify, "checksum verification must precede tar extraction");
});

test("PowerShell installer verifies hash and filename before Expand-Archive", () => {
  const verify = ps.indexOf('if ($actualHash -ne $expectedHash)');
  const extract = ps.indexOf("Expand-Archive -LiteralPath $tmpArchive");
  const rejectReparse = ps.indexOf("[System.IO.FileAttributes]::ReparsePoint");
  const install = ps.indexOf("Copy-Item -LiteralPath $stagedExe");
  assert.ok(ps.includes('Invoke-WebRequest -Uri "$url.sha256"'));
  assert.ok(ps.includes('if ($listedAsset -cne $asset)'));
  assert.ok(verify > 0 && extract > verify, "checksum verification must precede zip extraction");
  assert.ok(
    rejectReparse > extract && install > rejectReparse,
    "reparse points must be rejected before installation",
  );
});

test("POSIX installer installs verified bytes and rejects a tampered archive", () => {
  const root = mkdtempSync(join(tmpdir(), "jeliya-installer-e2e-"));
  const fixtureDir = join(root, "fixtures");
  const payloadDir = join(root, "payload");
  const mockBin = join(root, "bin");
  const installDir = join(root, "install");
  const asset = "jeliyad-v9.8.7-x86_64-unknown-linux-musl.tar.gz";
  mkdirSync(fixtureDir);
  mkdirSync(payloadDir);
  mkdirSync(mockBin);
  writeFileSync(join(payloadDir, "jeliyad"), "verified daemon fixture\n");
  execFileSync("tar", ["-czf", join(fixtureDir, asset), "-C", payloadDir, "jeliyad"]);
  const digest = createHash("sha256").update(readFileSync(join(fixtureDir, asset))).digest("hex");
  writeFileSync(join(fixtureDir, `${asset}.sha256`), `${digest}  ${asset}\n`);

  const curlMock = join(mockBin, "curl");
  writeFileSync(
    curlMock,
    `#!/bin/sh\nurl="$2"\ndest="$4"\ncase "$url" in\n  *.sha256) cp "$FIXTURE_DIR/$ASSET.sha256" "$dest" ;;\n  *) cp "$FIXTURE_DIR/$ASSET" "$dest" ;;\nesac\n`,
  );
  chmodSync(curlMock, 0o755);
  const unameMock = join(mockBin, "uname");
  writeFileSync(
    unameMock,
    "#!/bin/sh\ncase \"$1\" in -s) echo Linux ;; -m) echo x86_64 ;; *) echo Linux ;; esac\n",
  );
  chmodSync(unameMock, 0o755);

  const env = {
    ...process.env,
    ASSET: asset,
    FIXTURE_DIR: fixtureDir,
    INSTALL_DIR: installDir,
    JELIYA_VERSION: "v9.8.7",
    PATH: `${mockBin}:${process.env.PATH}`,
  };
  try {
    const installer = resolve(new URL("../packaging/install.sh", import.meta.url).pathname);
    const ok = spawnSync("sh", [installer], { env, encoding: "utf8" });
    assert.equal(ok.status, 0, ok.stderr);
    assert.equal(readFileSync(join(installDir, "jeliyad"), "utf8"), "verified daemon fixture\n");

    rmSync(installDir, { recursive: true, force: true });
    writeFileSync(join(fixtureDir, asset), "tampered archive");
    const rejected = spawnSync("sh", [installer], { env, encoding: "utf8" });
    assert.notEqual(rejected.status, 0);
    assert.match(rejected.stderr, /checksum mismatch/);
    assert.equal(existsSync(join(installDir, "jeliyad")), false);

    // Even a checksum-valid archive must contain a regular binary, not a
    // symlink that could escape the extraction directory when installed.
    rmSync(join(payloadDir, "jeliyad"));
    symlinkSync("/bin/sh", join(payloadDir, "jeliyad"));
    execFileSync("tar", ["-czf", join(fixtureDir, asset), "-C", payloadDir, "jeliyad"]);
    const symlinkDigest = createHash("sha256")
      .update(readFileSync(join(fixtureDir, asset)))
      .digest("hex");
    writeFileSync(join(fixtureDir, `${asset}.sha256`), `${symlinkDigest}  ${asset}\n`);
    const symlinkRejected = spawnSync("sh", [installer], { env, encoding: "utf8" });
    assert.notEqual(symlinkRejected.status, 0);
    assert.match(symlinkRejected.stderr, /regular 'jeliyad' file/);
    assert.equal(existsSync(join(installDir, "jeliyad")), false);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
