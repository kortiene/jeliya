//! The publish flow (spec §5.C, D6): validate the target is loopback
//! **client-side before** `pipe.publish`, so a hostile target costs no round
//! trip and reveals nothing.
//!
//! The daemon enforces the loopback policy authoritatively (`pipe_target_refused`
//! carrying the rejected target verbatim); this mirrors it up front. A
//! private-range (`192.168.x`), `0.0.0.0`, public, or out-of-range-port target is
//! refused locally. `PipeTargetRefused` (fix the target) and `PolicyRefused`
//! (publishing not permitted here) stay distinct outcomes.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use jeliya_api::{Audience, PipeId, PipePublish, RoomId, Target};
use jeliya_client::{ClientHandle, Dedup};
use jeliya_platform::CancelToken;

use crate::files::errors::{FlowFailure, Settled};

use super::connect::race_or;

/// The success payload of a publish: the new pipe's id.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PublishDone {
    /// The published pipe's id.
    pub pipe_id: PipeId,
}

/// Whether a host string is loopback (`127.0.0.0/8` or `::1`). Parses as an IP
/// and inspects it — a bare `localhost` is *not* accepted (the target is a
/// concrete `host:port`, and DNS is out of the loopback guarantee).
fn is_loopback_host(host: &str) -> bool {
    // Trim IPv6 brackets if present (`[::1]`).
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4.octets()[0] == 127 && v4 != Ipv4Addr::UNSPECIFIED,
        Ok(IpAddr::V6(v6)) => v6 == Ipv6Addr::LOCALHOST,
        Err(_) => false,
    }
}

/// Validate a publish target is loopback with an in-range port, **before** any
/// round trip. `Err(FlowFailure::PipeTargetRefused)` mirrors the daemon's
/// refusal; the rejected target is not echoed into the failure (spec D7).
pub fn validate_loopback_target(target: &Target) -> Result<(), FlowFailure> {
    let port_ok = (1..=65535).contains(&target.port);
    if is_loopback_host(&target.host) && port_ok {
        Ok(())
    } else {
        Err(FlowFailure::PipeTargetRefused)
    }
}

/// Run one `pipe.publish` to settlement, validating the target client-side
/// first. `audience` is required and stated explicitly by the caller (never
/// defaulted to `room`). Publishing is a signed mutation, so it is issued
/// without replay (`Dedup::None`) and never auto-replayed.
pub async fn run_publish(
    handle: &ClientHandle,
    room_id: RoomId,
    target: Target,
    audience: Audience,
    ct: &CancelToken,
) -> Settled<PublishDone> {
    if let Err(failure) = validate_loopback_target(&target) {
        return Settled::Failed(failure);
    }
    let call = handle.call::<PipePublish>(
        PipePublish {
            room_id,
            target,
            audience,
        },
        Dedup::None,
    );
    match race_or(call, ct).await {
        Ok(out) => Settled::Ok(PublishDone {
            pipe_id: out.pipe_id,
        }),
        Err(stop) => Settled::from_stop(stop),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(host: &str, port: u64) -> Target {
        Target {
            host: host.to_string(),
            port,
        }
    }

    #[test]
    fn loopback_targets_are_accepted() {
        for (host, port) in [
            ("127.0.0.1", 8080),
            ("127.5.6.7", 1),
            ("::1", 65535),
            ("[::1]", 22),
        ] {
            assert!(
                validate_loopback_target(&target(host, port)).is_ok(),
                "{host}:{port} is loopback"
            );
        }
    }

    #[test]
    fn non_loopback_and_bad_ports_are_refused_locally() {
        for (host, port) in [
            ("192.168.1.10", 8080),
            ("10.0.0.1", 22),
            ("0.0.0.0", 80),
            ("example.com", 443),
            ("localhost", 8080),
            ("127.0.0.1", 0),
            ("127.0.0.1", 65536),
            ("::2", 22),
        ] {
            assert_eq!(
                validate_loopback_target(&target(host, port)),
                Err(FlowFailure::PipeTargetRefused),
                "{host}:{port} must be refused"
            );
        }
    }

    #[test]
    fn the_refusal_carries_no_target_payload() {
        // The rejected target is not embedded in the failure discriminant (D7).
        let failure = validate_loopback_target(&target("192.168.1.10", 8080)).unwrap_err();
        assert_eq!(failure, FlowFailure::PipeTargetRefused);
    }
}
