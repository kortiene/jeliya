//! Jeliya resident core — the only crate in the project that imports the
//! `iroh_rooms` SDK (stable tier for authoring/validation, experimental tier
//! for the online runtime), consumed by the `jeliyad` daemon.
//!
//! Public modules:
//! * [`engine`] — the transport-free protocol-v2 facade: typed calls, typed
//!   replies, and typed push fan-out. Hosts own framing and serialization.
//! * [`fleet`] — pure agent-liveness derivation (the `agents.fleet` /
//!   `agent.history` decision table per `docs/agent-orchestration.md` §1.2).
//! * [`identity`] — create/load the device identity under `--data-dir`
//!   (mirrors the iroh-rooms CLI's `identity.json` / `identity.secret` split).
//! * [`localstate`] — daemon-local JSON state: known-rooms index + local name
//!   overrides. Note: the wire protocol *does* carry a room name
//!   (`room.created.room_name`), so the local name is an index/override, not
//!   the source of truth.
//!
//! The runtime supervisor and the retired v1 JSON materializer are deliberately
//! private. Protocol callers enter through [`Engine`] and receive
//! `jeliya-api` values; JSON exists only in host codecs and in the explicitly
//! out-of-scope daemon-local persistence modules (`identity` and `localstate`).

pub mod engine;
pub mod error;
pub mod fleet;
pub mod identity;
pub mod localstate;
#[cfg(test)]
mod materializer;
mod projection;
mod supervisor;
pub mod typed;

pub use engine::{Engine, EngineConfig, PROTOCOL_VERSION};
pub use error::{CoreError, CoreResult, ErrorKind};

/// Compile-time attestation used only by the controlled relay verifier build.
///
/// This item does not exist in ordinary builds. Enabling Jeliya's diagnostic
/// feature against an upstream SDK without its matching transport feature
/// fails at compile time below; it can never produce a falsely attested
/// direct-capable binary.
#[cfg(feature = "relay-only-test")]
pub const RELAY_ONLY_TEST_BUILD: bool = iroh_rooms::experimental::session::RELAY_ONLY_TEST_BUILD;

#[cfg(feature = "relay-only-test")]
const _: () = assert!(
    RELAY_ONLY_TEST_BUILD,
    "relay-only-test requires the reviewed upstream clear_ip_transports feature"
);

/// Wall-clock milliseconds since the Unix epoch (advisory/display only — the
/// protocol never orders by it).
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}
