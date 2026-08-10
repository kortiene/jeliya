import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

function parseProcState(stat) {
  const commandEnd = stat.lastIndexOf(") ");
  if (commandEnd < 0) throw new Error("malformed proc stat");
  return stat.slice(commandEnd + 2).trim().split(/\s+/);
}

export function linuxProcessIdentity(
  pid,
  {
    readStat = (p) => readFileSync(`/proc/${p}/stat`, "utf8"),
    readCmdline = (p) => readFileSync(`/proc/${p}/cmdline`),
    readBootId = () => readFileSync("/proc/sys/kernel/random/boot_id", "utf8"),
  } = {},
) {
  try {
    const fieldsFromState = parseProcState(readStat(pid));
    // A dead leader has already exited: a zombie (Z) cannot have its PID
    // recycled until the parent reaps it, and the kernel dead states (X, x)
    // are the final instants of the same exit path. Treat the leader as absent
    // and let the caller probe the still-existing process group for any
    // surviving children.
    if (fieldsFromState[0] === "Z" || fieldsFromState[0] === "X" || fieldsFromState[0] === "x") return null;
    const startTime = fieldsFromState[19]; // proc(5) field 22, with state at index 0.
    const bootId = readBootId().trim();
    const command = readCmdline(pid)
      .toString("utf8")
      .split("\0")
      .filter(Boolean)
      .join(" ");
    if (!startTime || !bootId) throw new Error("incomplete proc identity");
    if (!command) {
      // The stat read passed the dead-state guard, but the cmdline came back
      // empty. A process we own always has a cmdline while it is alive, so an
      // empty one means the leader vanished in the ~20 ms between the two
      // reads. Disambiguate with a second stat read:
      //  - reaped: ENOENT/ESRCH -> caught below as absence;
      //  - zombie or kernel dead state (Z, X, x): exited, PID not yet
      //    recyclable -> absence;
      //  - start time changed: the PID was RECYCLED to a new task, so -pid now
      //    names an unrelated process group. Absence would invite the caller
      //    to probe and signal it; surface the new occupant's identity instead
      //    so the caller's recycled-leader guard refuses to signal.
      //  - same live identity yet no cmdline: a genuine inspection failure,
      //    stays loud.
      const recheck = parseProcState(readStat(pid));
      if (recheck[0] === "Z" || recheck[0] === "X" || recheck[0] === "x") return null;
      if (recheck[19] !== startTime) return `linux:${bootId}:${recheck[19]}:`;
      throw new Error("incomplete proc identity");
    }
    return `linux:${bootId}:${startTime}:${command}`;
  } catch (error) {
    if (error?.code === "ENOENT" || error?.code === "ESRCH") return null;
    throw new Error(`could not inspect Linux process ${pid}`);
  }
}

export function readProcessIdentity(pid, deps = {}) {
  if (!Number.isInteger(pid) || pid <= 0) throw new Error(`invalid process id: ${pid}`);
  if (process.platform === "linux") return linuxProcessIdentity(pid, deps);
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
