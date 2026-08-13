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
    // Capture the group BEFORE the reaping `wait()`: once the leader is reaped
    // `child.id()` is `None`, so the group (needed for the post-reap existence
    // check) can no longer be recovered.
    #[cfg(unix)]
    let group = child.id().map(|pid| nix::unistd::Pid::from_raw(pid as i32));
    #[cfg(unix)]
    if let Some(group) = group {
        use nix::sys::signal::{killpg, Signal};
        // The child leads its own group (pgid == pid); signal the group.
        let _ = killpg(group, Signal::SIGKILL);
    }
    // Ensure the child handle itself is signalled and then reaped, so `wait`
    // cannot block on a survivor if the group signal missed. The `start_kill`
    // result is kept: if it failed, the reap is the more likely to hang, so its
    // error is the one worth surfacing on timeout.
    let kill_result = child.start_kill();
    match tokio::time::timeout(REAP_GRACE, child.wait()).await {
        Ok(reaped) => {
            reaped?;
            // The LEADER reaped, but a descendant wedged in an uninterruptible
            // syscall may still live in the isolated group, holding the data-dir
            // lock. Verify the group is gone (bounded) before reporting success, so
            // a caller never treats teardown as complete over a surviving subtree.
            #[cfg(unix)]
            if let Some(group) = group {
                if !wait_group_gone(group).await {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "child reaped but its process group did not terminate within the \
                         cleanup window (a descendant is likely wedged in an uninterruptible \
                         syscall, still holding the data-dir lock)",
                    ));
                }
            }
            Ok(())
        }
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

/// SYNCHRONOUSLY SIGKILL a freshly-spawned leader's **process group**, for the
/// LATE-SPAWN cleanup path: when the spawn deadline fired and the caller has
/// already returned `TimedOut`, the blocking spawn worker itself tears the child
/// down HERE, on its own blocking thread. A runtime shutdown WAITS for a blocking
/// task, so this cleanup survives a caller that drops its runtime — a detached
/// async task would instead be cancelled, and the late [`Child`] (with
/// `kill_on_drop(false)`) would leak its group. The child leads its own group
/// (`pgid == pid`), so one `killpg(SIGKILL)` reaches any descendant a late exec
/// already spawned before it can survive holding the `data_dir` lock. Synchronous
/// (needs no runtime) and best-effort: the timed-out caller has already returned,
/// and a `D`-state straggler is unkillable until its syscall returns (the same
/// bound [`force_kill_tree`] documents). Reaping the dead leader is delegated to
/// the runtime's child reaper, or to init once this supervisor exits — a zombie
/// holds no lock (its fd closed at death). Off Unix (no process-group kill) it
/// falls back to a direct `start_kill` of the child handle, so the late daemon is
/// still terminated rather than leaked.
pub(crate) fn force_kill_group_blocking(child: Child) {
    #[cfg(unix)]
    {
        // Signal via `nix` directly — NOT `tokio`'s `start_kill`/`kill`, which need
        // a live runtime: this runs on a detached OS thread whose caller may already
        // have DROPPED the runtime (the whole reason the spawn runs off the blocking
        // pool). `killpg` reaches the leader AND any descendant a late exec spawned;
        // a direct `kill` of the leader backs it up if `killpg` failed (its result is
        // best-effort). Both are plain syscalls, valid with or without a runtime.
        if let Some(pid) = child.id() {
            use nix::sys::signal::{kill, killpg, Signal};
            use nix::unistd::Pid;
            let target = Pid::from_raw(pid as i32);
            let _ = killpg(target, Signal::SIGKILL);
            let _ = kill(target, Signal::SIGKILL);
        }
        // `child` drops here: its fds close (the lock releases at death) and the
        // runtime's reaper (if still alive) collects the SIGKILLed leader.
        drop(child);
    }
    #[cfg(not(unix))]
    {
        // Windows Job-Object teardown is deferred (OQ-5); best-effort direct kill.
        let mut child = child;
        let _ = child.start_kill();
        drop(child);
    }
}

/// A freshly-spawned [`Child`] whose process GROUP is torn down if this guard is
/// dropped WITHOUT being consumed — the cancellation-safe ownership the late-spawn
/// hand-off needs. A oneshot `send` only acknowledges that the value was QUEUED, not
/// that the caller consumed it: if the receiver is dropped after a successful send
/// (the caller aborted `start_or_adopt`), the queued child would otherwise be
/// dropped with `kill_on_drop(false)` and leak its group and data-dir lock. On the
/// happy path the consumer calls [`SpawnGuard::into_child`] to TAKE the child,
/// disarming the guard; any other drop path (receiver gone, timeout) runs
/// [`force_kill_group_blocking`] synchronously via `Drop`.
pub(crate) struct SpawnGuard(Option<Child>);

impl SpawnGuard {
    pub(crate) fn new(child: Child) -> Self {
        Self(Some(child))
    }

    /// Disarm the guard and take ownership of the child — the caller is now
    /// responsible for its lifecycle (the normal spawn path).
    pub(crate) fn into_child(mut self) -> Child {
        self.0.take().expect("SpawnGuard child taken exactly once")
    }

    /// Borrow the guarded child WITHOUT disarming, so startup validation (reading
    /// the announcement, the portfile, and the health/PID gate) can hold the child
    /// under the guard until an owned [`crate::Sidecar`] actually takes ownership.
    /// If the enclosing future is dropped mid-validation, the still-armed guard
    /// SIGKILLs the child's whole group — an overridden/hung child that ignores its
    /// stdin-EOF parent-death signal (and any descendant in its group) cannot
    /// survive holding the data-dir lock.
    pub(crate) fn as_mut(&mut self) -> &mut Child {
        self.0
            .as_mut()
            .expect("SpawnGuard child present until taken")
    }
}

impl Drop for SpawnGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.take() {
            force_kill_group_blocking(child);
        }
    }
}

/// SIGKILL a spawned leader's **process group** by its pgid (`pgid == the
/// leader's pid`, set at spawn) and VERIFY, bounded, that the group is gone —
/// for the early-exit paths where the leader has ALREADY been reaped, so
/// [`force_kill_tree`] (which reads the live `child.id()`) can no longer reach the
/// group. A leader that spawned a descendant and then exited leaves that
/// descendant holding the data-dir lock in the group we isolated; killing the
/// group reclaims it before the caller retries.
///
/// The pgid must be captured from `child.id()` BEFORE the reaping `wait()`. A
/// process group persists while any member lives, so if a descendant survives the
/// group is intact and this reaches exactly it. After the signal it polls the
/// group's existence (`killpg(_, None)` → `ESRCH` once empty) up to [`REAP_GRACE`]
/// and returns [`SupervisorError::GroupCleanupTimedOut`] if a member outlives the
/// bound — no longer silently discarding the result, so a caller never reports a
/// clean stop over a surviving subtree. An already-empty group returns `Ok`
/// immediately. The recycled-pgid TOCTOU is the same bounded, same-uid window the
/// eviction path documents (§6.7). No-op (always `Ok`) off Unix (single-process
/// daemon; group isolation is deferred with Windows, OQ-5).
pub(crate) async fn kill_reaped_process_group(pgid: u32) -> Result<(), SupervisorError> {
    #[cfg(unix)]
    {
        use nix::sys::signal::{killpg, Signal};
        use nix::unistd::Pid;
        let Some(valid) = SignalPid::new(pgid) else {
            // An invalid pgid names no group (and must never be signalled — the
            // `SignalPid` guard); there is nothing to reclaim.
            return Ok(());
        };
        let group = Pid::from_raw(valid.get());
        let _ = killpg(group, Signal::SIGKILL);
        if wait_group_gone(group).await {
            Ok(())
        } else {
            Err(SupervisorError::GroupCleanupTimedOut { pgid })
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
        Ok(())
    }
}

/// Poll a process GROUP's existence until it is empty — `killpg(group, None)`
/// (the null signal, which delivers nothing) returns `ESRCH` once no member
/// remains — or [`REAP_GRACE`] expires. Returns `true` iff the group disappeared
/// within the bound. `SIGKILL` only QUEUES for a descendant wedged in an
/// uninterruptible (`D`-state) syscall — e.g. a `data_dir` on a hung mount — which
/// receives it only when the syscall returns (maybe never), so BOTH reaped-group
/// cleanup paths ([`kill_reaped_process_group`] and [`force_kill_tree`]) share this
/// check to avoid reporting a clean stop over a live subtree still holding the
/// data-dir lock.
#[cfg(unix)]
async fn wait_group_gone(group: nix::unistd::Pid) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::killpg;
    let deadline = tokio::time::Instant::now() + REAP_GRACE;
    loop {
        // Deadline BEFORE the probe: a naturally-late `sleep` resume can land past
        // the deadline, and an `ESRCH` observed only then is out-of-budget cleanup —
        // accepting it would let `force_kill_tree`/`kill_reaped_process_group`
        // suppress the documented `GroupCleanupTimedOut` and report a subtree
        // reclaimed after the bound.
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        if matches!(killpg(group, None), Err(Errno::ESRCH)) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
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

    /// The bounded sweep CONFIRMS an already-empty group is gone (returns `Ok`)
    /// rather than firing and returning blind. A child spawned as its own group
    /// leader and then reaped leaves an empty group, so the existence poll sees
    /// `ESRCH` immediately. An invalid pgid (0) is a no-op `Ok` (nothing to
    /// reclaim, and `SignalPid` refuses to signal it). The surviving-group timeout
    /// path is not deterministically reproducible in a test — `SIGKILL` cannot be
    /// caught — so it is covered structurally by the bounded poll.
    #[cfg(unix)]
    #[test]
    fn kill_reaped_process_group_confirms_an_empty_group_is_gone() {
        use std::os::unix::process::CommandExt;
        // A child that is its OWN group leader (pgid == pid); reap it so the group
        // is empty.
        let mut child = std::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .process_group(0)
            .spawn()
            .expect("spawn a child in its own group");
        let pgid = child.id();
        child.wait().expect("reap the child");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(
            rt.block_on(kill_reaped_process_group(pgid)).is_ok(),
            "an empty (reaped) group must be confirmed gone within the bound"
        );
        assert!(
            rt.block_on(kill_reaped_process_group(0)).is_ok(),
            "an invalid pgid is a no-op Ok (nothing to reclaim)"
        );
    }

    /// `force_kill_tree` SIGKILLs the child's group, reaps the leader, AND confirms
    /// (bounded) the group is gone before returning `Ok`. Here a plain long-lived
    /// child dies, so the post-reap group check passes. The surviving-group `Err`
    /// path is not deterministically reproducible (`SIGKILL` cannot be caught), so
    /// it is covered structurally by the shared `wait_group_gone` poll.
    #[cfg(unix)]
    #[test]
    fn force_kill_tree_kills_a_child_and_confirms_the_group_gone() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", "sleep 600"]).kill_on_drop(false);
            configure_new_process_group(&mut cmd);
            let mut child = cmd.spawn().expect("spawn a long-lived child");
            let result = force_kill_tree(&mut child).await;
            assert!(
                result.is_ok(),
                "killing a plain child and confirming its group gone must succeed; got: {result:?}"
            );
        });
    }

    /// The synchronous late-spawn cleanup kills the child: `killpg` reaches its
    /// group AND the direct `start_kill` fallback covers a failed `killpg` / a
    /// non-Unix build, so a child dropped with `kill_on_drop(false)` cannot survive.
    #[cfg(unix)]
    #[test]
    fn force_kill_group_blocking_kills_a_child() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", "sleep 600"]).kill_on_drop(false);
            configure_new_process_group(&mut cmd);
            let child = cmd.spawn().expect("spawn a long-lived child");
            let pid = child.id().expect("child has a pid") as i32;
            force_kill_group_blocking(child);
            // The child is SIGKILLed; poll until it is gone (reaped by the runtime).
            use nix::sys::signal::kill;
            use nix::unistd::Pid;
            let target = Pid::from_raw(pid);
            let mut gone = false;
            for _ in 0..200 {
                if matches!(kill(target, None), Err(nix::errno::Errno::ESRCH)) {
                    gone = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(
                gone,
                "force_kill_group_blocking must kill the child's process"
            );
        });
    }

    /// A `SpawnGuard` dropped WITHOUT `into_child` tears down the child's group —
    /// the cancellation-safe cleanup for a late child the caller never consumed
    /// (the oneshot queued it, then the receiver was dropped).
    #[cfg(unix)]
    #[test]
    fn spawn_guard_kills_the_child_when_dropped_unconsumed() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", "sleep 600"]).kill_on_drop(false);
            configure_new_process_group(&mut cmd);
            let child = cmd.spawn().expect("spawn a long-lived child");
            let pid = child.id().expect("child has a pid") as i32;
            // Wrap and drop WITHOUT consuming — the guard's Drop must kill the group.
            drop(SpawnGuard::new(child));
            use nix::sys::signal::kill;
            use nix::unistd::Pid;
            let target = Pid::from_raw(pid);
            let mut gone = false;
            for _ in 0..200 {
                if matches!(kill(target, None), Err(nix::errno::Errno::ESRCH)) {
                    gone = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(gone, "an unconsumed SpawnGuard must kill the child on drop");
        });
    }

    /// Borrowing the child through [`SpawnGuard::as_mut`] must NOT disarm the guard:
    /// startup validation (announcement/portfile/health) borrows the child while it
    /// stays force-killable, so a cancel mid-validation still tears the group down.
    /// Fails if `as_mut` ever consumed the child (e.g. `take`) — the group would then
    /// survive the drop.
    #[test]
    fn spawn_guard_as_mut_borrows_without_disarming() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", "sleep 600"]).kill_on_drop(false);
            configure_new_process_group(&mut cmd);
            let child = cmd.spawn().expect("spawn a long-lived child");
            let pid = child.id().expect("child has a pid") as i32;
            let mut guard = SpawnGuard::new(child);
            // Borrow the child the way startup validation does — must not disarm.
            assert_eq!(guard.as_mut().id(), Some(pid as u32));
            let _ = guard.as_mut().id();
            // Drop the STILL-ARMED guard: the group must be killed despite the borrows.
            drop(guard);
            use nix::sys::signal::kill;
            use nix::unistd::Pid;
            let target = Pid::from_raw(pid);
            let mut gone = false;
            for _ in 0..200 {
                if matches!(kill(target, None), Err(nix::errno::Errno::ESRCH)) {
                    gone = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(
                gone,
                "a SpawnGuard borrowed via as_mut must stay armed and kill on drop"
            );
        });
    }
}
