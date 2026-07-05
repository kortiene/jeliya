//! Jeliya resident core — the only crate in the project that imports the
//! `iroh_rooms` SDK (stable tier for authoring/validation, experimental tier
//! for the online runtime), consumed by the `jeliyad` daemon.
//!
//! Modules:
//! * [`fleet`] — pure agent-liveness derivation (the `agents.fleet` /
//!   `agent.history` decision table per `docs/agent-orchestration.md` §1.2).
//! * [`identity`] — create/load the device identity under `--data-dir`
//!   (mirrors the iroh-rooms CLI's `identity.json` / `identity.secret` split).
//! * [`localstate`] — daemon-local JSON state: known-rooms index + local name
//!   overrides. Note: the wire protocol *does* carry a room name
//!   (`room.created.room_name`), so the local name is an index/override, not
//!   the source of truth.
//! * [`materializer`] — pure `StoredEvent -> TimelineEvent` JSON view-models
//!   per `docs/PROTOCOL.md`.
//! * [`supervisor`] — `RoomSupervisor`: one experimental `Node` per open room
//!   (spawned the way the reference CLI spawns its room session), plus the
//!   offline flows (create/invite/join/reads) mirrored from the CLI.

pub mod error;
pub mod fleet;
pub mod identity;
pub mod localstate;
pub mod materializer;
pub mod supervisor;

pub use error::{CoreError, CoreResult, ErrorKind};

/// Wall-clock milliseconds since the Unix epoch (advisory/display only — the
/// protocol never orders by it).
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}
