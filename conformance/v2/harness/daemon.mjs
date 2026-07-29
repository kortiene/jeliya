// Daemon lifecycle for the v2 conformance harness.
//
// Spawns `jeliyad` on a fresh data dir with an OS-chosen loopback port, reads
// the portfile for the bound port and auth token, and tears the process down
// deterministically. One daemon per case that requires `daemon:fresh` (the
// default); `daemon:second` spawns an additional, independent daemon so
// cross-daemon cases (links, relays, foreign rooms) have a peer.

import { spawn } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, existsSync, watchFile, unwatchFile } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const READY_TIMEOUT_MS = 15_000;

/** A running daemon under test. */
export class Daemon {
  constructor(proc, dataDir, port, token, sg) {
    this.proc = proc;
    this.dataDir = dataDir;
    this.port = port;
    this.token = token;
    this.storageGeneration = sg;
    this.exited = null; // exit status once observed
    proc.on('exit', (code, signal) => {
      this.exited = { code, signal };
    });
  }

  get wsBase() {
    return `ws://127.0.0.1:${this.port}/ws`;
  }

  /** Stop the daemon (SIGTERM) and wait for exit. */
  async stop() {
    if (this.exited) return;
    this.proc.kill('SIGTERM');
    await new Promise((resolve) => {
      const t = setTimeout(() => {
        this.proc.kill('SIGKILL');
        resolve();
      }, 5_000);
      this.proc.once('exit', () => {
        clearTimeout(t);
        resolve();
      });
    });
    this.cleanup();
  }

  cleanup() {
    try {
      rmSync(this.dataDir, { recursive: true, force: true });
    } catch {
      /* best effort */
    }
  }
}

/** Wait for the portfile to appear and be parseable. */
function readPortfile(dataDir, timeoutMs) {
  const path = join(dataDir, 'daemon.json');
  return new Promise((resolve, reject) => {
    const deadline = Date.now() + timeoutMs;
    const tryRead = () => {
      if (existsSync(path)) {
        try {
          const pf = JSON.parse(readFileSync(path, 'utf8'));
          if (pf && pf.port && pf.auth_token) {
            unwatchFile(path);
            resolve(pf);
            return;
          }
        } catch {
          /* not fully written yet */
        }
      }
      if (Date.now() > deadline) {
        unwatchFile(path);
        reject(new Error(`portfile did not appear within ${timeoutMs}ms at ${path}`));
        return;
      }
      setTimeout(tryRead, 50);
    };
    tryRead();
  });
}

/**
 * Spawn a daemon on a fresh data dir. `binary` is the path to `jeliyad`.
 * Returns a Daemon once its portfile is readable.
 */
export async function startDaemon(binary, { loopback = true } = {}) {
  const dataDir = mkdtempSync(join(tmpdir(), 'jeliya-conf-'));
  const args = [
    '--port',
    '0',
    '--data-dir',
    dataDir,
    '--no-open',
  ];
  if (loopback) args.push('--loopback');
  const proc = spawn(binary, args, {
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let stderr = '';
  proc.stderr.on('data', (d) => {
    stderr += d;
  });
  try {
    const pf = await readPortfile(dataDir, READY_TIMEOUT_MS);
    return new Daemon(proc, dataDir, pf.port, pf.auth_token, pf.protocol);
  } catch (err) {
    proc.kill('SIGKILL');
    rmSync(dataDir, { recursive: true, force: true });
    throw new Error(`daemon failed to start: ${err.message}\nstderr: ${stderr.slice(-500)}`);
  }
}
