#!/usr/bin/env node
// Phase 5 packaging pipeline: turn the Flutter app + jeliyad into a signed
// (and, with credentials, notarized) Jeliya.app + DMG.
//
//   node scripts/package-macos.mjs [--skip-universal] [--skip-gate] [--skip-dmg]
//
// Steps, in order:
//   1. cargo build --release jeliyad for x86_64 + aarch64 (lipo → universal;
//      --skip-universal or a missing rust target falls back to host-arch)
//   2. flutter build macos --release (sandboxed via Release.entitlements)
//   3. copy jeliyad into Jeliya.app/Contents/Helpers/ (resolveJeliyadBinary
//      finds it there — no env override in shipped builds)
//   4. sign inner→outer with hardened runtime: frameworks/dylibs, then the
//      sidecar with Sidecar.entitlements (sandbox + inherit), then the app
//      with Release.entitlements
//   5. verify: codesign --strict --deep, entitlement asserts (app-sandbox on
//      both, inherit on the sidecar, the shared-dir exception on the app)
//   6. runtime gate: launch the SANDBOXED bundle with JELIYA_DATA_DIR inside
//      the exception path, prove sidecar spawn + portfile + clean teardown
//   7. DMG (hdiutil, with /Applications symlink) → dist/
//   8. notarize + staple — only when both credentials are present
//
// Signing credentials (all optional — defaults produce a locally-runnable
// ad-hoc build):
//   JELIYA_SIGN_IDENTITY   codesign identity ("Developer ID Application: …");
//                          default "-" (ad-hoc: sandbox works, Gatekeeper on
//                          OTHER machines will refuse it)
//   JELIYA_NOTARY_PROFILE  notarytool keychain profile name (created once via
//                          `xcrun notarytool store-credentials`); requires a
//                          real identity. Absent → notarization is skipped
//                          with instructions.
//
// Node 22+; no npm deps.

import { execFileSync, spawn } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
} from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const appDir = join(repoRoot, "app");
const flags = new Set(process.argv.slice(2));
const IDENTITY = process.env.JELIYA_SIGN_IDENTITY ?? "-";
const NOTARY_PROFILE = process.env.JELIYA_NOTARY_PROFILE ?? "";
const adHoc = IDENTITY === "-";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let step = 0;
const log = (msg) => console.log(`package-macos: ${msg}`);
const begin = (msg) => console.log(`\npackage-macos: [${++step}] ${msg}`);
const die = (msg) => {
  console.error(`package-macos: FAIL — ${msg}`);
  process.exit(1);
};

function run(cmd, args, opts = {}) {
  log(`$ ${cmd} ${args.join(" ")}`);
  return execFileSync(cmd, args, { stdio: ["ignore", "inherit", "inherit"], ...opts });
}

function capture(cmd, args, opts = {}) {
  return execFileSync(cmd, args, { encoding: "utf8", ...opts });
}

const appVersion = (() => {
  const m = readFileSync(join(appDir, "pubspec.yaml"), "utf8").match(/^version:\s*([\d.]+)/m);
  return m ? m[1] : "0.0.0";
})();

// --- 1. jeliyad release binary (universal when possible) -----------------------------

begin("building jeliyad (release)");
const hostArch = process.arch === "arm64" ? "aarch64" : "x86_64";
const targets = [`${hostArch}-apple-darwin`];
if (!flags.has("--skip-universal")) {
  const other = hostArch === "aarch64" ? "x86_64-apple-darwin" : "aarch64-apple-darwin";
  const installed = capture("rustup", ["target", "list", "--installed"]);
  if (installed.includes(other)) targets.push(other);
  else log(`WARNING: rust target ${other} not installed — building ${hostArch}-only (rustup target add ${other})`);
}
for (const t of targets) {
  run("cargo", ["build", "--release", "-p", "jeliyad", "--target", t], { cwd: repoRoot });
}
const jeliyadOut = join(repoRoot, "target", "universal", "jeliyad");
mkdirSync(dirname(jeliyadOut), { recursive: true });
rmSync(jeliyadOut, { force: true });
if (targets.length === 2) {
  run("lipo", [
    "-create",
    ...targets.map((t) => join(repoRoot, "target", t, "release", "jeliyad")),
    "-output", jeliyadOut,
  ]);
} else {
  copyFileSync(join(repoRoot, "target", targets[0], "release", "jeliyad"), jeliyadOut);
}
log(`jeliyad archs: ${capture("lipo", ["-archs", jeliyadOut]).trim()}`);

// --- 2. flutter release build ---------------------------------------------------------

begin("building Jeliya.app (flutter release)");
run("flutter", ["pub", "get"], { cwd: appDir });
run("flutter", ["build", "macos", "--release"], { cwd: appDir });
const appBundle = join(appDir, "build", "macos", "Build", "Products", "Release", "Jeliya.app");
if (!existsSync(appBundle)) die(`expected bundle at ${appBundle}`);

// --- 3. bundle the sidecar -------------------------------------------------------------

begin("bundling the sidecar into Contents/Helpers");
const helpers = join(appBundle, "Contents", "Helpers");
mkdirSync(helpers, { recursive: true });
copyFileSync(jeliyadOut, join(helpers, "jeliyad"));
run("chmod", ["755", join(helpers, "jeliyad")]);

// --- 4. sign inner → outer -------------------------------------------------------------

begin(`signing (identity: ${adHoc ? "ad-hoc" : IDENTITY})`);
// Hardened runtime ONLY with a real identity: it is a notarization
// prerequisite, and its library validation compares Team IDs — ad-hoc
// signatures have none, so an ad-hoc hardened-runtime app refuses to load
// its own ad-hoc frameworks ("different Team IDs" dyld errors). The sandbox
// (what the local gate exercises) is independent of the hardened runtime.
const runtimeFlags = ["--force", ...(adHoc ? [] : ["--options", "runtime", "--timestamp"])];
const sign = (path, entitlements) =>
  run("codesign", [
    ...runtimeFlags,
    "--sign", IDENTITY,
    ...(entitlements ? ["--entitlements", entitlements] : []),
    path,
  ]);

const frameworksDir = join(appBundle, "Contents", "Frameworks");
if (existsSync(frameworksDir)) {
  for (const entry of readdirSync(frameworksDir)) {
    if (entry.endsWith(".framework") || entry.endsWith(".dylib")) {
      sign(join(frameworksDir, entry));
    }
  }
}
sign(join(helpers, "jeliyad"), join(appDir, "macos", "Runner", "Sidecar.entitlements"));
sign(appBundle, join(appDir, "macos", "Runner", "Release.entitlements"));

// --- 5. static verification --------------------------------------------------------------

begin("verifying signatures and entitlements");
run("codesign", ["--verify", "--strict", "--deep", "--verbose=2", appBundle]);
const entsOf = (path) =>
  capture("codesign", ["-d", "--entitlements", "-", "--xml", path]).toString();
const appEnts = entsOf(appBundle);
const helperEnts = entsOf(join(helpers, "jeliyad"));
if (!appEnts.includes("com.apple.security.app-sandbox")) die("app is not sandboxed");
if (!appEnts.includes("Library/Application Support/Jeliya/")) die("app lacks the shared-dir exception");
if (!helperEnts.includes("com.apple.security.app-sandbox")) die("sidecar is not sandboxed");
if (!helperEnts.includes("com.apple.security.inherit")) die("sidecar does not inherit the sandbox");
log("entitlement asserts OK (sandbox on both, inherit on sidecar, shared-dir exception on app)");
if (!adHoc) {
  try {
    run("spctl", ["--assess", "--type", "execute", "--verbose", appBundle]);
  } catch {
    log("spctl assess failed (expected until notarization is stapled)");
  }
}

// --- 6. runtime gate: sandboxed spawn/teardown -----------------------------------------------

if (flags.has("--skip-gate")) {
  begin("runtime gate SKIPPED (--skip-gate)");
} else {
  begin("runtime gate: sandboxed bundle spawns and tears down the sidecar");
  // The sandbox only allows the container + the shared-dir exception, so the
  // gate data dir must live INSIDE the exception path. A dot-dir keeps it out
  // of the user's way; it is deleted afterwards. The user's real daemon (if
  // any) uses the parent dir's own daemon.json/lock — no interference.
  const gateDir = join(homedir(), "Library", "Application Support", "Jeliya", `.package-gate-${process.pid}`);
  rmSync(gateDir, { recursive: true, force: true });
  const appProc = spawn(join(appBundle, "Contents", "MacOS", "Jeliya"), [], {
    env: { ...process.env, JELIYA_DATA_DIR: gateDir },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let appOut = "";
  appProc.stdout.on("data", (d) => (appOut += d));
  appProc.stderr.on("data", (d) => (appOut += d));
  const portfile = join(gateDir, "daemon.json");
  try {
    let pf = null;
    for (let i = 0; i < 60 && !pf; i++) {
      await sleep(500);
      if (existsSync(portfile)) pf = JSON.parse(readFileSync(portfile, "utf8"));
      if (appProc.exitCode !== null) break;
    }
    if (!pf) {
      die(`sidecar never wrote a portfile in the gate data dir.\napp output:\n${appOut.slice(-2000)}`);
    }
    log(`sidecar up (pid ${pf.pid}, port ${pf.port}) inside the sandbox`);
    const health = await fetch(`http://127.0.0.1:${pf.port}/api/health`).then((r) => r.json());
    if (health.pid !== pf.pid) die("health check does not match the portfile");
    log("health check OK");
    appProc.kill("SIGTERM");
    let gone = false;
    for (let i = 0; i < 30 && !gone; i++) {
      await sleep(500);
      try {
        process.kill(pf.pid, 0);
      } catch {
        gone = true;
      }
    }
    if (!gone) die("sidecar ORPHANED after app teardown");
    if (existsSync(portfile)) die("portfile not removed on teardown");
    log("teardown OK — sidecar exited, portfile removed, no orphan");
  } finally {
    try { appProc.kill("SIGKILL"); } catch {}
    rmSync(gateDir, { recursive: true, force: true });
  }
}

// --- 7. DMG ------------------------------------------------------------------------------------

let dmgPath = "";
if (flags.has("--skip-dmg")) {
  begin("DMG SKIPPED (--skip-dmg)");
} else {
  begin("building the DMG");
  const dist = join(repoRoot, "dist");
  mkdirSync(dist, { recursive: true });
  const staging = join(dist, ".dmg-staging");
  rmSync(staging, { recursive: true, force: true });
  mkdirSync(staging, { recursive: true });
  // ditto (not fs.cpSync) preserves framework structure verbatim. cpSync
  // resolves relative symlinks to absolute build-tree paths, which both
  // un-relocates the bundle (flutter_assets not found on other machines /
  // once the build tree moves) and unseals the signed frameworks.
  run("ditto", [appBundle, join(staging, "Jeliya.app")]);
  symlinkSync("/Applications", join(staging, "Applications"));
  dmgPath = join(dist, `Jeliya-v${appVersion}-macos.dmg`);
  rmSync(dmgPath, { force: true });
  run("hdiutil", ["create", "-volname", "Jeliya", "-srcfolder", staging, "-format", "UDZO", "-quiet", dmgPath]);
  rmSync(staging, { recursive: true, force: true });
  if (!adHoc) run("codesign", ["--force", "--sign", IDENTITY, dmgPath]);
  log(`DMG: ${dmgPath}`);
}

// --- 8. notarization ------------------------------------------------------------------------------

begin("notarization");
if (adHoc || !NOTARY_PROFILE || !dmgPath) {
  log("SKIPPED — needs a Developer ID identity (JELIYA_SIGN_IDENTITY) and a");
  log("notarytool profile (JELIYA_NOTARY_PROFILE). Once the Apple Developer");
  log("enrollment lands:");
  log("  xcrun notarytool store-credentials jeliya-notary \\");
  log("    --apple-id <id> --team-id <team> --password <app-specific>");
  log("  JELIYA_SIGN_IDENTITY='Developer ID Application: …' \\");
  log("  JELIYA_NOTARY_PROFILE=jeliya-notary node scripts/package-macos.mjs");
} else {
  run("xcrun", ["notarytool", "submit", dmgPath, "--keychain-profile", NOTARY_PROFILE, "--wait"]);
  run("xcrun", ["stapler", "staple", dmgPath]);
  log("notarized and stapled");
}

console.log(`\npackage-macos: DONE — Jeliya.app v${appVersion} (${adHoc ? "ad-hoc signed, this-machine-only" : `signed: ${IDENTITY}`})`);
console.log(`package-macos: bundle: ${appBundle}`);
if (dmgPath) console.log(`package-macos: dmg:    ${dmgPath}`);
