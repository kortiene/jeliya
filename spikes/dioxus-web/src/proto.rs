//! Wire types for the #158 spike, transcribed from `docs/PROTOCOL.md` and
//! `ui/src/lib/protocol.ts`.
//!
//! **This is protocol v1 and it is NOT a compatibility baseline.** The spike
//! uses the current daemon as a disposable fixture so it can answer a rendering
//! and packaging question without waiting for protocol v2. Nothing here
//! constrains what #161 specifies: v2 may retain, rename, combine, or remove
//! any of it. Do not lift these structs into `jeliya-api`.

use serde::{Deserialize, Serialize};

/// Client -> server call. Correlated by numeric `id`.
#[derive(Serialize)]
pub struct Request<'a, P> {
    pub id: u64,
    pub method: &'a str,
    pub params: P,
}

/// Server -> client frame: either a reply (`id` present) or a push (`push`).
#[derive(Deserialize)]
pub struct Frame {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub ok: Option<bool>,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<DaemonError>,
    #[serde(default)]
    pub push: Option<String>,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct DaemonError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub hint: Option<String>,
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, " ({hint})")?;
        }
        Ok(())
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct Identity {
    pub identity_id: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct DaemonStatus {
    pub version: String,
    pub protocol: u32,
    #[serde(default)]
    pub identity: Option<Identity>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct RoomSummary {
    pub room_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub member_count: u32,
    #[serde(default)]
    pub open: bool,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Sender {
    #[serde(default)]
    pub identity_id: String,
    #[serde(default)]
    pub device_id: String,
    /// "owner" | "member" | "agent". Kept an open string: rule 2 of the
    /// forward-compatibility contract makes rejecting an unknown value
    /// non-conformant.
    #[serde(default)]
    pub role: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct TimelineEvent {
    pub event_id: String,
    #[serde(default)]
    pub room_id: String,
    #[serde(default)]
    pub ts: f64,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub sender: Option<Sender>,
}

#[derive(Deserialize)]
pub struct RoomList {
    pub rooms: Vec<RoomSummary>,
}

#[derive(Deserialize)]
pub struct RoomOpened {
    #[serde(default)]
    pub timeline: Vec<TimelineEvent>,
}

#[derive(Deserialize)]
pub struct RoomCreated {
    pub room_id: String,
}

#[derive(Deserialize)]
pub struct SessionToken {
    pub token: String,
}

/// Payload of a `room.event` push.
#[derive(Deserialize)]
pub struct RoomEventPush {
    pub room_id: String,
    pub event: TimelineEvent,
}
