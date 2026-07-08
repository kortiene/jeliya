// Shared helper: the daemon's WS auth token, read from its portfile
// (`<data_dir>/daemon.json` — docs/PROTOCOL.md, "Process supervision").
//
// jeliyad mints a fresh token per start and requires it on /ws and
// /api/files/*. Native clients (these scripts) are expected to read it from
// the portfile; only the browser UI uses /api/session. The portfile appears
// once the daemon is ready, so callers re-read it inside their connect-retry
// loops rather than once up front.

import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

/** Mirror of the daemon's platform default data dir (main.rs default_data_dir). */
export function defaultDataDir() {
  if (process.platform === "darwin") {
    return join(homedir(), "Library", "Application Support", "Jeliya");
  }
  if (process.platform === "win32") {
    const appData = process.env.APPDATA ?? join(homedir(), "AppData", "Roaming");
    return join(appData, "Jeliya");
  }
  const dataHome = process.env.XDG_DATA_HOME ?? join(homedir(), ".local", "share");
  return join(dataHome, "Jeliya");
}

/** The parsed portfile for a data dir, or null (absent/unreadable — e.g. the
 *  daemon is still starting, or an older jeliyad with no portfile). */
export function readPortfile(dataDir) {
  try {
    return JSON.parse(readFileSync(join(dataDir, "daemon.json"), "utf8"));
  } catch {
    return null;
  }
}

/** The auth token from a data dir's portfile, or null. */
export function readDaemonToken(dataDir) {
  const portfile = readPortfile(dataDir);
  return portfile && typeof portfile.auth_token === "string" && portfile.auth_token
    ? portfile.auth_token
    : null;
}

/**
 * Wire a spawned jeliyad's stdout/stderr to this process's, detecting the
 * supervision contract's `already_running` line (a second daemon on the same
 * data dir adopts the incumbent and exits 0). On adoption we must NOT treat the
 * exit as a failure — the caller connects to the live daemon on the same port.
 *
 * Pass the failure handler you'd otherwise attach to `proc.on('exit')`; it is
 * called only for genuine early exits, never for a clean adoption.
 */
export function pipeDaemonOutput(proc, label, onFailure) {
  let adopted = false;
  let buffer = "";
  proc.stdout.on("data", (chunk) => {
    buffer += String(chunk);
    const newlineAt = buffer.lastIndexOf("\n");
    if (newlineAt >= 0) {
      for (const line of buffer.slice(0, newlineAt).split("\n")) {
        const trimmed = line.trim();
        if (!trimmed.startsWith("{")) continue;
        try {
          if (JSON.parse(trimmed).event === "already_running") adopted = true;
        } catch {}
      }
      buffer = buffer.slice(newlineAt + 1);
    }
    process.stdout.write(`[${label}] ${chunk}`);
  });
  proc.stderr.on("data", (chunk) => process.stderr.write(`[${label}] ${chunk}`));
  proc.on("exit", (code, signal) => {
    if (adopted && code === 0) {
      console.error(`[${label}] adopted an already-running daemon on this data dir`);
      return;
    }
    onFailure?.(code, signal);
  });
  return proc;
}

/** `ws://127.0.0.1:<port>/ws`, with `?token=` ONLY when the portfile actually
 *  describes a daemon on that same port — otherwise the data dir belongs to a
 *  daemon on a different port and its token must not be handed to whoever
 *  happens to own `port` (a 256-bit secret leak to an unrelated listener). */
export function wsUrlFor(port, dataDir) {
  const base = `ws://127.0.0.1:${port}/ws`;
  if (!dataDir) return base;
  const portfile = readPortfile(dataDir);
  const token =
    portfile && portfile.port === port && typeof portfile.auth_token === "string"
      ? portfile.auth_token
      : null;
  return token ? `${base}?token=${token}` : base;
}
