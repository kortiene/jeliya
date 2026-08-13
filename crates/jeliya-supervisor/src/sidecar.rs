//! The process-ownership handle: a live daemon this process either started
//! (`Owned`) or adopted (`Adopted`).
//!
//! [`Sidecar`] is deliberately **not `Clone`** — it owns the child process and
//! the stdin parent-death pipe, and only it can stop a daemon (and only an owned
//! one, through a signal). A transport that only needs to dial holds a cheap
//! [`crate::TargetResolver`] instead, obtained via [`Sidecar::target_resolver`],
//! and can therefore never kill the process.
//!
//! The `Owned`/`Adopted` split is the whole point (spec §5.1): a supervisor that
//! stops "the daemon" without knowing whether it started it will stop someone
//! else's. An adopted daemon is left running by [`Sidecar::shutdown`]; the only
//! lever that reaches it is the protocol `daemon.stop`
//! ([`Sidecar::stop_adopted`]), never a signal.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use tokio::process::{Child, ChildStdin};

use crate::error::{BoxFuture, CallerRpcError, SupervisorError};
use crate::generation::Generation;
use crate::portfile::{self, Portfile};
use crate::process;
use crate::supervisor::Timeouts;
use crate::target::{DialTarget, TargetResolver};
use crate::validate;

/// How this process came to have a daemon — and therefore what it owes that
/// daemon at shutdown.
pub(crate) enum Ownership {
    /// We started it. Closing its stdin is the documented `--supervised`
    /// shutdown signal; `child`/`stdin` are held for the daemon's whole life.
    Owned {
        child: Child,
        /// Held open for the daemon's life. Dropping it IS the parent-death
        /// signal (stdin EOF), so it must never be dropped early.
        stdin: Option<ChildStdin>,
    },
    /// It was already serving this data dir. We are a guest; we do not get to
    /// end its life with a signal.
    Adopted,
}

impl fmt::Debug for Ownership {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Owned { .. } => f.write_str("Owned"),
            Self::Adopted => f.write_str("Adopted"),
        }
    }
}

/// A live daemon this process can talk to.
pub struct Sidecar {
    pub(crate) portfile: Portfile,
    pub(crate) ownership: Ownership,
    pub(crate) data_dir: PathBuf,
    pub(crate) expected: Generation,
    pub(crate) strict_portfile_perms: bool,
    pub(crate) timeouts: Timeouts,
    /// For an ADOPTED daemon: a handle to the `daemon.lock` inode captured AT
    /// ADOPTION, when the daemon was proven alive and holding it. `stop_adopted`
    /// polls THIS handle (the original inode) for the daemon's exit, so a cleanup
    /// tool that unlinks/replaces `daemon.lock` between adoption and shutdown cannot
    /// substitute an unrelated process's lock — a re-open at shutdown could. `None`
    /// for an owned daemon (stopped by its stdin, not a lock probe) or when the
    /// capture missed (a stall / an already-free lock), which fails closed.
    pub(crate) adopted_lock: Option<fd_lock::RwLock<std::fs::File>>,
}

impl fmt::Debug for Sidecar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The portfile's `Debug` already redacts the token; still, keep the
        // Sidecar surface minimal and stable.
        f.debug_struct("Sidecar")
            .field("ownership", &self.ownership)
            .field("data_dir", &self.data_dir)
            .field("pid", &self.portfile.pid)
            .field("port", &self.portfile.port)
            .field("expected", &self.expected)
            .finish_non_exhaustive()
    }
}

impl Sidecar {
    /// True iff this process started the daemon and may therefore stop it with a
    /// signal. An adopted daemon returns `false`.
    pub fn is_owned(&self) -> bool {
        matches!(self.ownership, Ownership::Owned { .. })
    }

    /// A cheap, cloneable resolver bound to this daemon's data dir and the
    /// supervisor's expected generation. Hand it to the transport (#172); it
    /// re-reads and re-validates the portfile on every dial.
    pub fn target_resolver(&self) -> TargetResolver {
        TargetResolver {
            data_dir: self.data_dir.clone(),
            expected: self.expected,
            strict_portfile_perms: self.strict_portfile_perms,
            timeouts: self.timeouts,
        }
    }

    /// Stop an owned daemon (bounded, process-tree-safe) and report which
    /// teardown occurred; leave an adopted daemon running ([`Teardown::LeftRunning`]).
    ///
    /// Owned path: drop stdin → EOF → the daemon removes its portfile and exits
    /// gracefully within the teardown budget → [`Teardown::Graceful`]. On
    /// timeout, escalate to a SIGKILL of the child's whole process group →
    /// [`Teardown::Forced`] (no cleanup ran, so a stale `daemon.json` may
    /// remain, which the next start's health check discards).
    pub async fn shutdown(self) -> Result<Teardown, SupervisorError> {
        // Borrowed before the `&mut self.ownership` match so the portfile path is
        // available inside the arms (disjoint field).
        let data_dir = self.data_dir.clone();
        match self.ownership {
            Ownership::Adopted => Ok(Teardown::LeftRunning),
            Ownership::Owned { child, mut stdin } => {
                // Keep a cancellation-safe group-kill guard armed for the WHOLE owned
                // teardown. If `shutdown()` is cancelled/aborted after stdin is closed
                // but while `child.wait()` is pending (e.g. the application runtime is
                // torn down), dropping the guard SIGKILLs the child's process group —
                // so a hung or overridden daemon that ignores its stdin-EOF signal, and
                // any descendant in its group, cannot survive holding the data-dir lock.
                // It is disarmed implicitly once the child is reaped (id `None` → the
                // Drop kill is then a no-op); `as_mut` borrows without disarming.
                let mut guard = process::SpawnGuard::new(child);
                // Time-box the WHOLE owned teardown — portfile probes included —
                // within one `teardown`. Start the deadline FIRST so the metadata
                // probes below count against it and cannot run unbounded on a
                // stalled mount before the timeout exists (mirrors the adopted path).
                let deadline = validate::deadline_from(self.timeouts.teardown);
                // Snapshot portfile presence BEFORE the shutdown signal: `Graceful`
                // requires a present→absent TRANSITION (the daemon's final step),
                // not mere absence — an already-absent `daemon.json` (external
                // delete / prior partial shutdown) does not prove this run's
                // cleanup completed, and jeliyad treats a missing portfile during
                // removal as success. BOUNDED and OFF the executor (`try_exists` is a
                // `stat` that blocks on a stalled mount and would otherwise keep
                // stdin open, leaving the daemon running).
                let portfile_present_before = validate::portfile_present(&data_dir, deadline).await;
                // Closing stdin is the graceful `--supervised` signal.
                drop(stdin.take());
                // Capture the pgid BEFORE the reaping wait: once `wait()` reaps the
                // leader, `child.id()` is None and the isolated group can no longer
                // be reached by pid. An overridden/future daemon could spawn a
                // long-lived descendant into the group and then exit on stdin EOF;
                // the reaped arms must sweep the group so a descendant cannot linger
                // holding locks / writing state (and, if the leader removed
                // `daemon.json`, be reported `Graceful`). The `Err(_elapsed)` arm
                // force-kills the un-reaped child, so it already reaches the group.
                let leader_pgid = guard.as_mut().id();
                // Bound the reaping wait by the budget REMAINING after the presence
                // probe, so the probe's time counts against `teardown` rather than
                // on top of it.
                let wait_budget = deadline.saturating_duration_since(tokio::time::Instant::now());
                match tokio::time::timeout(wait_budget, guard.as_mut().wait()).await {
                    // A zero exit is NOT itself proof of completed cleanup: the
                    // daemon discards its room-close result and only LOGS a failed
                    // `daemon.json` removal, so it can exit 0 with cleanup
                    // incomplete. `Graceful` is promised only when the portfile is
                    // actually gone (its removal is the daemon's final step); a
                    // lingering portfile means cleanup did not finish → `Forced`.
                    Ok(Ok(status)) if status.success() => {
                        // Sweep the isolated group and CONFIRM, bounded, that it is
                        // gone. A descendant that outlived the reaped leader may
                        // still hold the data-dir lock — which would BLOCK the next
                        // start — so its survival is a teardown FAILURE, not a
                        // `Forced` the caller can retry over. Propagate it.
                        if let Some(pgid) = leader_pgid {
                            process::kill_reaped_process_group(pgid).await?;
                        }
                        // `Graceful` ONLY on a present→absent transition: the
                        // portfile was there before the signal and is confirmed
                        // gone now (its removal is the daemon's final step). An
                        // already-absent portfile, a lingering one, or a stat error
                        // (unreadable dir) is `Forced` — cleanup is not proven. This
                        // probe is BOUNDED and OFF the executor too (a stalled mount
                        // must not hang here after `child.wait()` completed).
                        let removed = validate::portfile_absent(&data_dir, deadline).await;
                        if portfile_present_before && removed {
                            Ok(Teardown::Graceful)
                        } else {
                            Ok(Teardown::Forced)
                        }
                    }
                    // The daemon exited abnormally (crashed / was externally
                    // killed) before or during shutdown. Its cleanup may not have
                    // run — a stale `daemon.json` or half-written state may remain
                    // — so this is NOT graceful; report `Forced`, whose contract
                    // already warns that cleanup did not necessarily run.
                    Ok(Ok(_status)) => {
                        // A group that survived SIGKILL past the grace window (a
                        // descendant still holding locks / writing state) is a
                        // teardown FAILURE the caller must see, NOT a `Forced` it can
                        // treat as complete — propagate the verified failure rather
                        // than discarding it.
                        if let Some(pgid) = leader_pgid {
                            process::kill_reaped_process_group(pgid).await?;
                        }
                        Ok(Teardown::Forced)
                    }
                    Ok(Err(e)) => {
                        // The wait itself failed while the child (and its isolated
                        // group) may still be alive. `kill_on_drop(false)` means
                        // nothing else reaps them, so force-kill the group like the
                        // timeout arm — a faulty daemon must not linger holding the
                        // data-dir lock after `shutdown` errors. Surface the wait
                        // error, folding in any cleanup failure.
                        match process::force_kill_tree(guard.as_mut()).await {
                            Ok(()) => Err(SupervisorError::Spawn(e)),
                            Err(kill_err) => Err(SupervisorError::Spawn(std::io::Error::new(
                                e.kind(),
                                format!(
                                    "child.wait() failed ({e}); group cleanup also failed ({kill_err})"
                                ),
                            ))),
                        }
                    }
                    Err(_elapsed) => {
                        process::force_kill_tree(guard.as_mut())
                            .await
                            .map_err(SupervisorError::Spawn)?;
                        Ok(Teardown::Forced)
                    }
                }
            }
        }
    }

    /// Stop an **adopted** daemon through the protocol, using a caller-supplied
    /// `daemon.stop` invoker (the supervisor does not link the client). No
    /// signal is ever sent — an adopted daemon is never SIGKILLed by the client.
    /// Polls `/api/health` until the daemon goes dark, bounded; on timeout,
    /// [`SupervisorError::ShutdownTimedOut`].
    ///
    /// The invoker is handed the [`DialTarget`] of the REVALIDATED incarnation (the
    /// re-read portfile that just matched the adopted pid+port+token), so its request
    /// is bound to that daemon's port AND per-start bearer. A caller MUST dial the
    /// passed target rather than re-resolving through [`TargetResolver`]: if the
    /// adopted daemon exits between the identity check and the RPC and a replacement
    /// takes the recycled port, a fresh resolve would hand back the replacement's new
    /// token — the passed target carries the ORIGINAL token, so a request built from it
    /// cannot authenticate to (nor stop) the replacement.
    ///
    /// On an **owned** sidecar this delegates to [`Sidecar::shutdown`] (the
    /// signal path), because an owned daemon is stopped by closing its stdin,
    /// not by an RPC.
    pub async fn stop_adopted(
        self,
        shutdown_rpc: impl FnOnce(DialTarget) -> BoxFuture<'static, Result<(), CallerRpcError>>,
    ) -> Result<Teardown, SupervisorError> {
        if self.is_owned() {
            return self.shutdown().await;
        }
        let pid = self.portfile.pid;
        let port = self.portfile.port;
        // The exit proof is the `daemon.lock` handle captured AT ADOPTION — the
        // original inode the daemon holds — NOT one re-opened here at shutdown. A
        // shutdown-time re-open could open a REPLACEMENT inode (a cleanup tool
        // unlinked/replaced `daemon.lock` after adoption) that a different process
        // holds; that process releasing its unrelated lock would then read as the
        // daemon's exit. Our retained fd still refers to the inode the daemon holds,
        // so it cannot be fooled. `None` (capture missed at adoption) fails closed
        // below.
        let adopted_lock = self.adopted_lock;
        // The whole adopted stop is time-boxed by `teardown` — RPC, portfile and
        // lock probes, and all polling stages included. Start the deadline FIRST so
        // every blocking stage counts against it; the metadata probes below touch a
        // possibly-stalled mount and must not run unbounded before the deadline even
        // exists.
        let deadline = validate::deadline_from(self.timeouts.teardown);
        // Snapshot portfile presence BEFORE the RPC: the daemon removes
        // `daemon.json` as the LAST step of graceful shutdown, so a present→absent
        // transition is the completion proof below. If it is ALREADY absent here
        // (a cleanup tool removed it, or a prior partial shutdown), its removal
        // proves nothing — see the completion check. BOUNDED and OFF the executor
        // (`try_exists` is a `stat` that blocks on a stalled mount).
        let portfile_present_before = validate::portfile_present(&self.data_dir, deadline).await;
        // REVALIDATE the adopted identity before issuing the stop: if the daemon we
        // adopted already exited and a REPLACEMENT started in the same data dir, a
        // reconnecting RPC could otherwise stop the replacement we never adopted —
        // and its portfile transition, freed lock, and dark old PID would all read as
        // OUR daemon's clean exit. Re-read the CURRENT portfile (bounded, off the
        // executor) and require it to still name the SAME pid+port as the identity we
        // hold; a replacement rewrote `daemon.json` with a NEW pid and a fresh
        // `--port 0` bind, so a mismatch — or an unreadable/absent portfile — means
        // the daemon changed under us. Refuse rather than stop a foreign process.
        // (The completion proof below binds to this same pid via `wait_process_exited`.)
        // Bounded by the REMAINING teardown budget — not `read_portfile_bounded`'s own
        // fresh 5s — so a stalled mount (or an already-exhausted deadline) cannot make
        // this re-read blow past the whole-operation `teardown` bound.
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let current = match tokio::time::timeout(
            remaining,
            portfile::read_portfile_bounded(&self.data_dir, self.strict_portfile_perms),
        )
        .await
        {
            Ok(Ok(current)) => current,
            _ => return Err(SupervisorError::ShutdownTimedOut { pid }),
        };
        // STAMP the result against the deadline: `timeout` polls the ready read
        // BEFORE its timer, so a read completing just PAST the budget would otherwise
        // be accepted and let the RPC run beyond the whole-operation `teardown`.
        // Reject an out-of-budget read, and an identity that changed under us.
        //
        // pid and port are BOTH recyclable — a replacement daemon can be assigned the
        // same PID and the same ephemeral `--port 0`, so matching them alone does not
        // prove this is the daemon we adopted. `daemon.json` carries a per-start
        // 256-bit `auth_token`, which a replacement necessarily regenerates; require
        // the re-read token to equal the token we captured at adoption, so a recycled
        // pid+port cannot let the caller's RPC stop a foreign replacement.
        if tokio::time::Instant::now() >= deadline
            || current.pid != pid
            || current.port != port
            || current.token() != self.portfile.token()
        {
            return Err(SupervisorError::ShutdownTimedOut { pid });
        }
        // BIND the RPC to THIS revalidated incarnation: build its dial target (loopback
        // port + the per-start bearer from the just-matched `current` portfile) and hand
        // it to the invoker. If the daemon exits between this check and the request and a
        // replacement takes the recycled port, a caller that dials the passed target
        // still carries the ORIGINAL token — so its request cannot authenticate to the
        // replacement, closing the resolve-a-later-daemon window.
        let target = DialTarget::from_validated(&current, self.expected).map_err(|e| {
            SupervisorError::ShutdownRpcFailed(format!("could not build the stop dial target: {e}"))
        })?;
        // Ask the daemon to shut itself down over the caller's RPC, bounded by the
        // budget REMAINING after the presence probe so its time counts against
        // `teardown`, not on top of it. A caller-supplied invoker whose transport stalls must
        // not wedge shutdown just because its own future has no timeout. A failure
        // here is the dedicated `ShutdownRpcFailed` — NOT `Handshake` (a startup
        // error) — so a caller can apply shutdown-specific retry/reporting policy.
        let rpc_budget = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(rpc_budget, shutdown_rpc(target)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                // REDACT the daemon bearer from the caller-controlled error text
                // before it enters `ShutdownRpcFailed` (whose Debug/Display are
                // commonly logged): a transport that echoes its request or
                // `Authorization` header could otherwise leak the token the crate
                // contracts to keep native and never log (§5.8).
                let token = self.portfile.token();
                let mut message = e.to_string();
                if !token.is_empty() {
                    message = message.replace(token, "<redacted>");
                }
                return Err(SupervisorError::ShutdownRpcFailed(message));
            }
            Err(_elapsed) => return Err(SupervisorError::ShutdownTimedOut { pid }),
        }
        // Confirm it actually went dark; we never signalled it, so the RPC is
        // the only lever and we must verify it took effect. Bound by the budget
        // REMAINING after the RPC, so the whole adopted stop stays within one
        // `teardown` rather than one-per-stage.
        let after_rpc = deadline.saturating_duration_since(tokio::time::Instant::now());
        if !validate::wait_health_dark(pid, port, after_rpc, &self.timeouts).await {
            return Err(SupervisorError::ShutdownTimedOut { pid });
        }
        // A dark listener is not a finished shutdown: the daemon drops its
        // listener FIRST and only then closes rooms (~10s) and removes
        // `daemon.json` last. `Graceful` invites the caller to reuse or remove
        // the data dir, so wait for portfile removal — the true completion
        // signal — within the REMAINING budget before promising it. On timeout
        // the process is dark but may still hold its lock / be writing state, so
        // `ShutdownTimedOut` is the honest verdict rather than a premature
        // `Graceful`.
        //
        // Removal is proof ONLY as a present→absent transition. If `daemon.json`
        // was already absent before the RPC, `wait_portfile_removed` returns on
        // its first poll and would promise `Graceful` while the daemon may still
        // be closing rooms / holding its lock — the very premature verdict this
        // guard exists to avoid. With no process handle for an adopted daemon and
        // the listener already dark, completion cannot be proven in that case, so
        // the honest verdict is `ShutdownTimedOut`.
        if !portfile_present_before {
            return Err(SupervisorError::ShutdownTimedOut { pid });
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if !validate::wait_portfile_removed(&self.data_dir, remaining).await {
            return Err(SupervisorError::ShutdownTimedOut { pid });
        }
        // Portfile removal is NOT the daemon's final act: it removes the portfile,
        // THEN flushes logs and only then `process::exit`s (jeliyad `main.rs`),
        // holding `daemon.lock` until exit. `Graceful` invites the caller to reuse
        // the dir, so confirm the daemon FULLY EXITED before promising it, with TWO
        // proofs:
        //   1. the `daemon.lock` (captured at adoption) is RELEASED — the dir is
        //      reuse-safe, the direct proof a restart will not wedge on the lock; and
        //   2. on Unix, the ADOPTED PID has EXITED (`kill(pid, 0)` → `ESRCH`) — an
        //      IDENTITY-bound proof that THIS daemon, not an unrelated process that
        //      happened to hold a replacement lock inode, is the one that is gone. A
        //      lock-path swap can spoof (1) but never (2).
        // A missing lock handle or a still-live PID fails closed to `ShutdownTimedOut`
        // rather than a premature `Graceful`. Off Unix (deferred), the lock is the
        // only available signal.
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let lock_released = validate::wait_lock_handle_released(adopted_lock, remaining).await;
        let identity_exited = if cfg!(unix) {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            validate::wait_process_exited(pid, remaining).await
        } else {
            true
        };
        if lock_released && identity_exited {
            Ok(Teardown::Graceful)
        } else {
            Err(SupervisorError::ShutdownTimedOut { pid })
        }
    }

    /// The advertised daemon PID (diagnostics; identity is bound by the health
    /// proof, not by this value alone).
    pub fn pid(&self) -> u32 {
        self.portfile.pid
    }

    /// The advertised loopback port.
    pub fn port(&self) -> u16 {
        self.portfile.port
    }
}

/// What a [`Sidecar::shutdown`] / [`Sidecar::stop_adopted`] actually did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Teardown {
    /// Adopted daemon: nothing was done, deliberately.
    LeftRunning,
    /// The daemon exited within its budget (stdin EOF for owned, `daemon.stop`
    /// for adopted). It ran its own cleanup and removed its portfile.
    Graceful,
    /// Owned daemon: cleanup did NOT necessarily run. Either the graceful path
    /// timed out and the process group was SIGKILLed, or the daemon exited
    /// abnormally (crash / external kill) on its own. A stale `daemon.json` or
    /// half-written state may remain (the next start's health check discards a
    /// stale portfile).
    Forced,
}

impl Teardown {
    /// Whether this call ended a daemon at all.
    pub fn stopped_something(self) -> bool {
        !matches!(self, Self::LeftRunning)
    }
}

/// The teardown budget default (the daemon's own room-close path is bounded at
/// ~10s; allow margin), exposed for [`Timeouts`]'s default.
pub(crate) const DEFAULT_TEARDOWN: Duration = Duration::from_secs(15);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teardown_stopped_something_is_false_only_for_left_running() {
        assert!(!Teardown::LeftRunning.stopped_something());
        assert!(Teardown::Graceful.stopped_something());
        assert!(Teardown::Forced.stopped_something());
    }

    #[test]
    fn teardown_variants_are_distinguishable() {
        assert_ne!(Teardown::LeftRunning, Teardown::Graceful);
        assert_ne!(Teardown::Graceful, Teardown::Forced);
        assert_ne!(Teardown::LeftRunning, Teardown::Forced);
    }

    #[test]
    #[allow(clippy::clone_on_copy)]
    fn teardown_is_copy_and_clone() {
        let t = Teardown::Graceful;
        let _copy = t;
        let _also = t; // Copy: both moves compile.
        let _ = t.clone(); // Proves Clone is also implemented.
    }

    #[test]
    fn adopted_sidecar_is_not_owned() {
        let portfile: crate::portfile::Portfile = serde_json::from_str(
            r#"{"pid":1,"port":9,"protocol":2,"storage_generation":2,
               "data_dir":"/d","auth_token":"t"}"#,
        )
        .unwrap();
        let sidecar = Sidecar {
            portfile,
            ownership: Ownership::Adopted,
            adopted_lock: None,
            data_dir: std::path::PathBuf::from("/d"),
            expected: crate::generation::Generation::new(2, 2),
            strict_portfile_perms: false,
            timeouts: crate::supervisor::Timeouts::default(),
        };
        assert!(
            !sidecar.is_owned(),
            "adopted Sidecar must not report is_owned()"
        );
        assert_eq!(sidecar.pid(), 1);
        assert_eq!(sidecar.port(), 9);
    }

    /// An owned daemon that exits ABNORMALLY (non-zero) during shutdown must be
    /// reported as `Forced`, not `Graceful`: cleanup may not have run. Red-before:
    /// the pre-fix `shutdown()` returned `Graceful` for any `Ok(status)`,
    /// regardless of `status.success()`.
    #[cfg(unix)] // spawns a real `sh` child (Unix-only; Windows uses a different teardown)
    #[test]
    fn shutdown_reports_forced_for_an_abnormal_owned_exit() {
        use std::process::Stdio;
        use std::time::Duration;

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            // A child that exits non-zero on its own (not via our SIGKILL).
            let mut cmd = tokio::process::Command::new("sh");
            cmd.arg("-c").arg("exit 3").stdin(Stdio::piped());
            let mut child = cmd.spawn().expect("spawn stub child");
            let stdin = child.stdin.take();

            let portfile: crate::portfile::Portfile = serde_json::from_str(
                r#"{"pid":1,"port":9,"protocol":2,"storage_generation":2,
                   "data_dir":"/d","auth_token":"t"}"#,
            )
            .unwrap();
            let sidecar = Sidecar {
                portfile,
                ownership: Ownership::Owned { child, stdin },
                adopted_lock: None,
                data_dir: std::path::PathBuf::from("/d"),
                expected: crate::generation::Generation::new(2, 2),
                strict_portfile_perms: false,
                timeouts: crate::supervisor::Timeouts {
                    teardown: Duration::from_secs(5),
                    ..crate::supervisor::Timeouts::default()
                },
            };
            let teardown = sidecar.shutdown().await.expect("shutdown resolves");
            assert_eq!(
                teardown,
                Teardown::Forced,
                "an abnormal (non-zero) owned exit is not Graceful — cleanup may not have run"
            );
        });
    }

    /// An owned daemon that exits ZERO but leaves its `daemon.json` behind did
    /// not finish cleanup, so `shutdown` must report `Forced`, not `Graceful`.
    /// Red-before: before the portfile-removal check, any zero exit → `Graceful`.
    #[cfg(unix)] // spawns a real `sh` child (Unix-only; Windows uses a different teardown)
    #[test]
    fn shutdown_reports_forced_when_a_zero_exit_leaves_the_portfile() {
        use std::process::Stdio;
        use std::time::Duration;

        let dir = std::env::temp_dir().join(format!(
            "jeliya-sup-cleanup-{}-{}",
            std::process::id(),
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // A portfile the "daemon" fails to remove (the stub just exits 0).
        std::fs::write(dir.join("daemon.json"), r#"{"pid":1,"port":9}"#).unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .unwrap();
        let teardown = rt.block_on(async {
            let mut cmd = tokio::process::Command::new("sh");
            cmd.arg("-c").arg("exit 0").stdin(Stdio::piped());
            let mut child = cmd.spawn().expect("spawn stub child");
            let stdin = child.stdin.take();
            let portfile: crate::portfile::Portfile = serde_json::from_str(
                r#"{"pid":1,"port":9,"protocol":2,"storage_generation":2,
                   "data_dir":"/d","auth_token":"t"}"#,
            )
            .unwrap();
            let sidecar = Sidecar {
                portfile,
                ownership: Ownership::Owned { child, stdin },
                adopted_lock: None,
                data_dir: dir.clone(),
                expected: crate::generation::Generation::new(2, 2),
                strict_portfile_perms: false,
                timeouts: crate::supervisor::Timeouts {
                    teardown: Duration::from_secs(5),
                    ..crate::supervisor::Timeouts::default()
                },
            };
            sidecar.shutdown().await.expect("shutdown resolves")
        });
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(
            teardown,
            Teardown::Forced,
            "a zero exit that left the portfile is not Graceful — cleanup did not finish"
        );
    }

    /// An owned shutdown must report `Forced`, not `Graceful`, when `daemon.json`
    /// was ALREADY absent before the signal: mere absence does not prove THIS
    /// run's cleanup ran (jeliyad treats a missing portfile during removal as
    /// success, and an overridden child can exit 0 with no cleanup). Red-before/
    /// green-after: without the present→absent transition, a zero-exiting child
    /// with no portfile reports `Graceful`.
    #[cfg(unix)] // spawns a real `sh` child (Unix-only; Windows uses a different teardown)
    #[test]
    fn owned_shutdown_with_a_pre_absent_portfile_is_forced() {
        use std::process::Stdio;
        use std::time::Duration;

        let dir = std::env::temp_dir().join(format!("sup-owned-preabsent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // No daemon.json is written — pre-absent before the shutdown.

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .unwrap();
        let teardown = rt.block_on(async {
            let mut cmd = tokio::process::Command::new("sh");
            cmd.arg("-c").arg("exit 0").stdin(Stdio::piped());
            let mut child = cmd.spawn().expect("spawn stub child");
            let stdin = child.stdin.take();
            let portfile: crate::portfile::Portfile = serde_json::from_str(
                r#"{"pid":1,"port":9,"protocol":2,"storage_generation":2,
                   "data_dir":"/d","auth_token":"t"}"#,
            )
            .unwrap();
            let sidecar = Sidecar {
                portfile,
                ownership: Ownership::Owned { child, stdin },
                adopted_lock: None,
                data_dir: dir.clone(),
                expected: crate::generation::Generation::new(2, 2),
                strict_portfile_perms: false,
                timeouts: crate::supervisor::Timeouts {
                    teardown: Duration::from_secs(5),
                    ..crate::supervisor::Timeouts::default()
                },
            };
            sidecar.shutdown().await.expect("shutdown resolves")
        });
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(
            teardown,
            Teardown::Forced,
            "a pre-absent portfile must not report Graceful — the transition proves nothing"
        );
    }

    /// A `stop_adopted` whose RPC returns an error surfaces the dedicated
    /// `ShutdownRpcFailed`, NOT `Handshake` (a startup-announcement error).
    #[test]
    fn stop_adopted_maps_an_rpc_error_to_shutdown_rpc_failed() {
        let dir = std::env::temp_dir().join(format!("sup-adopt-rpcerr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A CURRENT portfile matching the adopted identity, so the pre-RPC
        // revalidation passes and the RPC is reached.
        let json = r#"{"pid":4242,"port":9,"protocol":2,"storage_generation":2,"data_dir":"/d","auth_token":"t"}"#;
        std::fs::write(dir.join("daemon.json"), json).unwrap();
        let portfile: crate::portfile::Portfile = serde_json::from_str(json).unwrap();
        let sidecar = Sidecar {
            portfile,
            ownership: Ownership::Adopted,
            adopted_lock: None,
            data_dir: dir.clone(),
            expected: crate::generation::Generation::new(2, 2),
            strict_portfile_perms: false,
            timeouts: crate::supervisor::Timeouts::default(),
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(sidecar.stop_adopted(|_target| {
            Box::pin(async { Err(CallerRpcError("access denied".to_owned())) })
        }));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::ShutdownRpcFailed(_))),
            "an RPC error must be ShutdownRpcFailed, not Handshake; got: {result:?}"
        );
    }

    /// A `daemon.stop` RPC error whose text echoes the caller's `Authorization`
    /// header must have the bearer token REDACTED before it enters
    /// `ShutdownRpcFailed` (whose Debug/Display are logged) — the crate's
    /// token-redaction contract (§5.8).
    #[test]
    fn stop_adopted_redacts_the_bearer_token_from_an_rpc_error() {
        let dir = std::env::temp_dir().join(format!("sup-adopt-redact-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let token = "SECRETdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef00";
        let json = format!(
            r#"{{"pid":4242,"port":9,"protocol":2,"storage_generation":2,"data_dir":"/d","auth_token":"{token}"}}"#
        );
        std::fs::write(dir.join("daemon.json"), &json).unwrap();
        let portfile: crate::portfile::Portfile = serde_json::from_str(&json).unwrap();
        let sidecar = Sidecar {
            portfile,
            ownership: Ownership::Adopted,
            adopted_lock: None,
            data_dir: dir.clone(),
            expected: crate::generation::Generation::new(2, 2),
            strict_portfile_perms: false,
            timeouts: crate::supervisor::Timeouts::default(),
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let echoed = format!("401 Unauthorized (sent Authorization: Bearer {token})");
        let result = rt.block_on(
            sidecar
                .stop_adopted(move |_target| Box::pin(async move { Err(CallerRpcError(echoed)) })),
        );
        std::fs::remove_dir_all(&dir).ok();
        match result {
            Err(SupervisorError::ShutdownRpcFailed(msg)) => {
                assert!(
                    !msg.contains(token),
                    "the bearer token must be redacted; got: {msg}"
                );
                assert!(
                    msg.contains("<redacted>"),
                    "the token should be replaced with <redacted>; got: {msg}"
                );
            }
            other => panic!("expected ShutdownRpcFailed, got: {other:?}"),
        }
    }

    /// `stop_adopted` must REFUSE, and NEVER issue the stop RPC, when the current
    /// portfile names a DIFFERENT identity than the one adopted — a replacement
    /// daemon started in the same dir. Otherwise a reconnecting RPC would stop a
    /// process this `Sidecar` never adopted.
    #[test]
    fn stop_adopted_refuses_a_replaced_identity_without_issuing_the_rpc() {
        let dir = std::env::temp_dir().join(format!("sup-adopt-idchange-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // The CURRENT portfile names a REPLACEMENT (different pid+port).
        let current = r#"{"pid":5555,"port":10,"protocol":2,"storage_generation":2,"data_dir":"/d","auth_token":"t"}"#;
        std::fs::write(dir.join("daemon.json"), current).unwrap();
        // We adopted pid 4242 / port 9.
        let adopted: crate::portfile::Portfile = serde_json::from_str(
            r#"{"pid":4242,"port":9,"protocol":2,"storage_generation":2,"data_dir":"/d","auth_token":"t"}"#,
        )
        .unwrap();
        let sidecar = Sidecar {
            portfile: adopted,
            ownership: Ownership::Adopted,
            adopted_lock: None,
            data_dir: dir.clone(),
            expected: crate::generation::Generation::new(2, 2),
            strict_portfile_perms: false,
            timeouts: crate::supervisor::Timeouts::default(),
        };
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_rpc = called.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(sidecar.stop_adopted(move |_target| {
            called_rpc.store(true, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::ShutdownTimedOut { pid: 4242 })),
            "a changed identity must refuse the stop; got: {result:?}"
        );
        assert!(
            !called.load(std::sync::atomic::Ordering::SeqCst),
            "the stop RPC must NOT be issued against a replacement daemon"
        );
    }

    /// `stop_adopted` must also refuse — and never issue the RPC — when a replacement
    /// daemon was assigned the SAME recycled pid AND the same ephemeral port but a
    /// fresh per-start `auth_token`. pid+port alone are recyclable; the token is not.
    /// Red-before (pre-token check): matching pid+port accepted the replacement and
    /// the RPC fired.
    #[test]
    fn stop_adopted_refuses_a_recycled_pid_port_with_a_new_token() {
        let dir = std::env::temp_dir().join(format!("sup-adopt-tokchange-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // CURRENT portfile: identical pid+port (recycled), but a DIFFERENT token.
        let current = r#"{"pid":4242,"port":9,"protocol":2,"storage_generation":2,"data_dir":"/d","auth_token":"replacement-token"}"#;
        std::fs::write(dir.join("daemon.json"), current).unwrap();
        // We adopted the same pid+port, but with the ORIGINAL per-start token.
        let adopted: crate::portfile::Portfile = serde_json::from_str(
            r#"{"pid":4242,"port":9,"protocol":2,"storage_generation":2,"data_dir":"/d","auth_token":"adopted-token"}"#,
        )
        .unwrap();
        let sidecar = Sidecar {
            portfile: adopted,
            ownership: Ownership::Adopted,
            adopted_lock: None,
            data_dir: dir.clone(),
            expected: crate::generation::Generation::new(2, 2),
            strict_portfile_perms: false,
            timeouts: crate::supervisor::Timeouts::default(),
        };
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_rpc = called.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(sidecar.stop_adopted(move |_target| {
            called_rpc.store(true, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::ShutdownTimedOut { pid: 4242 })),
            "a recycled pid+port with a fresh token must refuse the stop; got: {result:?}"
        );
        assert!(
            !called.load(std::sync::atomic::Ordering::SeqCst),
            "the stop RPC must NOT be issued against a replacement with a fresh token"
        );
    }

    /// The stop invoker is handed a `DialTarget` BOUND to the revalidated incarnation:
    /// its URL dials the validated port and its bearer is the validated per-start
    /// token, so a caller that dials the passed target cannot reconnect to a later
    /// replacement (a fresh resolve would hand back the replacement's new token).
    #[test]
    fn stop_adopted_binds_the_rpc_to_the_revalidated_target() {
        let dir = std::env::temp_dir().join(format!("sup-adopt-bind-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let json = r#"{"pid":4242,"port":9,"protocol":2,"storage_generation":2,"data_dir":"/d","auth_token":"the-adopted-token"}"#;
        std::fs::write(dir.join("daemon.json"), json).unwrap();
        let adopted: crate::portfile::Portfile = serde_json::from_str(json).unwrap();
        let sidecar = Sidecar {
            portfile: adopted,
            ownership: Ownership::Adopted,
            adopted_lock: None,
            data_dir: dir.clone(),
            expected: crate::generation::Generation::new(2, 2),
            strict_portfile_perms: false,
            // Short teardown so the post-RPC completion check does not linger.
            timeouts: crate::supervisor::Timeouts {
                teardown: Duration::from_millis(200),
                ..crate::supervisor::Timeouts::default()
            },
        };
        let captured: std::sync::Arc<std::sync::Mutex<Option<(String, String)>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let cap = captured.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _ = rt.block_on(sidecar.stop_adopted(move |target| {
            *cap.lock().unwrap() = Some((target.ws_url().to_string(), target.bearer().to_string()));
            Box::pin(async { Ok(()) })
        }));
        std::fs::remove_dir_all(&dir).ok();
        let (url, bearer) = captured
            .lock()
            .unwrap()
            .clone()
            .expect("the invoker must receive a bound DialTarget");
        assert!(
            url.contains("127.0.0.1:9/ws"),
            "the target must dial the validated port; got {url}"
        );
        assert_eq!(
            bearer, "the-adopted-token",
            "the target must carry the validated per-start bearer, not a re-resolved one"
        );
    }

    /// `stop_adopted` must NOT report `Graceful` when `daemon.json` was ALREADY
    /// absent before the RPC (a cleanup tool removed it, or a prior partial
    /// shutdown): portfile removal is proof only as a present→absent transition,
    /// so a pre-absent portfile yields the honest `ShutdownTimedOut`.
    ///
    /// Red-before/green-after: without the pre-absence guard,
    /// `wait_portfile_removed` returns immediately on the missing file and the
    /// call reports `Graceful` while the daemon (whose listener drops FIRST, ~10s
    /// before cleanup finishes) may still be closing rooms / holding its lock.
    #[test]
    fn stop_adopted_with_a_pre_absent_portfile_is_not_graceful() {
        use std::time::Duration;

        let dir = std::env::temp_dir().join(format!("sup-adopt-preabsent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // No daemon.json is written — it is pre-absent. A dead port makes the
        // health probe report dark immediately, so the completion check is reached.
        let dead_port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let p = l.local_addr().unwrap().port();
            drop(l);
            p
        };
        let portfile: crate::portfile::Portfile = serde_json::from_str(&format!(
            r#"{{"pid":4242,"port":{dead_port},"protocol":2,"storage_generation":2,
               "data_dir":"/d","auth_token":"t"}}"#,
        ))
        .unwrap();
        let sidecar = Sidecar {
            portfile,
            ownership: Ownership::Adopted,
            adopted_lock: None,
            data_dir: dir.clone(),
            expected: crate::generation::Generation::new(2, 2),
            strict_portfile_perms: false,
            timeouts: crate::supervisor::Timeouts {
                teardown: Duration::from_millis(300),
                health_connect: Duration::from_millis(50),
                health_read: Duration::from_millis(50),
                ..crate::supervisor::Timeouts::default()
            },
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .unwrap();
        let result = rt.block_on(sidecar.stop_adopted(|_target| Box::pin(async { Ok(()) })));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::ShutdownTimedOut { .. })),
            "a pre-absent portfile must not report Graceful; got: {result:?}"
        );
    }

    /// An OWNED daemon that spawned a long-lived descendant into its isolated
    /// process group and then exits on stdin EOF must not orphan that descendant:
    /// `Sidecar::shutdown` sweeps the group after reaping the leader.
    /// Red-before/green-after: without the pgid capture + sweep on the owned
    /// reaped arms, the `sleep 600` descendant survives the shutdown.
    #[cfg(unix)]
    #[test]
    fn owned_shutdown_sweeps_a_descendant_left_by_the_leader() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Stdio;
        use std::time::Duration;

        let dir = std::env::temp_dir().join(format!(
            "sup-owned-forker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let pidfile = dir.join("descendant.pid");
        let stub = dir.join("leader");
        // Spawn a stdio-closed descendant, record its pid, then block on stdin
        // until EOF (the shutdown signal) and exit 0.
        let script = format!(
            "#!/bin/sh\n\
             sh -c 'exec >/dev/null 2>&1 <&-; echo $$ > \"{pf}\"; sleep 600' &\n\
             i=0\n\
             while [ ! -s \"{pf}\" ] && [ $i -lt 100 ]; do i=$((i+1)); sleep 0.02; done\n\
             cat >/dev/null\n\
             exit 0\n",
            pf = pidfile.display()
        );
        std::fs::write(&stub, script).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .unwrap();
        let descendant = rt.block_on(async {
            let mut cmd = tokio::process::Command::new(&stub);
            cmd.stdin(Stdio::piped());
            // Isolate the group exactly as the real spawn does, so the pgid==leader
            // pid and the sweep reaches the descendant.
            crate::process::configure_new_process_group(&mut cmd);
            // Retry a transient ETXTBSY (errno 26): a just-written+chmod'd script can be
            // "Text file busy" until the writer fd is fully closed — the same race the
            // production spawn path retries. Without it this direct spawn flakes.
            let mut child = {
                let mut attempt = 0;
                loop {
                    match cmd.spawn() {
                        Ok(c) => break c,
                        Err(e) if e.raw_os_error() == Some(26) && attempt < 50 => {
                            attempt += 1;
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                        Err(e) => panic!("spawn leader: {e}"),
                    }
                }
            };
            let stdin = child.stdin.take();
            // Wait for the descendant to record its pid before shutting down.
            let mut pid = None;
            for _ in 0..100 {
                if let Ok(text) = std::fs::read_to_string(&pidfile) {
                    if let Ok(n) = text.trim().parse::<u32>() {
                        pid = Some(n);
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let portfile: crate::portfile::Portfile = serde_json::from_str(
                r#"{"pid":1,"port":9,"protocol":2,"storage_generation":2,
                   "data_dir":"/d","auth_token":"t"}"#,
            )
            .unwrap();
            let sidecar = Sidecar {
                portfile,
                ownership: Ownership::Owned { child, stdin },
                adopted_lock: None,
                data_dir: dir.clone(),
                expected: crate::generation::Generation::new(2, 2),
                strict_portfile_perms: false,
                timeouts: crate::supervisor::Timeouts {
                    teardown: Duration::from_secs(5),
                    ..crate::supervisor::Timeouts::default()
                },
            };
            // `shutdown` drops stdin → the leader's `cat` hits EOF, exits, is
            // reaped; the sweep then SIGKILLs the descendant with the group.
            let _ = sidecar.shutdown().await;
            pid.expect("descendant recorded its pid")
        });

        let mut alive = true;
        for _ in 0..40 {
            let up = std::process::Command::new("kill")
                .args(["-0", &descendant.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !up {
                alive = false;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if alive {
            let _ = std::process::Command::new("kill")
                .args(["-9", &descendant.to_string()])
                .status();
        }
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            !alive,
            "the owned leader's descendant must be swept with the group (pid {descendant})"
        );
    }

    /// A never-resolving `shutdown_rpc` must not wedge `stop_adopted`: it is
    /// bounded by `teardown` and surfaces `ShutdownTimedOut`. Red-before: the
    /// pre-fix `stop_adopted` awaited `shutdown_rpc()` with no timeout (the
    /// deadline was created only after it resolved), so this hangs forever
    /// without the bound. A manual current-thread runtime keeps it off the
    /// dev-only `macros` feature.
    #[test]
    fn stop_adopted_bounds_a_never_resolving_rpc() {
        use std::time::Duration;

        let dir = std::env::temp_dir().join(format!("sup-adopt-hangrpc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A matching CURRENT portfile so the pre-RPC revalidation passes.
        let json = r#"{"pid":4242,"port":9,"protocol":2,"storage_generation":2,"data_dir":"/d","auth_token":"t"}"#;
        std::fs::write(dir.join("daemon.json"), json).unwrap();
        let portfile: crate::portfile::Portfile = serde_json::from_str(json).unwrap();
        let sidecar = Sidecar {
            portfile,
            ownership: Ownership::Adopted,
            adopted_lock: None,
            data_dir: dir.clone(),
            expected: crate::generation::Generation::new(2, 2),
            strict_portfile_perms: false,
            timeouts: crate::supervisor::Timeouts {
                teardown: Duration::from_millis(150),
                ..crate::supervisor::Timeouts::default()
            },
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let started = std::time::Instant::now();
        let result = rt.block_on(sidecar.stop_adopted(|_target| {
            // An invoker whose transport never answers.
            Box::pin(std::future::pending::<Result<(), CallerRpcError>>())
        }));
        let elapsed = started.elapsed();
        std::fs::remove_dir_all(&dir).ok();

        assert!(
            matches!(result, Err(SupervisorError::ShutdownTimedOut { pid: 4242 })),
            "a stalled RPC must surface ShutdownTimedOut, not hang; got: {result:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "the RPC wait must be bounded by teardown (~150ms), took {elapsed:?}"
        );
    }

    #[test]
    fn target_resolver_from_sidecar_shares_data_dir_and_expected() {
        let portfile: crate::portfile::Portfile = serde_json::from_str(
            r#"{"pid":1,"port":9,"protocol":2,"storage_generation":2,
               "data_dir":"/d","auth_token":"t"}"#,
        )
        .unwrap();
        let data_dir = std::path::PathBuf::from("/my/data");
        let expected = crate::generation::Generation::new(2, 2);
        let sidecar = Sidecar {
            portfile,
            ownership: Ownership::Adopted,
            adopted_lock: None,
            data_dir: data_dir.clone(),
            expected,
            strict_portfile_perms: false,
            timeouts: crate::supervisor::Timeouts::default(),
        };
        let resolver = sidecar.target_resolver();
        assert_eq!(resolver.data_dir, data_dir);
        assert_eq!(resolver.expected, expected);
        // TargetResolver is Clone: a transport can clone it without holding the Sidecar.
        let _ = resolver.clone();
    }

    /// Cancelling `shutdown()` mid-wait (the application runtime is torn down after
    /// stdin is closed) must still SIGKILL an owned daemon that ignores stdin-EOF.
    /// The child is `sleep`, which never exits on stdin close; an OUTER timeout drops
    /// the `shutdown()` future while its long internal teardown wait is still pending.
    /// Red-before: with the child held by `&mut self.ownership` (no guard), the
    /// dropped future dropped the child with `kill_on_drop(false)` and the group
    /// SURVIVED; green-after: the armed `SpawnGuard` force-kills it on drop.
    #[cfg(unix)] // spawns a real `sh`/`sleep` child (Unix-only)
    #[test]
    fn cancelling_owned_shutdown_still_kills_a_daemon_that_ignores_stdin_eof() {
        use std::process::Stdio;
        use std::time::Duration;

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            // `sleep` ignores stdin entirely, so closing stdin never stops it — the
            // exact "overridden daemon that ignores its parent-death signal" case.
            let mut cmd = tokio::process::Command::new("sh");
            cmd.arg("-c")
                .arg("sleep 600")
                .stdin(Stdio::piped())
                .kill_on_drop(false);
            crate::process::configure_new_process_group(&mut cmd);
            let mut child = cmd.spawn().expect("spawn stub child");
            let stdin = child.stdin.take();
            let pid = child.id().expect("child has a pid") as i32;

            let portfile: crate::portfile::Portfile = serde_json::from_str(
                r#"{"pid":1,"port":9,"protocol":2,"storage_generation":2,
                   "data_dir":"/d","auth_token":"t"}"#,
            )
            .unwrap();
            let sidecar = Sidecar {
                portfile,
                ownership: Ownership::Owned { child, stdin },
                adopted_lock: None,
                data_dir: std::path::PathBuf::from("/d"),
                expected: crate::generation::Generation::new(2, 2),
                strict_portfile_perms: false,
                // A LONG teardown so the internal `child.wait()` is still pending when
                // the outer timeout cancels the future (isolating cancellation from the
                // internal-timeout escalation arm).
                timeouts: crate::supervisor::Timeouts {
                    teardown: Duration::from_secs(600),
                    ..crate::supervisor::Timeouts::default()
                },
            };

            // Cancel the shutdown mid-wait by dropping its future (outer timeout).
            let _ = tokio::time::timeout(Duration::from_millis(150), sidecar.shutdown()).await;

            // The guard's Drop must have SIGKILLed the child's group.
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
                "a cancelled owned shutdown must still SIGKILL a daemon that ignores stdin-EOF"
            );
        });
    }
}
