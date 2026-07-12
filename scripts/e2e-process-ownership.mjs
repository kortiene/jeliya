import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

function linuxProcessIdentity(pid) {
  try {
    const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
    const commandEnd = stat.lastIndexOf(") ");
    if (commandEnd < 0) throw new Error("malformed proc stat");
    const fieldsFromState = stat.slice(commandEnd + 2).trim().split(/\s+/);
    // A zombie has already exited, so its PID can no longer be recycled until
    // the parent reaps it. Treat the leader as absent and let the caller probe
    // the still-existing process group for any surviving children.
    if (fieldsFromState[0] === "Z") return null;
    const startTime = fieldsFromState[19]; // proc(5) field 22, with state at index 0.
    const bootId = readFileSync("/proc/sys/kernel/random/boot_id", "utf8").trim();
    const command = readFileSync(`/proc/${pid}/cmdline`)
      .toString("utf8")
      .split("\0")
      .filter(Boolean)
      .join(" ");
    if (!startTime || !bootId || !command) throw new Error("incomplete proc identity");
    return `linux:${bootId}:${startTime}:${command}`;
  } catch (error) {
    if (error?.code === "ENOENT" || error?.code === "ESRCH") return null;
    throw new Error(`could not inspect Linux process ${pid}`);
  }
}

export function readProcessIdentity(pid) {
  if (!Number.isInteger(pid) || pid <= 0) throw new Error(`invalid process id: ${pid}`);
  if (process.platform === "linux") return linuxProcessIdentity(pid);
  try {
    const identity = execFileSync(
      "ps",
      ["-ww", "-o", "lstart=", "-o", "command=", "-p", String(pid)],
      { encoding: "utf8" },
    ).trim();
    return identity || null;
  } catch (error) {
    if (error?.status === 1) return null;
    throw new Error(`could not inspect process ${pid}`);
  }
}

export function recordOwnedProcess(pid, { readIdentity = readProcessIdentity } = {}) {
  const identity = readIdentity(pid);
  if (!identity) throw new Error(`run-owned process ${pid} disappeared before registration`);
  return Object.freeze({ pid, identity });
}

export function signalOwnedProcessGroup(
  record,
  signal,
  {
    readIdentity = readProcessIdentity,
    signalProcess = process.kill,
  } = {},
) {
  if (!record || !Number.isInteger(record.pid) || record.pid <= 0 || !record.identity) {
    throw new Error("invalid run-owned process-group record");
  }
  const currentIdentity = readIdentity(record.pid);
  if (currentIdentity && currentIdentity !== record.identity) {
    throw new Error(`refusing to signal recycled process-group leader ${record.pid}`);
  }
  if (!currentIdentity) {
    try {
      signalProcess(-record.pid, 0);
    } catch (error) {
      if (error?.code === "ESRCH") return "already-exited";
      throw new Error(
        `failed to probe run-owned process group ${record.pid}: ${error?.code ?? "unknown"}`,
      );
    }
  }
  try {
    signalProcess(-record.pid, signal);
    return "signalled";
  } catch (error) {
    if (error?.code === "ESRCH") return "already-exited";
    throw new Error(
      `failed to signal run-owned process group ${record.pid}: ${error?.code ?? "unknown"}`,
    );
  }
}
