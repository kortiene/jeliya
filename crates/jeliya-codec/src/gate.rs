//! The generation gate. Runs on the upgrade request, **before** the
//! WebSocket upgrade is performed, before any frame is parsed, and before
//! any dispatch — the only point provably before mutation.

use crate::error::GateRejection;
use crate::{CodecError, MIN_PROTOCOL, PROTOCOL};
use jeliya_api::{ApiError, DeclaredGeneration, DeclaredVersion};

/// The gate's inputs, parsed from the upgrade request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateParams {
    /// The `Host` header.
    pub host: String,
    /// The `Origin` header, if present.
    pub origin: Option<String>,
    /// The `v` query parameter, if present.
    pub v: Option<u64>,
    /// The `sg` query parameter, if present.
    pub sg: Option<u64>,
    /// The presented credential, if present.
    pub credential: Option<String>,
    /// Whether the daemon is at connection capacity.
    pub at_capacity: bool,
    /// The daemon's storage generation.
    pub daemon_sg: u64,
    /// The expected credential, compared in constant time.
    pub expected_credential: String,
}

/// The gate's verdict.
#[derive(Debug)]
pub enum GateDecision {
    /// The upgrade proceeds.
    Admit,
    /// The upgrade is refused with a bare error body and an HTTP status.
    Refuse(GateRejection),
}

/// Runs the six checks in the record's fixed order: (1) loopback `Host`,
/// (2) loopback `Origin` if present, (3) present and supported `v`,
/// (4) present and equal `sg`, (5) credential — and capacity **last**,
/// after the credential, so an unauthenticated caller cannot use the
/// capacity check as an occupancy oracle.
pub fn gate(params: &GateParams) -> Result<GateDecision, CodecError> {
    // 1. Host must be loopback.
    if !is_loopback_authority(&params.host) {
        return Ok(GateDecision::Refuse(GateRejection {
            body: ApiError::ForbiddenOrigin,
            status: 403,
        }));
    }
    // 2. Origin, if present, must be loopback.
    if let Some(origin) = &params.origin {
        if !is_loopback_origin(origin) {
            return Ok(GateDecision::Refuse(GateRejection {
                body: ApiError::ForbiddenOrigin,
                status: 403,
            }));
        }
    }
    // 3. v must be present and name a supported generation. Absence is
    //    refusal, never a default — a v1 client sends no v, and a missing
    //    generation that defaulted to current would admit every legacy client.
    let supported = params.v.is_some_and(|v| v >= MIN_PROTOCOL && v <= PROTOCOL);
    if !supported {
        return Ok(GateDecision::Refuse(GateRejection {
            body: ApiError::ProtocolUnsupported {
                supported: vec![PROTOCOL],
                client: match params.v {
                    Some(v) => DeclaredVersion::Declared { v },
                    None => DeclaredVersion::Absent,
                },
            },
            status: 426,
        }));
    }
    // 4. sg must be present and equal the daemon's storage generation.
    if params.sg != Some(params.daemon_sg) {
        return Ok(GateDecision::Refuse(GateRejection {
            body: ApiError::StorageGenerationMismatch {
                daemon: params.daemon_sg,
                client: match params.sg {
                    Some(sg) => DeclaredGeneration::Declared { sg },
                    None => DeclaredGeneration::Absent,
                },
            },
            status: 426,
        }));
    }
    // 5. Credential, compared in constant time.
    let ok = match (&params.credential, &params.expected_credential) {
        (Some(presented), expected) => constant_time_eq(presented.as_bytes(), expected.as_bytes()),
        (None, _) => false,
    };
    if !ok {
        return Ok(GateDecision::Refuse(GateRejection {
            body: ApiError::Unauthenticated,
            status: 401,
        }));
    }
    // 6. Capacity, checked last and the only transient refusal — 503, not 4xx.
    if params.at_capacity {
        return Ok(GateDecision::Refuse(GateRejection {
            body: ApiError::ResourceExhausted {
                resource: "max_connections".into(),
                limit: 0, // the served value is filled by the daemon, which owns it
            },
            status: 503,
        }));
    }
    Ok(GateDecision::Admit)
}

/// A constant-time byte comparison. The credential is a shared secret, so
/// the comparison never short-circuits on a mismatch.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// `Host` is loopback when its host part is a loopback literal or `localhost`.
fn is_loopback_authority(host: &str) -> bool {
    let host_part = host.split(':').next().unwrap_or(host);
    matches!(host_part, "127.0.0.1" | "localhost" | "[::1]" | "::1")
}

/// `Origin` is loopback when its authority is loopback.
fn is_loopback_origin(origin: &str) -> bool {
    // Origin is scheme://authority[/]; extract the authority.
    let authority = origin
        .split("://")
        .nth(1)
        .unwrap_or(origin)
        .split('/')
        .next()
        .unwrap_or("");
    is_loopback_authority(authority)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> GateParams {
        GateParams {
            host: "127.0.0.1:8080".into(),
            origin: None,
            v: Some(2),
            sg: Some(1),
            credential: Some("tok".into()),
            at_capacity: false,
            daemon_sg: 1,
            expected_credential: "tok".into(),
        }
    }

    #[test]
    fn admits_a_valid_upgrade() {
        assert!(matches!(gate(&base()).unwrap(), GateDecision::Admit));
    }

    #[test]
    fn refuses_a_non_loopback_host() {
        let mut p = base();
        p.host = "evil.example.com".into();
        let d = gate(&p).unwrap();
        assert!(matches!(d, GateDecision::Refuse(_)));
    }

    #[test]
    fn refuses_a_non_loopback_origin() {
        let mut p = base();
        p.origin = Some("https://evil.example.com".into());
        let d = gate(&p).unwrap();
        assert!(matches!(d, GateDecision::Refuse(_)));
    }

    #[test]
    fn absence_of_v_is_refusal_never_a_default() {
        let mut p = base();
        p.v = None;
        let d = gate(&p).unwrap();
        match d {
            GateDecision::Refuse(r) => {
                assert_eq!(r.status, 426);
                assert!(matches!(r.body, ApiError::ProtocolUnsupported { .. }));
            }
            _ => panic!("expected refusal"),
        }
    }

    #[test]
    fn a_v1_generation_is_refused() {
        let mut p = base();
        p.v = Some(1);
        let d = gate(&p).unwrap();
        assert!(matches!(d, GateDecision::Refuse(_)));
    }

    #[test]
    fn a_storage_generation_mismatch_is_refused() {
        let mut p = base();
        p.sg = Some(0);
        let d = gate(&p).unwrap();
        match d {
            GateDecision::Refuse(r) => {
                assert_eq!(r.status, 426);
                assert!(matches!(r.body, ApiError::StorageGenerationMismatch { .. }));
            }
            _ => panic!("expected refusal"),
        }
    }

    #[test]
    fn a_wrong_credential_is_refused() {
        let mut p = base();
        p.credential = Some("wrong".into());
        let d = gate(&p).unwrap();
        match d {
            GateDecision::Refuse(r) => assert_eq!(r.status, 401),
            _ => panic!("expected refusal"),
        }
    }

    #[test]
    fn capacity_is_checked_after_the_credential() {
        // an unauthenticated caller at capacity gets 401, not 503 — the
        // capacity check is not an occupancy oracle.
        let mut p = base();
        p.credential = Some("wrong".into());
        p.at_capacity = true;
        let d = gate(&p).unwrap();
        match d {
            GateDecision::Refuse(r) => assert_eq!(r.status, 401),
            _ => panic!("expected refusal"),
        }
        // an authenticated caller at capacity gets 503.
        let mut p2 = base();
        p2.at_capacity = true;
        let d2 = gate(&p2).unwrap();
        match d2 {
            GateDecision::Refuse(r) => assert_eq!(r.status, 503),
            _ => panic!("expected refusal"),
        }
    }

    #[test]
    fn constant_time_compare() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
