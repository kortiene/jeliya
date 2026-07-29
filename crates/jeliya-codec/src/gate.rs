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
    /// The served `max_connections` limit, returned in the capacity
    /// refusal's typed `limit` field — a refusal that reports a limit it
    /// does not have is a false limit on the wire.
    pub max_connections: u64,
    /// The `cid` query parameter, if present. **Absence is not refusal** —
    /// an omitted `cid` yields a fresh ephemeral principal, the documented
    /// choice a short-lived CLI makes. `cid` is not a credential and is
    /// never compared in constant time.
    pub cid: Option<String>,
    /// The daemon's storage generation.
    pub daemon_sg: u64,
    /// The expected credential, compared in constant time.
    pub expected_credential: String,
}

/// The gate's verdict.
#[derive(Debug)]
pub enum GateDecision {
    /// The upgrade proceeds, yielding the authenticated session principal.
    Admit(SessionPrincipal),
    /// The upgrade is refused with a bare error body and an HTTP status.
    Refuse(GateRejection),
}

/// The authenticated session principal: `(credential, client_id)`. It keys
/// the dedup ledger and transfer-cancellation isolation. Distinct
/// `client_id`s have isolated ledgers; reusing another's shares its
/// `op_id` scope, which is the same relationship as sharing a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPrincipal {
    /// The authenticated credential identity.
    pub credential: String,
    /// The declared `client_id`, or `None` for a fresh ephemeral principal.
    pub client_id: Option<String>,
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
                limit: params.max_connections,
            },
            status: 503,
        }));
    }
    Ok(GateDecision::Admit(SessionPrincipal {
        credential: params.expected_credential.clone(),
        client_id: params.cid.clone(),
    }))
}

/// A constant-time byte comparison. The credential is a shared secret, so
/// the comparison never short-circuits — and it must not leak the expected
/// length either: a length check that returns before doing byte work lets a
/// loopback attacker recover the expected length by timing candidate
/// lengths. The length is folded into the accumulator instead, and the
/// comparison always traverses the longer input.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let max_len = a.len().max(b.len());
    let mut diff = (a.len() ^ b.len()) as u8;
    for i in 0..max_len {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    diff == 0
}

/// `Host` is loopback when its host part is a loopback literal or
/// `localhost`. An IPv6 literal is bracketed (`[::1]:8080`), so the port is
/// stripped after the closing bracket, not by splitting on the first colon —
/// splitting an IPv6 authority on `:` yields `[`, which is not a host.
fn is_loopback_authority(host: &str) -> bool {
    let host_part = if let Some(bracket_end) = host.find(']') {
        // [::1] or [::1]:port
        &host[..=bracket_end]
    } else {
        // 127.0.0.1[:port], localhost[:port], or ::1
        host.split(':').next().unwrap_or(host)
    };
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
            max_connections: 64,
            cid: None,
            daemon_sg: 1,
            expected_credential: "tok".into(),
        }
    }

    #[test]
    fn admits_a_valid_upgrade() {
        assert!(matches!(gate(&base()).unwrap(), GateDecision::Admit(_)));
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
