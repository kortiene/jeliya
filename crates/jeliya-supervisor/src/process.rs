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

use std::time::Duration;

use tokio::process::{Child, Command};

use crate::error::SupervisorError;

/// How long to wait for a SIGKILLed child to be reaped before giving up. A
/// killed process reaps effectively instantly UNLESS it is wedged in an
/// uninterruptible (`D` state) syscall — e.g. a `data_dir` on a hung
/// network/FUSE mount — where SIGKILL is delivered only when the syscall
/// returns, which may be never. Without a bound the reap `wait()` inherits that
/// hang, defeating the crate's "every wait is time-boxed" contract, so the reap
/// is capped and a stuck child is surfaced as an error rather than hanging the
/// caller forever.
const REAP_GRACE: Duration = Duration::from_secs(5);

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

/// SIGKILL the owned child's whole process group and reap it, BOUNDED. On Unix,
/// `killpg` on the child's pgid (set at spawn) tears down any descendant with
/// it; a direct `start_kill` backs it up in case the group is already gone. On
/// non-Unix, a plain kill of the child handle.
///
/// `child.id()` is the OS-assigned pid of a `Child` this process spawned and
/// still holds un-reaped, so it is always a positive `pid_t` and never needs the
/// [`SignalPid`] guard (which exists for attacker-supplied portfile PIDs).
///
/// The reap is capped at [`REAP_GRACE`]: an unkillable `D`-state child would
/// otherwise hang `wait()` forever. On expiry — or if `start_kill` itself
/// failed — the error is surfaced (no longer silently discarded) so the caller
/// learns the signal did not land instead of blocking.
pub(crate) async fn force_kill_tree(child: &mut Child) -> std::io::Result<()> {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        use nix::sys::signal::{killpg, Signal};
        use nix::unistd::Pid;
        // The child leads its own group (pgid == pid); signal the group.
        let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
    }
    // Ensure the child handle itself is signalled and then reaped, so `wait`
    // cannot block on a survivor if the group signal missed. The `start_kill`
    // result is kept: if it failed, the reap is the more likely to hang, so its
    // error is the one worth surfacing on timeout.
    let kill_result = child.start_kill();
    match tokio::time::timeout(REAP_GRACE, child.wait()).await {
        Ok(reaped) => reaped.map(|_| ()),
        Err(_elapsed) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            match kill_result {
                Ok(()) => "child was SIGKILLed but did not reap within the grace window \
                           (likely wedged in an uninterruptible syscall)"
                    .to_owned(),
                Err(e) => format!("could not signal the child ({e}) and it did not reap"),
            },
        )),
    }
}

/// Best-effort SIGKILL of a spawned leader's **process group** by its pgid
/// (`pgid == the leader's pid`, set at spawn), for the early-exit paths where the
/// leader has ALREADY been reaped — so [`force_kill_tree`] (which reads the live
/// `child.id()`) can no longer reach the group. A leader that spawned a
/// descendant and then exited leaves that descendant holding the data-dir lock in
/// the group we isolated; killing the group reclaims it before the caller retries.
///
/// The pgid must be captured from `child.id()` BEFORE the reaping `wait()`. A
/// process group persists while any member lives, so if a descendant survives the
/// group is intact and this reaches exactly it; if the group is already empty,
/// `killpg` returns `ESRCH` and is ignored. The recycled-pgid TOCTOU is the same
/// bounded, same-uid window the eviction path documents (§6.7). No-op off Unix
/// (single-process daemon; group isolation is deferred with Windows, OQ-5).
pub(crate) fn kill_reaped_process_group(pgid: u32) {
    #[cfg(unix)]
    {
        use nix::sys::signal::{killpg, Signal};
        use nix::unistd::Pid;
        if let Some(valid) = SignalPid::new(pgid) {
            let _ = killpg(Pid::from_raw(valid.get()), Signal::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
    }
}

/// A validated positive process id, safe to convert to the platform signal
/// type. Constructed only through [`SignalPid::new`], which rejects the values
/// whose `as i32` cast would change Unix signal semantics.
#[cfg(unix)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct SignalPid(i32);

#[cfg(unix)]
impl SignalPid {
    /// Reject a PID that cannot address exactly one real process before any
    /// signal is derived from it. On Unix, `kill(pid, …)` reinterprets its
    /// argument by sign: `0` addresses the **caller's** whole process group and
    /// `-1`/other negatives address process *groups* — so a portfile `pid` of
    /// `0`, or one above `i32::MAX` (whose `as i32` cast wraps negative), would
    /// broadcast SIGTERM well beyond the intended incumbent. Only `1..=i32::MAX`
    /// names a single process; everything else is refused here, before the cast
    /// (the P1 the review names).
    pub(crate) fn new(pid: u32) -> Option<Self> {
        if pid >= 1 && pid <= i32::MAX as u32 {
            Some(Self(pid as i32))
        } else {
            None
        }
    }

    /// The validated value as the raw signal argument.
    pub(crate) fn get(self) -> i32 {
        self.0
    }
}

/// Send SIGTERM to a **foreign** proven-owned incumbent (eviction fallback,
/// spec §6.7). The caller MUST have proven, via a PID-bound health probe, that
/// this PID is the daemon serving the exact data dir — this function does not
/// re-check; it is the raw lever the eviction gate guards. Graceful (SIGTERM,
/// never SIGKILL): an adopted/incumbent daemon runs its own teardown.
///
/// The PID is validated positive-and-representable before the signal is derived
/// ([`SignalPid::new`]): `kill(0, …)`/negative wraps would signal a whole
/// process group, so a `0`/overflowing portfile PID is refused rather than
/// cast.
///
/// Unix only. On other platforms a foreign process cannot be signalled without
/// forbidden unsafe FFI, so eviction there requires the caller-supplied
/// `daemon.stop` RPC (deferred with Windows packaging, OQ-4/OQ-5).
pub(crate) fn sigterm_foreign(pid: u32) -> Result<(), SupervisorError> {
    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        let signal_pid = SignalPid::new(pid).ok_or_else(|| {
            SupervisorError::Handshake(format!(
                "refusing to signal invalid incumbent pid {pid} (not a positive, representable process id)"
            ))
        })?;
        kill(Pid::from_raw(signal_pid.get()), Signal::SIGTERM).map_err(|errno| {
            SupervisorError::Handshake(format!("could not SIGTERM incumbent pid {pid}: {errno}"))
        })
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Err(SupervisorError::Handshake(
            "signal-based eviction is unavailable off Unix; supply a daemon.stop RPC".to_owned(),
        ))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn signal_pid_rejects_zero() {
        // `kill(0, sig)` signals the CALLER's whole process group — never a
        // proven incumbent. A portfile PID of 0 must be refused before the cast.
        assert!(SignalPid::new(0).is_none());
    }

    #[test]
    fn signal_pid_rejects_values_above_i32_max() {
        // `(u32 as i32)` wraps negative for anything over `i32::MAX`, and a
        // negative kill target addresses a process GROUP. Refuse the whole
        // overflowing range.
        assert!(SignalPid::new(i32::MAX as u32 + 1).is_none());
        assert!(SignalPid::new(u32::MAX).is_none());
    }

    #[test]
    fn signal_pid_accepts_the_representable_positive_range() {
        assert_eq!(SignalPid::new(1).map(SignalPid::get), Some(1));
        assert_eq!(
            SignalPid::new(i32::MAX as u32).map(SignalPid::get),
            Some(i32::MAX)
        );
        // A plausible real PID round-trips unchanged.
        assert_eq!(SignalPid::new(4242).map(SignalPid::get), Some(4242));
    }

    #[test]
    fn sigterm_foreign_refuses_pid_zero_without_signalling() {
        // The public lever refuses pid 0 with a Handshake error rather than
        // calling `kill(0, …)` (which would SIGTERM this test's own process
        // group and abort the run). Reaching this assertion at all proves the
        // guard fired before any signal.
        let err = sigterm_foreign(0).expect_err("pid 0 must be refused");
        assert!(matches!(err, SupervisorError::Handshake(_)));
    }
}
