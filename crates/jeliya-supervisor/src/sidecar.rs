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
use crate::portfile::Portfile;
use crate::process;
use crate::supervisor::Timeouts;
use crate::target::TargetResolver;
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
    pub async fn shutdown(mut self) -> Result<Teardown, SupervisorError> {
        // Borrowed before the `&mut self.ownership` match so the portfile path is
        // available inside the arms (disjoint field).
        let data_dir = self.data_dir.clone();
        match &mut self.ownership {
            Ownership::Adopted => Ok(Teardown::LeftRunning),
            Ownership::Owned { child, stdin } => {
                // Closing stdin is the graceful `--supervised` signal.
                drop(stdin.take());
                match tokio::time::timeout(self.timeouts.teardown, child.wait()).await {
                    // A zero exit is NOT itself proof of completed cleanup: the
                    // daemon discards its room-close result and only LOGS a failed
                    // `daemon.json` removal, so it can exit 0 with cleanup
                    // incomplete. `Graceful` is promised only when the portfile is
                    // actually gone (its removal is the daemon's final step); a
                    // lingering portfile means cleanup did not finish → `Forced`.
                    Ok(Ok(status)) if status.success() => {
                        // Only a CONFIRMED absence (`Ok(false)`) is `Graceful`; a
                        // lingering portfile OR a stat error (unreadable dir) is
                        // `Forced` — cleanup is not proven complete.
                        if matches!(
                            crate::portfile::portfile_path(&data_dir).try_exists(),
                            Ok(false)
                        ) {
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
                    Ok(Ok(_status)) => Ok(Teardown::Forced),
                    Ok(Err(e)) => Err(SupervisorError::Spawn(e)),
                    Err(_elapsed) => {
                        process::force_kill_tree(child)
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
    /// On an **owned** sidecar this delegates to [`Sidecar::shutdown`] (the
    /// signal path), because an owned daemon is stopped by closing its stdin,
    /// not by an RPC.
    pub async fn stop_adopted(
        self,
        shutdown_rpc: impl FnOnce() -> BoxFuture<'static, Result<(), CallerRpcError>>,
    ) -> Result<Teardown, SupervisorError> {
        if self.is_owned() {
            return self.shutdown().await;
        }
        let pid = self.portfile.pid;
        let port = self.portfile.port;
        // Snapshot portfile presence BEFORE the RPC: the daemon removes
        // `daemon.json` as the LAST step of graceful shutdown, so a present→absent
        // transition is the completion proof below. If it is ALREADY absent here
        // (a cleanup tool removed it, or a prior partial shutdown), its removal
        // proves nothing — see the completion check.
        let portfile_present_before = matches!(
            crate::portfile::portfile_path(&self.data_dir).try_exists(),
            Ok(true)
        );
        // The whole adopted stop is time-boxed by `teardown`, RPC included: a
        // caller-supplied invoker whose transport stalls must not wedge shutdown
        // just because its own future has no timeout. Start the deadline BEFORE
        // the RPC and bound the RPC against it, so a never-resolving invoker
        // surfaces `ShutdownTimedOut` rather than hanging here forever.
        let deadline = tokio::time::Instant::now() + self.timeouts.teardown;
        // Ask the daemon to shut itself down over the caller's RPC. A failure
        // here is surfaced as the dedicated `ShutdownRpcFailed` — NOT `Handshake`
        // (a startup-announcement error) — so a caller can apply shutdown-
        // specific retry/reporting policy. The caller decides whether to retry.
        match tokio::time::timeout(self.timeouts.teardown, shutdown_rpc()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(SupervisorError::ShutdownRpcFailed(e.to_string())),
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
        // the honest verdict is `ShutdownTimedOut`. (A lock-release proof would
        // remain valid across a pre-absent portfile but needs a cross-crate
        // lock-protocol assumption against `jeliyad`'s `daemon.lock`; deferred.)
        if !portfile_present_before {
            return Err(SupervisorError::ShutdownTimedOut { pid });
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if validate::wait_portfile_removed(&self.data_dir, remaining).await {
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

    /// A `stop_adopted` whose RPC returns an error surfaces the dedicated
    /// `ShutdownRpcFailed`, NOT `Handshake` (a startup-announcement error).
    #[test]
    fn stop_adopted_maps_an_rpc_error_to_shutdown_rpc_failed() {
        let portfile: crate::portfile::Portfile = serde_json::from_str(
            r#"{"pid":4242,"port":9,"protocol":2,"storage_generation":2,
               "data_dir":"/d","auth_token":"t"}"#,
        )
        .unwrap();
        let sidecar = Sidecar {
            portfile,
            ownership: Ownership::Adopted,
            data_dir: std::path::PathBuf::from("/d"),
            expected: crate::generation::Generation::new(2, 2),
            strict_portfile_perms: false,
            timeouts: crate::supervisor::Timeouts::default(),
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let result =
            rt.block_on(sidecar.stop_adopted(|| {
                Box::pin(async { Err(CallerRpcError("access denied".to_owned())) })
            }));
        assert!(
            matches!(result, Err(SupervisorError::ShutdownRpcFailed(_))),
            "an RPC error must be ShutdownRpcFailed, not Handshake; got: {result:?}"
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
        let result = rt.block_on(sidecar.stop_adopted(|| Box::pin(async { Ok(()) })));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(result, Err(SupervisorError::ShutdownTimedOut { .. })),
            "a pre-absent portfile must not report Graceful; got: {result:?}"
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

        let portfile: crate::portfile::Portfile = serde_json::from_str(
            r#"{"pid":4242,"port":9,"protocol":2,"storage_generation":2,
               "data_dir":"/d","auth_token":"t"}"#,
        )
        .unwrap();
        let sidecar = Sidecar {
            portfile,
            ownership: Ownership::Adopted,
            data_dir: std::path::PathBuf::from("/d"),
            expected: crate::generation::Generation::new(2, 2),
            strict_portfile_perms: false,
            timeouts: crate::supervisor::Timeouts {
                teardown: Duration::from_millis(150),
                ..crate::supervisor::Timeouts::default()
            },
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let started = std::time::Instant::now();
        let result = rt.block_on(sidecar.stop_adopted(|| {
            // An invoker whose transport never answers.
            Box::pin(std::future::pending::<Result<(), CallerRpcError>>())
        }));
        let elapsed = started.elapsed();

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
}
