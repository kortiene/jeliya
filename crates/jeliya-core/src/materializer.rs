//! Pure `StoredEvent` -> `TimelineEvent` JSON view-models, exactly as
//! `docs/PROTOCOL.md` specifies.
//!
//! One validated room event folds into one JSON object with a stable common
//! header (`event_id`, `room_id`, `ts`, `sender`, `kind`) plus kind-specific
//! fields present only for that kind. Event kinds outside the protocol's
//! displayed set (`member.removed`) fold to `None` and are omitted from
//! timelines. `member.left` is displayed as a lightweight system event so live
//! clients can refresh rosters when someone leaves.

use serde_json::{json, Map, Value};

use iroh_rooms::events::{Content, EventId, RoomId, SignedEvent};
use iroh_rooms::experimental::store::StoredEvent;
use iroh_rooms::identity::IdentityKey;
use iroh_rooms::room::{MembershipSnapshot, Role};

/// Map a fold [`Role`] to the protocol's `owner|member|agent` vocabulary.
/// (The SDK calls the room creator `admin`; the protocol calls it `owner`.)
#[must_use]
pub fn role_label(role: Role) -> &'static str {
    match role {
        Role::Admin => "owner",
        Role::Member => "member",
        Role::Agent => "agent",
    }
}

/// Map an on-wire role string (`admin|member|agent`) to the protocol
/// vocabulary; unknown strings pass through unchanged.
fn wire_role_label(role: &str) -> &str {
    if role == "admin" {
        "owner"
    } else {
        role
    }
}

/// The sender's current role, from the membership fold; a sender the fold does
/// not know (should not happen for a validated event) reads `member`.
fn sender_role(snapshot: &MembershipSnapshot, sender: &IdentityKey) -> &'static str {
    snapshot.role(sender).map_or("member", role_label)
}

/// The bare 64-hex form of an event id (the protocol strips the `blake3:`
/// prefix for `event_id`; `room_id` keeps it).
#[must_use]
pub fn bare_event_hex(event_id: &EventId) -> String {
    let s = event_id.to_string();
    match s.strip_prefix("blake3:") {
        Some(hex_part) => hex_part.to_owned(),
        None => s,
    }
}

/// The `file_<32-hex>` handle for a 16-byte on-wire short id (mirrors the
/// reference CLI's `file_handle`).
#[must_use]
pub fn file_handle(file_id: &[u8; 16]) -> String {
    format!("file_{}", hex::encode(file_id))
}

/// Fold one stored event into its protocol `TimelineEvent`, or `None` for an
/// event kind the protocol does not display. Pure: no IO, no clock.
#[must_use]
pub fn materialize(se: &StoredEvent, snapshot: &MembershipSnapshot) -> Option<Value> {
    let ev = SignedEvent::decode(&se.wire.signed).ok()?;
    materialize_signed(&se.room_id, &se.event_id, &ev, snapshot)
}

/// Fold one decoded signed event into its protocol `TimelineEvent`.
#[must_use]
pub fn materialize_signed(
    room_id: &RoomId,
    event_id: &EventId,
    ev: &SignedEvent,
    snapshot: &MembershipSnapshot,
) -> Option<Value> {
    let (kind, extra) = kind_fields(&ev.content)?;
    let mut obj = Map::new();
    obj.insert("event_id".into(), json!(bare_event_hex(event_id)));
    obj.insert("room_id".into(), json!(room_id.to_string()));
    obj.insert("ts".into(), json!(ev.created_at));
    obj.insert(
        "sender".into(),
        json!({
            "identity_id": ev.sender_id.to_string(),
            "device_id": ev.device_id.to_string(),
            "role": sender_role(snapshot, &ev.sender_id),
        }),
    );
    obj.insert("kind".into(), json!(kind));
    for (k, v) in extra {
        obj.insert(k, v);
    }
    Some(Value::Object(obj))
}

/// The protocol `kind` plus its kind-specific fields, or `None` for kinds the
/// protocol does not enumerate (`member.removed`).
fn kind_fields(content: &Content) -> Option<(&'static str, Map<String, Value>)> {
    let mut m = Map::new();
    let kind = match content {
        Content::RoomCreated(_) => "room_created",
        Content::MemberInvited(c) => {
            m.insert(
                "member".into(),
                json!({
                    "identity_id": c.invitee_key.to_string(),
                    "role": wire_role_label(&c.role),
                }),
            );
            "member_invited"
        }
        Content::MemberJoined(c) => {
            // The joiner is the event's sender; its identity lands in the
            // common `sender` header, and the membership summary mirrors it.
            m.insert(
                "member".into(),
                json!({
                    "identity_id": c.device_binding.identity_key.to_string(),
                    "role": wire_role_label(&c.role),
                }),
            );
            "member_joined"
        }
        Content::MessageText(c) => {
            m.insert("body".into(), json!(c.body));
            "message"
        }
        Content::AgentStatus(c) => {
            m.insert("label".into(), json!(c.status));
            if let Some(msg) = &c.message {
                m.insert("status_message".into(), json!(msg));
            }
            if let Some(pct) = c.progress_pct {
                m.insert("progress".into(), json!(pct));
            }
            if let Some(ids) = &c.related_artifact_ids {
                if !ids.is_empty() {
                    let handles: Vec<String> = ids.iter().map(file_handle).collect();
                    m.insert("artifacts".into(), json!(handles));
                }
            }
            "agent_status"
        }
        Content::FileShared(c) => {
            m.insert(
                "file".into(),
                json!({
                    "file_id": file_handle(&c.file_id),
                    "name": c.name,
                    "size": c.size_bytes,
                    "mime": c.mime_type,
                }),
            );
            "file_shared"
        }
        Content::PipeOpened(c) => {
            // Every authorized peer, not just the first: a validated remote
            // pipe.opened may declare several, and dropping the rest would
            // misrepresent who can reach the exposed loopback target. Our own
            // pipe.expose authorizes exactly one, so the common case is a single
            // identity unchanged.
            let authorized_peer = if c.allowed_members.is_empty() {
                Value::Null
            } else {
                Value::String(
                    c.allowed_members
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(","),
                )
            };
            m.insert(
                "pipe".into(),
                json!({
                    "pipe_id": hex::encode(c.pipe_id),
                    "target": c.target_hint,
                    "authorized_peer": authorized_peer,
                }),
            );
            "pipe_opened"
        }
        Content::PipeClosed(c) => {
            // A pipe.closed carries only the pipe id on the wire; target and
            // authorized_peer are honest nulls (they live on the pipe.opened).
            m.insert(
                "pipe".into(),
                json!({
                    "pipe_id": hex::encode(c.pipe_id),
                    "target": Value::Null,
                    "authorized_peer": Value::Null,
                }),
            );
            "pipe_closed"
        }
        Content::MemberLeft(c) => {
            m.insert(
                "member".into(),
                json!({
                    "identity_id": c.member_id.to_string(),
                }),
            );
            "member_left"
        }
        // Not part of the protocol's TimelineEvent kind enumeration.
        Content::MemberRemoved(_) => return None,
    };
    Some((kind, m))
}

#[cfg(test)]
mod tests {
    use super::{bare_event_hex, materialize, materialize_signed, role_label};
    use iroh_rooms::events::{
        build_agent_status, build_message_text, validate_wire_bytes, SignedEvent,
        ValidationContext, WireEvent,
    };
    use iroh_rooms::experimental::store::EventStore;
    use iroh_rooms::files::{build_file_shared, HashRef};
    use iroh_rooms::identity::{DeviceBinding, SigningKey};
    use iroh_rooms::pipes::{build_pipe_closed, build_pipe_opened};
    use iroh_rooms::room::{
        build_member_invited, build_member_joined, build_member_left, build_room_created,
        derive_room_id, MembershipSnapshot, Role, RoomId, RoomMembership,
    };

    const TS: u64 = 1_783_190_000_000;

    struct Fixture {
        identity: SigningKey,
        device: SigningKey,
        room_id: RoomId,
        genesis: WireEvent,
    }

    fn fixture() -> Fixture {
        let identity = SigningKey::generate();
        let device = SigningKey::generate();
        let nonce = [0x42u8; 16];
        let room_id = derive_room_id(&identity.identity_key(), &nonce, TS);
        let genesis = build_room_created(&identity, &device, "Build Iroh Rooms MVP", &nonce, TS);
        Fixture {
            identity,
            device,
            room_id,
            genesis,
        }
    }

    /// Fold the genesis so the sender resolves as admin/"owner".
    fn snapshot_of(fx: &Fixture) -> MembershipSnapshot {
        let ctx = ValidationContext::for_room(fx.room_id);
        let validated = validate_wire_bytes(&fx.genesis.to_bytes(), &ctx).expect("genesis valid");
        RoomMembership::from_events(fx.room_id, vec![validated]).snapshot()
    }

    fn decode(wire: &WireEvent) -> SignedEvent {
        SignedEvent::decode(&wire.signed).expect("authored event decodes")
    }

    fn mat(fx: &Fixture, wire: &WireEvent) -> serde_json::Value {
        let snapshot = snapshot_of(fx);
        let ev = decode(wire);
        let ctx = ValidationContext::for_room(fx.room_id);
        // Use the validator's recomputed event id (the same id the store keys
        // by) when the fixture validates statelessly; kind-only fixtures that
        // deliberately skip a real invite handshake (member.joined) fall back
        // to a fixed placeholder id — the shape assertions never depend on it.
        let event_id = validate_wire_bytes(&wire.to_bytes(), &ctx)
            .map_or(iroh_rooms::events::EventId::from_bytes([0x0f; 32]), |v| {
                v.event_id
            });
        materialize_signed(&fx.room_id, &event_id, &ev, &snapshot).expect("materializes")
    }

    #[test]
    fn room_created_has_owner_sender_and_bare_hex_event_id() {
        let fx = fixture();
        let v = mat(&fx, &fx.genesis);
        assert_eq!(v["kind"], "room_created");
        assert_eq!(v["room_id"], fx.room_id.to_string());
        assert_eq!(v["ts"], TS);
        assert_eq!(v["sender"]["role"], "owner");
        assert_eq!(
            v["sender"]["identity_id"],
            fx.identity.identity_key().to_string()
        );
        assert_eq!(v["sender"]["device_id"], fx.device.device_key().to_string());
        let id = v["event_id"].as_str().unwrap();
        assert_eq!(id.len(), 64, "event_id must be bare 64-hex");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn message_carries_body() {
        let fx = fixture();
        let wire = build_message_text(
            &fx.identity,
            &fx.device,
            &fx.room_id,
            "hello",
            None,
            None,
            &[],
            &[],
            TS + 1,
        );
        let v = mat(&fx, &wire);
        assert_eq!(v["kind"], "message");
        assert_eq!(v["body"], "hello");
        assert!(v.get("file").is_none(), "kind-specific fields only");
    }

    #[test]
    fn member_invited_names_the_invitee() {
        let fx = fixture();
        let invitee = SigningKey::generate().identity_key();
        let wire = build_member_invited(
            &fx.identity,
            &fx.device,
            &fx.room_id,
            &[0x01; 16],
            &[0x02; 32],
            "member",
            &invitee,
            None,
            None,
            &[],
            TS + 1,
        );
        let v = mat(&fx, &wire);
        assert_eq!(v["kind"], "member_invited");
        assert_eq!(v["member"]["identity_id"], invitee.to_string());
        assert_eq!(v["member"]["role"], "member");
    }

    #[test]
    fn member_joined_names_the_joiner_and_role() {
        let fx = fixture();
        let joiner_identity = SigningKey::generate();
        let joiner_device = SigningKey::generate();
        let binding =
            DeviceBinding::create(&fx.room_id, &joiner_identity, joiner_device.device_key());
        let wire = build_member_joined(
            &joiner_identity,
            &joiner_device,
            &fx.room_id,
            &[0x01; 16],
            &[0x03; 16],
            "agent",
            binding,
            Some("Robo"),
            &[],
            TS + 2,
        );
        let v = mat(&fx, &wire);
        assert_eq!(v["kind"], "member_joined");
        assert_eq!(
            v["member"]["identity_id"],
            joiner_identity.identity_key().to_string()
        );
        assert_eq!(v["member"]["role"], "agent");
    }

    #[test]
    fn agent_status_maps_label_message_progress_artifacts() {
        let fx = fixture();
        let wire = build_agent_status(
            &fx.identity,
            &fx.device,
            &fx.room_id,
            "running_tests",
            Some("suite in progress"),
            &[[0xaa; 16], [0xbb; 16]],
            Some(60),
            &[],
            TS + 1,
        );
        let v = mat(&fx, &wire);
        assert_eq!(v["kind"], "agent_status");
        assert_eq!(v["label"], "running_tests");
        assert_eq!(v["status_message"], "suite in progress");
        assert_eq!(v["progress"], 60);
        let artifacts = v["artifacts"].as_array().unwrap();
        assert_eq!(artifacts.len(), 2);
        assert_eq!(
            artifacts[0].as_str().unwrap(),
            format!("file_{}", "aa".repeat(16))
        );
    }

    #[test]
    fn agent_status_omits_optionals_when_absent() {
        let fx = fixture();
        let wire = build_agent_status(
            &fx.identity,
            &fx.device,
            &fx.room_id,
            "idle",
            None,
            &[],
            None,
            &[],
            TS + 1,
        );
        let v = mat(&fx, &wire);
        assert_eq!(v["label"], "idle");
        assert!(v.get("status_message").is_none());
        assert!(v.get("progress").is_none());
        assert!(v.get("artifacts").is_none());
    }

    #[test]
    fn file_shared_maps_the_file_object() {
        let fx = fixture();
        let wire = build_file_shared(
            &fx.identity,
            &fx.device,
            &fx.room_id,
            [0x11; 16],
            "PRD.pdf",
            "application/pdf",
            123,
            HashRef::from_bytes([0xcc; 32]),
            Some("raw"),
            &[fx.device.device_key()],
            &[],
            TS + 1,
        );
        let v = mat(&fx, &wire);
        assert_eq!(v["kind"], "file_shared");
        assert_eq!(
            v["file"]["file_id"].as_str().unwrap(),
            format!("file_{}", "11".repeat(16))
        );
        assert_eq!(v["file"]["name"], "PRD.pdf");
        assert_eq!(v["file"]["size"], 123);
        assert_eq!(v["file"]["mime"], "application/pdf");
    }

    #[test]
    fn pipe_opened_and_closed_map_the_pipe_object() {
        let fx = fixture();
        let peer = SigningKey::generate().identity_key();
        let opened = build_pipe_opened(
            &fx.identity,
            &fx.device,
            &fx.room_id,
            [0xef; 16],
            &fx.device.device_key(),
            "dev",
            "127.0.0.1:3000",
            "iroh-rooms/pipe/1",
            &[peer],
            None,
            &[],
            TS + 1,
        );
        let v = mat(&fx, &opened);
        assert_eq!(v["kind"], "pipe_opened");
        assert_eq!(v["pipe"]["pipe_id"], "ef".repeat(16));
        assert_eq!(v["pipe"]["target"], "127.0.0.1:3000");
        assert_eq!(v["pipe"]["authorized_peer"], peer.to_string());

        let closed = build_pipe_closed(
            &fx.identity,
            &fx.device,
            &fx.room_id,
            [0xef; 16],
            Some("closed"),
            &[],
            TS + 2,
        );
        let v = mat(&fx, &closed);
        assert_eq!(v["kind"], "pipe_closed");
        assert_eq!(v["pipe"]["pipe_id"], "ef".repeat(16));
        assert!(v["pipe"]["target"].is_null());
        assert!(v["pipe"]["authorized_peer"].is_null());
    }

    #[test]
    fn member_left_is_a_system_timeline_kind() {
        let fx = fixture();
        let wire = build_member_left(&fx.identity, &fx.device, &fx.room_id, None, &[], TS + 1);
        let snapshot = snapshot_of(&fx);
        let ev = decode(&wire);
        let id = iroh_rooms::events::EventId::from_bytes([0x01; 32]);
        let v = materialize_signed(&fx.room_id, &id, &ev, &snapshot).expect("left materializes");
        assert_eq!(v["kind"], "member_left");
        assert_eq!(
            v["member"]["identity_id"],
            fx.identity.identity_key().to_string()
        );
    }

    #[test]
    fn materialize_stored_event_via_store_roundtrip() {
        // End-to-end through the real store: author -> validate -> insert ->
        // room_tail -> materialize, the exact path the daemon uses.
        let fx = fixture();
        let mut store = EventStore::open_in_memory().unwrap();
        let ctx = ValidationContext::for_room(fx.room_id);
        let genesis = validate_wire_bytes(&fx.genesis.to_bytes(), &ctx).unwrap();
        store.insert(&genesis).unwrap();
        let msg_wire = build_message_text(
            &fx.identity,
            &fx.device,
            &fx.room_id,
            "hi from the store",
            None,
            None,
            &[],
            &[genesis.event_id],
            TS + 5,
        );
        let msg = validate_wire_bytes(&msg_wire.to_bytes(), &ctx).unwrap();
        store.insert(&msg).unwrap();

        let snapshot = snapshot_of(&fx);
        let rows = store.room_tail(&fx.room_id, 100).unwrap();
        let views: Vec<serde_json::Value> = rows
            .iter()
            .filter_map(|se| materialize(se, &snapshot))
            .collect();
        assert_eq!(views.len(), 2);
        assert_eq!(views[0]["kind"], "room_created");
        assert_eq!(views[1]["kind"], "message");
        assert_eq!(views[1]["body"], "hi from the store");
        assert_eq!(views[1]["event_id"], bare_event_hex(&msg.event_id));
    }

    #[test]
    fn role_labels_use_protocol_vocabulary() {
        assert_eq!(role_label(Role::Admin), "owner");
        assert_eq!(role_label(Role::Member), "member");
        assert_eq!(role_label(Role::Agent), "agent");
    }

    #[test]
    fn unknown_sender_defaults_to_member_role() {
        let fx = fixture();
        let stranger_identity = SigningKey::generate();
        let stranger_device = SigningKey::generate();
        let wire = build_message_text(
            &stranger_identity,
            &stranger_device,
            &fx.room_id,
            "hi",
            None,
            None,
            &[],
            &[],
            TS,
        );
        let snapshot = snapshot_of(&fx);
        let ev = decode(&wire);
        let id = iroh_rooms::events::EventId::from_bytes([0x02; 32]);
        let v = materialize_signed(&fx.room_id, &id, &ev, &snapshot).unwrap();
        assert_eq!(v["sender"]["role"], "member");
    }
}
