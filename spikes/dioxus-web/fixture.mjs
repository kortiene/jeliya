#!/usr/bin/env node
// Disposable daemon fixture for the #158 spike.
//
// Spawns a REAL supervised `jeliyad` on a fresh, isolated data dir, serving the
// spike's own `dist` as its UI. Holds the child's stdin open, because
// `--supervised` means the daemon exits on stdin EOF (docs/PROTOCOL.md).
//
// Prints the parsed `ready` line as JSON on its own stdout so a caller (or
// Playwright's webServer) can read the ephemeral port, then stays alive until
// killed. The data dir is removed on exit: the spike creates no durable state.
//
//   node fixture.mjs [--ui-dir <path>] [--bin <path>] [--keep-data]

import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createInterface } from 'node:readline';

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, '../..');

function arg(name, fallback) {
  const i = process.argv.indexOf(name);
  return i !== -1 && process.argv[i + 1] ? process.argv[i + 1] : fallback;
}
const keepData = process.argv.includes('--keep-data');
const bin = resolve(arg('--bin', join(repo, 'target/debug/jeliyad')));
// Default: serve the spike's own build from disk (the development path).
// `--no-ui-dir` passes no `--ui-dir` at all, so an `embed-ui` daemon must serve
// the assets compiled into it — that is the other half of acceptance
// criterion 2, and it only proves anything if no directory is offered.
const uiDirRaw = process.argv.includes('--no-ui-dir') ? '' : arg('--ui-dir', join(here, 'dist'));

if (!existsSync(bin)) {
  console.error(`fixture: no daemon at ${bin} — run \`cargo build -p jeliyad\` first`);
  process.exit(1);
}

// `--port 0` (the default) takes an ephemeral port and prints it. Playwright
// needs a URL it can poll before the process talks, so the test passes a fixed
// one; a collision there fails loudly rather than silently testing the wrong
// daemon.
const port = arg('--port', '0');

const dataDir = mkdtempSync(join(tmpdir(), 'jeliya-spike158-'));
const args = ['--supervised', '--port', port, '--data-dir', dataDir];
if (uiDirRaw) args.push('--ui-dir', resolve(uiDirRaw));

// stdin is a pipe we deliberately hold open; closing it is the documented
// parent-death signal, so the daemon dies with this process even on SIGKILL.
const child = spawn(bin, args, { stdio: ['pipe', 'pipe', 'pipe'] });

let settled = false;
const cleanup = () => {
  try { child.kill('SIGTERM'); } catch {}
  if (!keepData) { try { rmSync(dataDir, { recursive: true, force: true }); } catch {} }
};
process.on('exit', cleanup);
for (const sig of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
  process.on(sig, () => { cleanup(); process.exit(0); });
}

createInterface({ input: child.stdout }).on('line', (line) => {
  if (settled || !line.startsWith('{')) return;
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  settled = true;
  if (msg.event === 'already_running') {
    console.error('fixture: a daemon already owns this data dir — unexpected for a fresh temp dir');
    process.exit(1);
  }
  // The token is NOT printed. The browser obtains it same-origin via
  // /api/session, which is the path the spike is meant to exercise.
  console.log(JSON.stringify({
    event: msg.event, port: msg.port, http: msg.http, ws: msg.ws,
    protocol: msg.protocol, version: msg.version, dataDir,
  }));
});

// Daemon logs go to our stderr, prefixed, so they never contaminate the
// machine-readable line above.
createInterface({ input: child.stderr }).on('line', (l) => console.error(`[jeliyad] ${l}`));

child.on('exit', (code, signal) => {
  console.error(`[jeliyad] exited code=${code} signal=${signal}`);
  cleanup();
  process.exit(code ?? 1);
});
