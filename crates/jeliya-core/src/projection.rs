//! Pure conversions from internal / Iroh structures to the typed
//! protocol-v2 `jeliya-api` projections (#165). This module is the explicit
//! boundary the issue names: everything protocol-facing that the materializer
//! and supervisor serve is built here as a `jeliya-api` value, and no
//! `serde_json::Value` appears in any of these shapes.
//!
//! The authoritative statement of every shape is `docs/protocol-v2.md`; where
//! this module and that record disagree, the record is right and this module
//! has a bug. Internal persistence JSON (`localstate.rs`, `identity.rs`) is
//! out of scope and stays JSON.

use jeliya_api::{
    Audience, Author, Cursor, DeviceId, Event, EventId, EventKind, EventKindContent, FileId,
    InviteId, LastEvent, LastSeen, LatestStatus, Link, LinkReason, Liveness, PipeId, Progress,
    Role, RoomId, Severity, Standing, StatusLabel, SubjectId, Target, Timestamp, Truncated,
};
use time::OffsetDateTime;

use iroh_rooms::events::{Content, SignedEvent};
use iroh_rooms::experimental::store::StoredEvent;
use iroh_rooms::identity::IdentityKey;
use iroh_rooms::room::{MembershipSnapshot, Role as IrohRole};

use crate::error::{CoreError, CoreResult};
use crate::materializer::{bare_event_hex, file_handle};

/// Convert a signed author-dated `created_at` (ms since the Unix epoch, the
/// non-repudiable author clock) into the wire `<ts>` (RFC 3339, `Z` offset).
///
/// The signed timestamp is the only clock a projection may serve; the wall
/// clock is never read here. Milliseconds are truncated to whole seconds
/// because RFC 3339 has no fractional requirement and the `<ts>` grammar is
/// second-precision.
fn ts(created_at_ms: u64) -> Timestamp {
    let secs = i64::try_from(created_at_ms / 1000).unwrap_or(i64::MAX);
    Timestamp::new(OffsetDateTime::from_unix_timestamp(secs).unwrap_or(OffsetDateTime::UNIX_EPOCH))
}

/// The room's per-event monotonic position, from the store's derived Lamport
/// clock. The genesis is `0`, matching the record's position space anchored
/// at the origin. A causally-incomplete row (a missing parent leaves
/// `lamport` unset) is not materialized into a committed timeline event.
fn pos(se: &StoredEvent) -> Option<u64> {
    se.lamport
}

/// Map an Iroh fold role to the v2 `role` vocabulary. `Admin` is the room's
/// authority; `Member` and `Agent` are both `member` — agent-ness is a
/// derived classification (an agent is a member that has posted a status),
/// never a role, so the v2 vocabulary has no `agent` arm.
#[must_use]
pub fn role(iroh: IrohRole) -> Role {
    match iroh {
        IrohRole::Admin => Role::Authority,
        IrohRole::Member | IrohRole::Agent => Role::Member,
    }
}

/// Map an on-wire role string (`admin|member|agent`) to the v2 role.
/// `admin` is `authority`; everything else is `member` (including `agent`,
/// which is not a role in v2).
fn role_from_wire(wire: &str) -> Role {
    if wire == "admin" {
        Role::Authority
    } else {
        Role::Member
    }
}

/// Resolve the event author's attribution. A sender the membership fold can
/// resolve is `Resolved` with its role and standing at read time; a sender it
/// cannot resolve carries **no attribution** (`Unresolved`) — the record
/// removes v1's fabricated default role, so an unknown author is stated
/// honestly rather than invented as a `member`.
fn author(snapshot: &MembershipSnapshot, sender: &IdentityKey) -> Author {
    match snapshot.member(sender) {
        Some(member) => Author::Resolved {
            subject_id: SubjectId::new(sender.to_string()),
            role: role(member.role),
            // The fold's member row carries the standing; a member in the set
            // is active unless a departure event set it aside. The snapshot's
            // status refinement is applied by the caller for roster rows; for
            // authorship the fold's membership fact is what we serve.
            standing: Standing::Active,
        },
        None => Author::Unresolved,
    }
}

/// Map a closed agent-status label string to the closed v2 vocabulary. An
/// out-of-vocabulary label is `status_label_unknown`, never silently
/// reclassified — this returns `Err` so the caller refuses rather than
/// fabricates a known state.
fn status_label(label: &str) -> CoreResult<StatusLabel> {
    Ok(match label {
        "online" => StatusLabel::Online,
        "idle" => StatusLabel::Idle,
        "claiming" => StatusLabel::Claiming,
        "working" => StatusLabel::Working,
        "done" => StatusLabel::Done,
        "failed" => StatusLabel::Failed,
        "blocked" => StatusLabel::Blocked,
        other => {
            return Err(CoreError::invalid(format!(
                "status label {other:?} is outside the closed v2 vocabulary"
            )))
        }
    })
}

/// Map an on-wire progress percent to the v2 `progress` variant. Absent is
/// the no-progress arm; a reported percent is bounded to `0..=100` by the
/// content validator, so it always fits the `Reported` arm's `u8`.
fn progress(pct: Option<u64>) -> Progress {
    match pct {
        Some(p) => Progress::Reported {
            percent: u8::try_from(p.min(100)).unwrap_or(100),
        },
        None => Progress::Absent,
    }
}

/// Fold one stored event into its committed v2 [`Event`], or `None` for an
/// event kind the protocol does not commit to the displayed timeline. Pure:
/// no IO, no clock beyond the signed `created_at`.
///
/// `member.invited` and `member.removed` are not among the ten committed
/// kinds the record enumerates (`invite.mint` produces no timeline event;
/// `member.remove` produces `member_removed` — see below), so an event whose
/// kind has no committed `EventKind` is omitted rather than fabricated.
///
/// # v2 committed-kind mapping
/// The record's ten kinds are authored one-per-operation. The Iroh content
/// registry is the v1 vocabulary, so the fold maps content to the committed
/// kind the corresponding v2 operation authors:
/// - `room.created` → `room_created`
/// - `message.text` → `message`
/// - `agent.status` → `agent_status`
/// - `member.joined` → `member_joined`
/// - `member.left` → `member_left`
/// - `member.removed` → `member_removed`
/// - `file.shared` → `file_shared`
/// - `pipe.opened` → `pipe_published`
/// - `pipe.closed` → `pipe_revoked`
/// - `member.invited` → (no committed event; `invite.mint` authors none)
#[must_use]
pub fn materialize(se: &StoredEvent, snapshot: &MembershipSnapshot) -> Option<Event> {
    let pos = pos(se)?;
    let ev = SignedEvent::decode(&se.wire.signed).ok()?;
    materialize_signed(pos, &se.event_id, &ev, snapshot)
}

/// Fold one decoded signed event plus its position into a committed v2
/// [`Event`]. Returns `None` for a content kind with no committed `EventKind`
/// (`member.invited`), and for an `agent.status` whose label is outside the
/// closed vocabulary (refused, never reclassified).
#[must_use]
pub fn materialize_signed(
    pos: u64,
    event_id: &iroh_rooms::events::EventId,
    ev: &SignedEvent,
    snapshot: &MembershipSnapshot,
) -> Option<Event> {
    let kind = kind_content(&ev.content)?;
    Some(Event {
        pos,
        event_id: EventId::new(bare_event_hex(event_id)),
        at: ts(ev.created_at),
        author: author(snapshot, &ev.sender_id),
        kind,
    })
}

/// The `created_at` and committed [`EventKind`] of a stored event, for the
/// `room.list` `last_event` recency projection. The kind is `None` for an
/// event with no committed kind (`member.invited`) — the timestamp is still
/// real, so the row says *when* without inventing *what*.
#[must_use]
pub fn stored_event_recency(se: &StoredEvent) -> Option<(u64, Option<EventKind>)> {
    let ev = SignedEvent::decode(&se.wire.signed).ok()?;
    Some((ev.created_at, kind_content(&ev.content).map(|k| k.kind())))
}

/// Build the `last_event` variant for a room row: the newest committed
/// event's author-dated instant and kind, or `Absent` when the room has no
/// committed event.
#[must_use]
pub fn last_event(recency: Option<(u64, Option<EventKind>)>) -> LastEvent {
    match recency {
        Some((created_at_ms, Some(kind))) => LastEvent::Present {
            at: ts(created_at_ms),
            kind,
        },
        _ => LastEvent::Absent,
    }
}

/// The coupled kind-and-content for one Iroh content value, or `None` when
/// the content has no committed v2 kind.
fn kind_content(content: &Content) -> Option<EventKindContent> {
    Some(match content {
        Content::RoomCreated(c) => EventKindContent::RoomCreated {
            name: c.room_name.clone(),
        },
        Content::MessageText(c) => EventKindContent::Message {
            body: c.body.clone(),
        },
        Content::AgentStatus(c) => {
            // An out-of-vocabulary label must not become a fabricated known
            // state: the event is omitted from the committed timeline rather
            // than reclassified.
            let label = status_label(&c.status).ok()?;
            EventKindContent::AgentStatus {
                label,
                progress: progress(c.progress_pct),
            }
        }
        Content::MemberJoined(c) => EventKindContent::MemberJoined {
            subject_id: SubjectId::new(c.device_binding.identity_key.to_string()),
            role: role_from_wire(&c.role),
        },
        Content::MemberLeft(c) => EventKindContent::MemberLeft {
            subject_id: SubjectId::new(c.member_id.to_string()),
        },
        Content::MemberRemoved(c) => EventKindContent::MemberRemoved {
            subject_id: SubjectId::new(c.member_id.to_string()),
            by: SubjectId::new(c.removed_by.to_string()),
        },
        Content::FileShared(c) => EventKindContent::FileShared {
            file_id: FileId::new(file_handle(&c.file_id)),
            name: c.name.clone(),
            bytes: c.size_bytes,
            digest: c.blob_hash.to_string(),
        },
        Content::PipeOpened(c) => EventKindContent::PipePublished {
            pipe_id: PipeId::new(hex::encode(c.pipe_id)),
            target: pipe_target(&c.target_hint),
            audience: pipe_audience(&c.allowed_members),
        },
        Content::PipeClosed(c) => EventKindContent::PipeRevoked {
            pipe_id: PipeId::new(hex::encode(c.pipe_id)),
        },
        // `member.invited` authors no committed timeline event in v2.
        Content::MemberInvited(_) => return None,
        // `invite.revoke` authors `invite_revoked`; the Iroh registry has no
        // distinct content for it in this MVP (a revoked invite is a store
        // fact, not a signed event kind), so nothing folds to it here. The
        // match above is already exhaustive over the registry's committed
        // kinds, so there is no fallthrough arm.
    })
}

/// Parse a pipe `target_hint` (`"host:port"`) into the v2 `target` object.
/// The hint is authored as a loopback target; a malformed hint falls back to
/// a loopback placeholder rather than failing the whole event.
fn pipe_target(hint: &str) -> Target {
    match hint.rsplit_once(':') {
        Some((host, port)) => Target {
            host: host.to_string(),
            port: port.parse().unwrap_or(0),
        },
        None => Target {
            host: hint.to_string(),
            port: 0,
        },
    }
}

/// Map a pipe's allowed-member list to the v2 `audience`. A single allowed
/// identity is `Subjects`; the record requires the audience to be stated
/// explicitly, never defaulted.
fn pipe_audience(allowed: &[IdentityKey]) -> Audience {
    Audience::Subjects {
        subject_ids: allowed
            .iter()
            .map(|k| SubjectId::new(k.to_string()))
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Shared projection helpers used by the supervisor's typed reads
// ---------------------------------------------------------------------------

/// The v2 `standing` for a roster row. `removed` and `left` are distinct
/// signed facts; an active member is `active`.
#[must_use]
pub fn standing(removed: bool, left: bool) -> Standing {
    if removed {
        Standing::Removed
    } else if left {
        Standing::Left
    } else {
        Standing::Active
    }
}

/// Build a `Link` from an observed transport state. `direct`/`mixed` are a
/// direct path, `relay` a relayed one, and anything else is not connected.
/// `since` is the author-dated instant the link came up when known; the
/// transport layer does not always date link establishment, so an unknown
/// since uses the Unix epoch rather than the wall clock.
#[must_use]
pub fn link(path: Option<&str>, since_ms: Option<u64>) -> Link {
    let since = ts(since_ms.unwrap_or(0));
    match path {
        Some("direct") | Some("mixed") => Link::Direct { since },
        Some("relay") => Link::Relay { since },
        _ => Link::NotConnected {
            reason: LinkReason::NoRoute,
        },
    }
}

/// A `LastSeen` variant from an optional author-dated instant.
#[must_use]
pub fn last_seen(at_ms: Option<u64>) -> LastSeen {
    match at_ms {
        Some(ms) => LastSeen::Present { at: ts(ms) },
        None => LastSeen::Absent,
    }
}

/// A `LatestStatus` variant from an optional (label, instant) pair. An
/// out-of-vocabulary label is `Absent` rather than fabricated.
#[must_use]
pub fn latest_status(label: Option<(&str, u64)>) -> LatestStatus {
    match label {
        Some((l, ms)) => match status_label(l) {
            Ok(label) => LatestStatus::Present { label, at: ts(ms) },
            Err(_) => LatestStatus::Absent,
        },
        None => LatestStatus::Absent,
    }
}

/// Map the internal liveness classification to the v2 `liveness` vocabulary.
#[must_use]
pub fn liveness(live: crate::fleet::Liveness) -> Liveness {
    match live {
        crate::fleet::Liveness::OnlineIdle => Liveness::OnlineIdle,
        crate::fleet::Liveness::Working => Liveness::Working,
        crate::fleet::Liveness::Offline => Liveness::Offline,
        crate::fleet::Liveness::Stale => Liveness::Stale,
    }
}

/// The derived severity for a status label — served, never re-derived by a
/// client.
#[must_use]
pub fn severity(label: StatusLabel) -> Severity {
    label.severity()
}

/// A continuation cursor: `More` carrying the position to resume from when a
/// page is truncated, `Complete` when there is nothing further.
#[must_use]
pub fn truncated(next: Option<u64>) -> Truncated {
    match next {
        Some(pos) => Truncated::More {
            cursor: Cursor::At { pos },
        },
        None => Truncated::Complete,
    }
}

/// Wrap an Iroh room id in the opaque v2 `RoomId`.
#[must_use]
pub fn room_id(id: &iroh_rooms::room::RoomId) -> RoomId {
    RoomId::new(id.to_string())
}

/// Wrap a raw event id (bare hex) in the opaque v2 `EventId`.
#[must_use]
pub fn event_id(id: &iroh_rooms::events::EventId) -> EventId {
    EventId::new(bare_event_hex(id))
}

/// Wrap an Iroh identity key in the opaque v2 `SubjectId`.
#[must_use]
pub fn subject_id(key: &IdentityKey) -> SubjectId {
    SubjectId::new(key.to_string())
}

/// Wrap a device key string in the opaque v2 `DeviceId`.
#[must_use]
pub fn device_id(key: impl Into<String>) -> DeviceId {
    DeviceId::new(key)
}

/// Wrap a 16-byte invite short id in the opaque v2 `InviteId` (bare hex).
#[must_use]
pub fn invite_id(id: &[u8; 16]) -> InviteId {
    InviteId::new(hex::encode(id))
}

/// Wrap a 16-byte pipe short id in the opaque v2 `PipeId` (bare hex).
#[must_use]
pub fn pipe_id(id: &[u8; 16]) -> PipeId {
    PipeId::new(hex::encode(id))
}

/// Wrap a 16-byte file short id in the opaque v2 `FileId` (`file_<hex>`).
#[must_use]
pub fn file_id(id: &[u8; 16]) -> FileId {
    FileId::new(file_handle(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_rooms::events::{
        build_agent_status, build_message_text, validate_wire_bytes, SignedEvent,
        ValidationContext, WireEvent,
    };
    use iroh_rooms::experimental::store::EventStore;
    use iroh_rooms::files::{build_file_shared, HashRef};
    use iroh_rooms::identity::DeviceBinding;
    use iroh_rooms::identity::SigningKey;
    use iroh_rooms::room::{
        build_member_joined, build_member_left, build_room_created, derive_room_id,
        MembershipSnapshot, RoomId, RoomMembership,
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

    fn snapshot_of(fx: &Fixture) -> MembershipSnapshot {
        let ctx = ValidationContext::for_room(fx.room_id);
        let validated = validate_wire_bytes(&fx.genesis.to_bytes(), &ctx).expect("genesis valid");
        RoomMembership::from_events(fx.room_id, vec![validated]).snapshot()
    }

    fn decode(wire: &WireEvent) -> SignedEvent {
        SignedEvent::decode(&wire.signed).expect("authored event decodes")
    }

    fn mat(fx: &Fixture, wire: &WireEvent, pos: u64) -> Event {
        let snapshot = snapshot_of(fx);
        let ev = decode(wire);
        let ctx = ValidationContext::for_room(fx.room_id);
        let event_id = validate_wire_bytes(&wire.to_bytes(), &ctx)
            .map_or(iroh_rooms::events::EventId::from_bytes([0x0f; 32]), |v| {
                v.event_id
            });
        materialize_signed(pos, &event_id, &ev, &snapshot).expect("materializes")
    }

    #[test]
    fn room_created_has_authority_author_and_typed_content() {
        let fx = fixture();
        let e = mat(&fx, &fx.genesis, 0);
        assert_eq!(e.pos, 0);
        assert!(matches!(e.kind, EventKindContent::RoomCreated { .. }));
        assert_eq!(e.kind.kind(), EventKind::RoomCreated);
        match e.author {
            Author::Resolved { role, standing, .. } => {
                assert_eq!(role, Role::Authority);
                assert_eq!(standing, Standing::Active);
            }
            Author::Unresolved => panic!("genesis author must resolve"),
        }
        if let EventKindContent::RoomCreated { name } = e.kind {
            assert_eq!(name, "Build Iroh Rooms MVP");
        }
    }

    #[test]
    fn message_carries_typed_body() {
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
        let e = mat(&fx, &wire, 1);
        match e.kind {
            EventKindContent::Message { body } => assert_eq!(body, "hello"),
            other => panic!("wrong kind: {other:?}"),
        }
    }

    #[test]
    fn agent_status_maps_typed_label_and_progress() {
        let fx = fixture();
        let wire = build_agent_status(
            &fx.identity,
            &fx.device,
            &fx.room_id,
            "working",
            None,
            &[],
            Some(60),
            &[],
            TS + 1,
        );
        let e = mat(&fx, &wire, 1);
        match e.kind {
            EventKindContent::AgentStatus { label, progress } => {
                assert_eq!(label, StatusLabel::Working);
                assert_eq!(progress, Progress::Reported { percent: 60 });
                assert_eq!(label.severity(), Severity::Ok);
            }
            other => panic!("wrong kind: {other:?}"),
        }
    }

    #[test]
    fn out_of_vocabulary_status_label_is_omitted_not_reclassified() {
        let fx = fixture();
        let wire = build_agent_status(
            &fx.identity,
            &fx.device,
            &fx.room_id,
            "running_tests", // not in the closed v2 vocabulary
            None,
            &[],
            None,
            &[],
            TS + 1,
        );
        let snapshot = snapshot_of(&fx);
        let ev = decode(&wire);
        let id = iroh_rooms::events::EventId::from_bytes([0x01; 32]);
        assert!(
            materialize_signed(1, &id, &ev, &snapshot).is_none(),
            "an unrecognized label must not become a fabricated known state"
        );
    }

    #[test]
    fn member_joined_and_left_map_subject_ids() {
        let fx = fixture();
        let joiner_identity = SigningKey::generate();
        let joiner_device = SigningKey::generate();
        let binding =
            DeviceBinding::create(&fx.room_id, &joiner_identity, joiner_device.device_key());
        let joined = build_member_joined(
            &joiner_identity,
            &joiner_device,
            &fx.room_id,
            &[0x01; 16],
            &[0x03; 16],
            "member",
            binding,
            None,
            &[],
            TS + 2,
        );
        let e = mat(&fx, &joined, 1);
        match e.kind {
            EventKindContent::MemberJoined { subject_id, role } => {
                assert_eq!(
                    subject_id.as_str(),
                    joiner_identity.identity_key().to_string()
                );
                assert_eq!(role, Role::Member);
            }
            other => panic!("wrong kind: {other:?}"),
        }

        let left = build_member_left(&fx.identity, &fx.device, &fx.room_id, None, &[], TS + 3);
        let e = mat(&fx, &left, 2);
        match e.kind {
            EventKindContent::MemberLeft { subject_id } => {
                assert_eq!(subject_id.as_str(), fx.identity.identity_key().to_string());
            }
            other => panic!("wrong kind: {other:?}"),
        }
    }

    #[test]
    fn file_shared_maps_typed_file_fields() {
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
        let e = mat(&fx, &wire, 1);
        match e.kind {
            EventKindContent::FileShared {
                file_id,
                name,
                bytes,
                digest,
            } => {
                assert!(file_id.as_str().starts_with("file_"));
                assert_eq!(name, "PRD.pdf");
                assert_eq!(bytes, 123);
                assert!(!digest.is_empty());
            }
            other => panic!("wrong kind: {other:?}"),
        }
    }

    #[test]
    fn store_roundtrip_materializes_committed_events() {
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
        let events: Vec<Event> = rows
            .iter()
            .filter_map(|se| materialize(se, &snapshot))
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind.kind(), EventKind::RoomCreated);
        assert_eq!(events[1].kind.kind(), EventKind::Message);
        // Positions come from the store's Lamport clock: genesis is 0.
        assert_eq!(events[0].pos, 0);
        assert!(events[1].pos > events[0].pos);
    }

    #[test]
    fn unresolved_author_carries_no_attribution() {
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
        let e = materialize_signed(1, &id, &ev, &snapshot).unwrap();
        assert_eq!(e.author, Author::Unresolved);
    }
}
