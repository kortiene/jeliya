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
//! lever that reaches it is the protocol `daemon.shutdown`
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
        match &mut self.ownership {
            Ownership::Adopted => Ok(Teardown::LeftRunning),
            Ownership::Owned { child, stdin } => {
                // Closing stdin is the graceful `--supervised` signal.
                drop(stdin.take());
                match tokio::time::timeout(self.timeouts.teardown, child.wait()).await {
                    Ok(Ok(_status)) => Ok(Teardown::Graceful),
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
    /// `daemon.shutdown` invoker (the supervisor does not link the client). No
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
        // Ask the daemon to shut itself down over the caller's RPC. A failure
        // here is surfaced, not swallowed — the caller decides whether to retry.
        shutdown_rpc()
            .await
            .map_err(|e| SupervisorError::Handshake(e.to_string()))?;
        // Confirm it actually went dark; we never signalled it, so the RPC is
        // the only lever and we must verify it took effect.
        let deadline = tokio::time::Instant::now() + self.timeouts.teardown;
        if !validate::wait_health_dark(pid, port, self.timeouts.teardown, &self.timeouts).await {
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
    /// The daemon exited within its budget (stdin EOF for owned, `daemon.shutdown`
    /// for adopted). It ran its own cleanup and removed its portfile.
    Graceful,
    /// Owned daemon: the graceful path timed out and its process group was
    /// SIGKILLed. No cleanup ran, so a stale `daemon.json` may remain.
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
