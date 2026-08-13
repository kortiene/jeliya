//! The fresh-per-dial resolver and the one value a transport dials with.
//!
//! [`TargetResolver`] is cheap and cloneable — it holds only the data-dir path,
//! the expected generation, and the timeouts. It re-reads and re-validates the
//! portfile on **every** call, so a daemon restart (new port, new token, maybe
//! new PID) heals transparently, and a restart that changed the generation is
//! caught as a [`SupervisorError::GenerationMismatch`] instead of silently
//! attaching to an incompatible daemon.

use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;

use crate::error::SupervisorError;
use crate::generation::Generation;
use crate::redact::Redacted;
use crate::supervisor::Timeouts;
use crate::validate;

/// A cloneable resolver bound to one daemon's data dir and the supervisor's
/// expected generation. Handed to the transport (#172), which calls
/// [`TargetResolver::resolve`] once per connection attempt.
#[derive(Clone)]
pub struct TargetResolver {
    pub(crate) data_dir: PathBuf,
    pub(crate) expected: Generation,
    pub(crate) strict_portfile_perms: bool,
    pub(crate) timeouts: Timeouts,
}

impl fmt::Debug for TargetResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No secret here (the token is read fresh per call, never stored), but
        // keep the shape tidy and stable.
        f.debug_struct("TargetResolver")
            .field("data_dir", &self.data_dir)
            .field("expected", &self.expected)
            .finish_non_exhaustive()
    }
}

impl TargetResolver {
    /// Produce a freshly validated dial target and token, or a typed error if
    /// the portfile is missing, torn, stale, non-loopback, or the wrong
    /// generation. Re-reads the portfile from disk every call and re-runs the
    /// loopback / health-PID / generation checks (spec §6.6).
    pub async fn resolve(&self) -> Result<DialTarget, SupervisorError> {
        // Dial-only: the resolved target is used to connect, never to stop a daemon,
        // so do NOT capture the `daemon.lock` handle — a stalled lock manager must not
        // delay a reconnect (nor leak a probe thread) for a handle this discards.
        let validated = validate::validate_portfile(
            &self.data_dir,
            self.expected,
            self.strict_portfile_perms,
            &self.timeouts,
            false,
        )
        .await?;
        DialTarget::from_validated(&validated.portfile, self.expected)
    }
}

/// The one value the transport dials with: a token-free, `Display`-safe
/// loopback WS URL carrying the generation-gate query, plus the per-start
/// bearer token (private, redacted, attached by the transport as
/// `Authorization: Bearer`).
#[derive(Clone)]
pub struct DialTarget {
    ws_url: url::Url,
    bearer: Redacted<String>,
}

impl DialTarget {
    /// Build a dial target from a validated portfile. The URL is constructed
    /// from a hardcoded `127.0.0.1` and the validated port, so it is loopback by
    /// construction; the generation-gate query carries the *expected* values
    /// (equal to the daemon's, since validation just proved it). **No token in
    /// the URL** (protocol-v2: the credential never travels in a URL).
    pub(crate) fn from_validated(
        portfile: &crate::portfile::Portfile,
        expected: Generation,
    ) -> Result<Self, SupervisorError> {
        let raw = format!(
            "ws://127.0.0.1:{}/ws?v={}&sg={}",
            portfile.port, expected.protocol, expected.storage_generation
        );
        let ws_url = url::Url::parse(&raw).map_err(|e| {
            // A parse failure here is a supervisor bug (the components are all
            // machine-generated), surfaced as a handshake fault rather than a
            // panic.
            SupervisorError::Handshake(format!("could not build the dial URL {raw:?}: {e}"))
        })?;
        Ok(Self {
            ws_url,
            bearer: Redacted::new(portfile.token().to_owned()),
        })
    }

    /// The token-free WebSocket URL. Safe to display, log, or render.
    pub fn ws_url(&self) -> &url::Url {
        &self.ws_url
    }

    /// The per-start bearer token. Native transport only — attach as
    /// `Authorization: Bearer`, never place in a URL, a log, a Dioxus prop, or a
    /// DOM attribute.
    pub fn bearer(&self) -> &str {
        self.bearer.expose()
    }
}

impl fmt::Debug for DialTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Hand-written so the token is `<redacted>` and the URL (already
        // token-free) is shown for diagnosis.
        f.debug_struct("DialTarget")
            .field("ws_url", &self.ws_url.as_str())
            .field("bearer", &self.bearer)
            .finish()
    }
}

/// Whether an advertised `ws`/`http`/`wss`/`https` endpoint string denotes a
/// loopback host. Reuses the daemon's `host_header_is_loopback` semantics:
/// `localhost`, or a host that *parses* as a loopback IP — a name that merely
/// looks loopback (`127.0.0.1.evil.example`) is refused because it does not
/// parse as an IP and is not the literal `localhost`.
pub(crate) fn advertised_endpoint_is_loopback(endpoint: &str) -> bool {
    let Ok(parsed) = url::Url::parse(endpoint) else {
        return false;
    };
    match parsed.host() {
        Some(url::Host::Domain(name)) => name == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => IpAddr::from(ip).is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portfile::Portfile;

    fn portfile(port: u16) -> Portfile {
        serde_json::from_str(&format!(
            r#"{{"pid":1,"port":{port},"protocol":2,"storage_generation":2,
                "data_dir":"/d","auth_token":"sixtyfourhexsecret"}}"#
        ))
        .expect("parses")
    }

    #[test]
    fn dial_url_carries_the_generation_gate_and_no_token() {
        let target =
            DialTarget::from_validated(&portfile(7420), Generation::new(2, 2)).expect("builds");
        assert_eq!(target.ws_url().as_str(), "ws://127.0.0.1:7420/ws?v=2&sg=2");
        assert!(
            !target.ws_url().as_str().contains("sixtyfourhexsecret"),
            "the token must never appear in the URL"
        );
        assert_eq!(target.bearer(), "sixtyfourhexsecret");
    }

    #[test]
    fn dial_target_debug_redacts_the_token() {
        let target =
            DialTarget::from_validated(&portfile(7420), Generation::new(2, 2)).expect("builds");
        let rendered = format!("{target:?}");
        assert!(rendered.contains("ws://127.0.0.1:7420/ws"));
        assert!(
            !rendered.contains("sixtyfourhexsecret"),
            "DialTarget Debug leaked the token: {rendered}"
        );
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn loopback_endpoints_are_accepted() {
        for ok in [
            "ws://127.0.0.1:7420/ws",
            "http://127.0.0.1:7420/",
            "ws://localhost:5173/ws",
            "http://[::1]:7420/",
            "ws://127.0.0.5:9000/ws",
        ] {
            assert!(
                advertised_endpoint_is_loopback(ok),
                "{ok} should be loopback"
            );
        }
    }

    #[test]
    fn non_loopback_or_lookalike_endpoints_are_refused() {
        for bad in [
            "ws://evil.example/ws",
            "http://127.0.0.1.evil.example/",
            "ws://10.0.0.5:7420/ws",
            "http://[2606:4700:4700::1111]/",
            "not a url",
        ] {
            assert!(
                !advertised_endpoint_is_loopback(bad),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn dial_url_uses_hardcoded_loopback_not_portfile_ws_field() {
        // The DialTarget URL is built from 127.0.0.1 + the validated port, not
        // from portfile.ws, so portfile.ws has no influence on what the transport
        // actually dials.
        let pf: Portfile = serde_json::from_str(
            r#"{"pid":1,"port":8080,"protocol":2,
            "storage_generation":2,"data_dir":"/d","auth_token":"abc",
            "ws":"ws://127.0.0.1:8080/ws"}"#,
        )
        .expect("parses");
        let target = DialTarget::from_validated(&pf, Generation::new(2, 2)).expect("builds");
        assert_eq!(target.ws_url().host_str(), Some("127.0.0.1"));
        assert_eq!(target.ws_url().port(), Some(8080));
    }

    #[test]
    fn dial_url_generation_gate_query_params_reflect_expected_generation() {
        // The generation gate (v=…&sg=…) must carry the EXPECTED generation (not
        // whatever the portfile says) so it matches what this binary was built
        // against (protocol-v2 §Layer 1).
        let pf: Portfile = serde_json::from_str(
            r#"{"pid":1,"port":9000,"protocol":3,
            "storage_generation":5,"data_dir":"/d","auth_token":"abc"}"#,
        )
        .expect("parses");
        let target = DialTarget::from_validated(&pf, Generation::new(3, 5)).expect("builds");
        let url = target.ws_url().as_str();
        assert!(url.contains("v=3"), "URL must embed v=3: {url}");
        assert!(url.contains("sg=5"), "URL must embed sg=5: {url}");
    }

    #[test]
    fn dial_url_scheme_is_ws_not_wss() {
        // The supervisor dials the loopback daemon over unencrypted WS; TLS is
        // not in scope here (loopback, private port, token in header not URL).
        let pf: Portfile = serde_json::from_str(
            r#"{"pid":1,"port":7420,"protocol":2,"storage_generation":2,
            "data_dir":"/d","auth_token":"t"}"#,
        )
        .expect("parses");
        let target = DialTarget::from_validated(&pf, Generation::new(2, 2)).expect("builds");
        assert_eq!(target.ws_url().scheme(), "ws");
    }

    #[test]
    fn target_resolver_is_clone_so_a_transport_does_not_hold_the_sidecar() {
        // The whole point of TargetResolver is that the transport can own a cheap
        // clone without owning (or being able to kill) the Sidecar.
        let resolver = TargetResolver {
            data_dir: std::path::PathBuf::from("/tmp"),
            expected: Generation::new(2, 2),
            strict_portfile_perms: false,
            timeouts: crate::supervisor::Timeouts::default(),
        };
        let clone = resolver.clone();
        assert_eq!(resolver.data_dir, clone.data_dir);
        assert_eq!(resolver.expected, clone.expected);
    }
}
