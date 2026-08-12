//! Per-platform process-tree safety: spawn the daemon into its own process
//! group / (future) Job Object, and escalate a hung shutdown to the whole group
//! rather than a lone PID.
//!
//! The #159 spike used a bare `child.kill()` and set no process group at all,
//! so any grandchild it spawned would orphan. This module is the "process-tree
//! safe per platform" delta (spec §6.3 / §6.9 / R1). All signalling goes through
//! `nix`'s safe wrappers, so the crate keeps `unsafe_code = "forbid"`.
//!
//! Windows: full Job-Object teardown needs unsafe FFI (`windows`/`winapi`),
//! which is forbidden here, so it is formally deferred with the rest of Windows
//! packaging (OQ-5). On non-Unix the fallback is a direct `child.kill()`; the
//! current daemon is a single process (it spawns no descendants), so this is
//! functionally correct today and the group escalation is future-proofing.

use tokio::process::{Child, Command};

use crate::error::SupervisorError;

/// Put the child in its **own** process group at spawn (`pgid == child pid` on
/// Unix), so it is isolated from the supervisor's controlling-terminal signals
/// and a later group-wide signal reaches its whole subtree. Pure std, no unsafe.
pub(crate) fn configure_new_process_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }
    #[cfg(not(unix))]
    {
        // Windows Job-Object isolation is deferred (OQ-5); nothing to configure
        // without forbidden unsafe FFI.
        let _ = cmd;
    }
}

/// SIGKILL the owned child's whole process group and reap it. On Unix, `killpg`
/// on the child's pgid (set at spawn) tears down any descendant with it; a
/// direct `start_kill` backs it up in case the group is already gone. On
/// non-Unix, a plain kill of the child handle.
pub(crate) async fn force_kill_tree(child: &mut Child) -> std::io::Result<()> {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        use nix::sys::signal::{killpg, Signal};
        use nix::unistd::Pid;
        // The child leads its own group (pgid == pid); signal the group.
        let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
    }
    // Ensure the child handle itself is signalled and then reaped, so `wait`
    // cannot block on a survivor if the group signal missed.
    let _ = child.start_kill();
    child.wait().await.map(|_| ())
}

/// Send SIGTERM to a **foreign** proven-owned incumbent (eviction fallback,
/// spec §6.7). The caller MUST have proven, via a PID-bound health probe, that
/// this PID is the daemon serving the exact data dir — this function does not
/// re-check; it is the raw lever the eviction gate guards. Graceful (SIGTERM,
/// never SIGKILL): an adopted/incumbent daemon runs its own teardown.
///
/// Unix only. On other platforms a foreign process cannot be signalled without
/// forbidden unsafe FFI, so eviction there requires the caller-supplied
/// `daemon.shutdown` RPC (deferred with Windows packaging, OQ-4/OQ-5).
pub(crate) fn sigterm_foreign(pid: u32) -> Result<(), SupervisorError> {
    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        kill(Pid::from_raw(pid as i32), Signal::SIGTERM).map_err(|errno| {
            SupervisorError::Handshake(format!("could not SIGTERM incumbent pid {pid}: {errno}"))
        })
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Err(SupervisorError::Handshake(
            "signal-based eviction is unavailable off Unix; supply a daemon.shutdown RPC"
                .to_owned(),
        ))
    }
}
