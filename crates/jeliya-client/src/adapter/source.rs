//! The injected target source: the adapter's only door to a dial endpoint.
//!
//! The resolver is **injected, never constructed** by the adapter (dependency
//! inversion): the adapter holds a [`TargetSource`] and can dial, but it holds
//! no `Sidecar`, so it can never spawn or kill a daemon (#170 owns that). The
//! provided `impl TargetSource for `[`jeliya_supervisor::TargetResolver`] is
//! the literal "reuse the supervisor / target resolver" edge; it re-reads and
//! re-validates the portfile on **every** call, so a restart's new port/token/
//! PID heals transparently and a changed generation fails closed.

use futures::future::BoxFuture;
use jeliya_supervisor::{Redacted, SupervisorError, TargetResolver};

/// The one value a dial attempt needs: a token-free, loopback-by-construction
/// WebSocket URL carrying the generation-gate query (`?v=&sg=`), plus the
/// per-start bearer token kept [`Redacted`] until the `Authorization` header is
/// built.
pub struct Dial {
    /// The token-free loopback WS URL (safe to log/display).
    pub url: url::Url,
    /// The per-start bearer token — native-only, exposed once at header time.
    pub bearer: Redacted<String>,
}

impl std::fmt::Debug for Dial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The URL is already token-free; the bearer redacts itself.
        f.debug_struct("Dial")
            .field("url", &self.url.as_str())
            .field("bearer", &self.bearer)
            .finish()
    }
}

/// Why a resolve failed, split by the only distinction the transport acts on.
#[derive(Debug, Clone)]
pub enum DialResolveError {
    /// Wrong daemon / attack shape: **fail closed**. Surface the reset path and
    /// do not auto-retry (mapped to the core's `GateRefused`, a terminal
    /// gate refusal). The message is redacted (it comes from
    /// [`SupervisorError`]'s token-free `Display`).
    Terminal(String),
    /// Not-yet / torn / no-listener: recoverable. Drive the bounded backoff and
    /// retry (mapped to the core's `DialFailed`).
    Transient(String),
}

/// The injected source of *verified* loopback dial facts. `Send + Sync +
/// 'static` so the runtime can hold it in an `Arc` and clone a fresh resolve
/// future per attempt.
pub trait TargetSource: Send + Sync + 'static {
    /// Produce a freshly validated [`Dial`], or a classified failure. Called
    /// once per connection attempt.
    fn resolve(&self) -> BoxFuture<'static, Result<Dial, DialResolveError>>;
}

impl TargetSource for TargetResolver {
    fn resolve(&self) -> BoxFuture<'static, Result<Dial, DialResolveError>> {
        // `TargetResolver` is `Clone` and carries no `Sidecar`, so the transport
        // holds a cheap clone it cannot use to kill the daemon.
        let this = self.clone();
        Box::pin(async move {
            match this.resolve().await {
                Ok(target) => Ok(Dial {
                    url: target.ws_url().clone(),
                    // The single legitimate read of the token, immediately
                    // re-wrapped so it stays `Redacted` on the adapter side.
                    bearer: Redacted::new(target.bearer().to_owned()),
                }),
                Err(error) => Err(classify(error)),
            }
        })
    }
}

/// The fail-closed-vs-retry table (spec §7.1 / §8). `GenerationMismatch`,
/// `NonLoopback`, `DataDirMismatch`, and a dial-URL-build `Handshake` are the
/// wrong-daemon/attack shapes that must fail closed; everything else a resolve
/// can surface (`Stale`, `PortfileMissing`, `PortfileUnreadable`, `Wedged`) is
/// a not-yet/torn condition a fresh validation heals, so it retries. The
/// remaining spawn/stop-only variants are unreachable from `resolve`, but the
/// enum is `#[non_exhaustive]`, so they fall through to the safe `Transient`
/// direction (retry, never a spurious terminal).
fn classify(error: SupervisorError) -> DialResolveError {
    // `SupervisorError`'s `Display` is deliberately token-free (`NonLoopback`
    // and `DataDirMismatch` never reproduce the raw value), so it is safe to
    // carry into a surfaced/logged error.
    let message = error.to_string();
    let terminal = matches!(
        error,
        SupervisorError::GenerationMismatch { .. }
            | SupervisorError::NonLoopback { .. }
            | SupervisorError::DataDirMismatch { .. }
            | SupervisorError::Handshake(_)
    );
    if terminal {
        DialResolveError::Terminal(message)
    } else {
        DialResolveError::Transient(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_supervisor::Generation;

    #[test]
    fn generation_mismatch_is_terminal() {
        let err = SupervisorError::GenerationMismatch {
            expected: Generation::new(2, 2),
            actual: jeliya_supervisor::SeenGeneration {
                protocol: Some(2),
                storage_generation: Some(3),
            },
        };
        assert!(matches!(classify(err), DialResolveError::Terminal(_)));
    }

    #[test]
    fn non_loopback_is_terminal_and_redacts() {
        let err = SupervisorError::NonLoopback { field: "ws" };
        match classify(err) {
            DialResolveError::Terminal(message) => assert!(!message.contains("://")),
            other => panic!("expected terminal, got {other:?}"),
        }
    }

    #[test]
    fn stale_and_missing_are_transient() {
        assert!(matches!(
            classify(SupervisorError::Stale { port: 7420 }),
            DialResolveError::Transient(_)
        ));
        assert!(matches!(
            classify(SupervisorError::PortfileMissing("/d/daemon.json".into())),
            DialResolveError::Transient(_)
        ));
        assert!(matches!(
            classify(SupervisorError::Wedged),
            DialResolveError::Transient(_)
        ));
    }

    #[test]
    fn a_dial_debug_never_leaks_the_bearer() {
        let dial = Dial {
            url: url::Url::parse("ws://127.0.0.1:7420/ws?v=2&sg=2").expect("url"),
            bearer: Redacted::new(String::from("sixtyfourhexsecret")),
        };
        let rendered = format!("{dial:?}");
        assert!(rendered.contains("ws://127.0.0.1:7420/ws"));
        assert!(!rendered.contains("sixtyfourhexsecret"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn data_dir_mismatch_is_terminal() {
        let err = SupervisorError::DataDirMismatch {
            resolved: std::path::PathBuf::from("/home/user/.jeliya"),
        };
        assert!(
            matches!(classify(err), DialResolveError::Terminal(_)),
            "DataDirMismatch is a wrong-daemon shape and must fail closed"
        );
    }

    #[test]
    fn portfile_unreadable_is_transient() {
        let err = SupervisorError::PortfileUnreadable {
            path: std::path::PathBuf::from("/d/daemon.json"),
            why: "truncated JSON".to_owned(),
        };
        assert!(
            matches!(classify(err), DialResolveError::Transient(_)),
            "PortfileUnreadable is a torn/not-yet condition and must retry"
        );
    }

    #[test]
    fn handshake_error_is_terminal() {
        let err = SupervisorError::Handshake("no ready line".to_owned());
        assert!(
            matches!(classify(err), DialResolveError::Terminal(_)),
            "Handshake failure is a dial-URL/config bug and must fail closed"
        );
    }
}
