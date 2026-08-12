//! The closed error taxonomy, so a caller can distinguish "wrong generation,
//! show the clean-slate reset path" from "the daemon died, retry".

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use crate::generation::{Generation, SeenGeneration};

/// A boxed, `'static` future the supervisor awaits inline. Not `Send`-bound: the
/// caller's `daemon.shutdown` closure (which may hold a `!Send` `ClientHandle`)
/// is driven on the supervisor's own task, never moved across threads. Mirrors
/// the std `BoxFuture` alias `jeliya-platform`/`jeliya-client` own.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// An error surfaced by the caller-supplied `daemon.shutdown` RPC closure. The
/// supervisor does not link the client, so it treats the RPC as opaque: any
/// failure carries only a message.
#[derive(Debug, Clone)]
pub struct CallerRpcError(pub String);

impl std::fmt::Display for CallerRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "daemon.shutdown RPC failed: {}", self.0)
    }
}

impl std::error::Error for CallerRpcError {}

/// Everything that can go wrong obtaining, validating, or stopping a daemon.
///
/// `GenerationMismatch` is the one a UI translates into the clean-slate reset
/// path; the rest are operational (retry, or surface the fault).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SupervisorError {
    /// No `jeliyad` binary could be resolved. Lists every path attempted, in
    /// order, so a packaging bug is diagnosable. A `PATH` daemon is never
    /// silently used (spec §6.1).
    #[error("no jeliyad binary found; tried: {}", .tried.join(", "))]
    NoBinary {
        /// Every candidate path attempted, in resolution order.
        tried: Vec<String>,
    },

    /// The daemon process could not be spawned.
    #[error("could not spawn jeliyad: {0}")]
    Spawn(#[source] std::io::Error),

    /// No/garbled `ready` line, or a ready↔portfile disagreement (a stale
    /// portfile from a prior run is being read).
    #[error("jeliyad did not announce itself: {0}")]
    Handshake(String),

    /// No portfile exists at the resolved data dir.
    #[error("no portfile at {0}")]
    PortfileMissing(PathBuf),

    /// A portfile exists but is torn, missing a load-bearing field, or (in
    /// strict mode) too permissive to trust with the token.
    #[error("portfile at {path} is unreadable: {why}")]
    PortfileUnreadable {
        /// The portfile path.
        path: PathBuf,
        /// Why it could not be trusted.
        why: String,
    },

    /// The portfile parsed and declares a supported generation, but no healthy
    /// daemon answers `/api/health` on the advertised port with the matching
    /// PID (a recycled/dead PID or port, or a daemon still starting). On a
    /// spawn path a fresh daemon heals this.
    #[error("portfile advertises port {port} but no matching healthy daemon answered")]
    Stale {
        /// The advertised port that failed the health/PID proof.
        port: u16,
    },

    /// The daemon's `protocol`/`storage_generation` (portfile or health) does
    /// not equal what this build speaks. **Fails closed**: never adopted, never
    /// migrated. Only a proven-owned incumbent may be replaced (spec §6.7).
    #[error("generation mismatch: expected {expected}, daemon reports {actual}")]
    GenerationMismatch {
        /// What this build was built against.
        expected: Generation,
        /// What the daemon advertised (either axis may be absent).
        actual: SeenGeneration,
    },

    /// A portfile advertising a non-loopback `ws`/`http` endpoint. Refused
    /// before any dial (spec §7.1).
    #[error("portfile advertises a non-loopback endpoint: {advertised}")]
    NonLoopback {
        /// The offending advertised endpoint.
        advertised: String,
    },

    /// The data dir's instance lock is held, no healthy daemon answers, and the
    /// daemon made no progress within its window. Distinct so a UI can say
    /// "starting, retry in a moment" (spec §6.5 / OQ-6).
    #[error("the data dir is locked but no healthy daemon answered; retry in a moment")]
    Wedged,

    /// An adopted daemon or a proven-owned incumbent survived its bounded stop.
    #[error("daemon pid {pid} did not stop within its bounded budget")]
    ShutdownTimedOut {
        /// The PID that outlived its stop budget.
        pid: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::{Generation, SeenGeneration};

    #[test]
    fn caller_rpc_error_display_contains_the_message() {
        let e = CallerRpcError("timed out".to_owned());
        let d = format!("{e}");
        assert!(d.contains("timed out"), "display: {d}");
    }

    #[test]
    fn no_binary_error_lists_tried_paths() {
        let e = SupervisorError::NoBinary {
            tried: vec!["/a/jeliyad".to_owned(), "/b/jeliyad".to_owned()],
        };
        let d = format!("{e}");
        assert!(d.contains("/a/jeliyad"), "display: {d}");
        assert!(d.contains("/b/jeliyad"), "display: {d}");
    }

    #[test]
    fn generation_mismatch_display_mentions_expected_and_actual() {
        let e = SupervisorError::GenerationMismatch {
            expected: Generation::new(2, 2),
            actual: SeenGeneration {
                protocol: Some(1),
                storage_generation: None,
            },
        };
        let d = format!("{e}");
        // Both the expected and the actual should appear; check for at least one axis value.
        assert!(
            d.contains("2") && d.contains("1"),
            "display must show values: {d}"
        );
    }

    #[test]
    fn stale_error_mentions_the_port() {
        let e = SupervisorError::Stale { port: 7420 };
        let d = format!("{e}");
        assert!(d.contains("7420"), "display: {d}");
    }

    #[test]
    fn non_loopback_error_mentions_the_advertised_endpoint() {
        let e = SupervisorError::NonLoopback {
            advertised: "ws://evil.example/ws".to_owned(),
        };
        let d = format!("{e}");
        assert!(d.contains("evil.example"), "display: {d}");
    }

    #[test]
    fn shutdown_timed_out_error_mentions_pid() {
        let e = SupervisorError::ShutdownTimedOut { pid: 12345 };
        let d = format!("{e}");
        assert!(d.contains("12345"), "display: {d}");
    }

    #[test]
    fn portfile_missing_error_mentions_path() {
        let e = SupervisorError::PortfileMissing(std::path::PathBuf::from("/tmp/daemon.json"));
        let d = format!("{e}");
        assert!(d.contains("daemon.json"), "display: {d}");
    }

    #[test]
    fn portfile_unreadable_error_mentions_path_and_reason() {
        let e = SupervisorError::PortfileUnreadable {
            path: std::path::PathBuf::from("/tmp/daemon.json"),
            why: "truncated JSON".to_owned(),
        };
        let d = format!("{e}");
        assert!(d.contains("daemon.json"), "display: {d}");
        assert!(d.contains("truncated JSON"), "display: {d}");
    }
}
