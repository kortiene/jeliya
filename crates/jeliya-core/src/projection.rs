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
    Audience, Author, Cursor, Event, EventId, EventKind, EventKindContent, FileId, InviteId,
    LastEvent, LastSeen, LatestStatus, Liveness, PipeId, Progress, Role, RoomId, Standing,
    StatusLabel, SubjectId, Target, Timestamp, Truncated,
};
use time::OffsetDateTime;

use iroh_rooms::events::{Content, SignedEvent};
use iroh_rooms::experimental::store::StoredEvent;
use iroh_rooms::identity::IdentityKey;
use iroh_rooms::room::{MembershipSnapshot, Role as IrohRole};

use crate::error::{CoreError, CoreResult};

/// The bare 64-hex form of an event id. Protocol v2 strips the SDK's optional
/// `blake3:` display prefix.
pub(crate) fn bare_event_hex(event_id: &iroh_rooms::events::EventId) -> String {
    let displayed = event_id.to_string();
    displayed
        .strip_prefix("blake3:")
        .unwrap_or(&displayed)
        .to_owned()
}

/// The protocol file handle for a 16-byte on-wire short id.
pub(crate) fn file_handle(file_id: &[u8; 16]) -> String {
    format!("file_{}", hex::encode(file_id))
}

/// Convert a signed author-dated `created_at` (ms since the Unix epoch, the
/// non-repudiable author clock) into the wire `<ts>` (RFC 3339, `Z` offset).
///
/// The signed timestamp is the only clock a projection may serve; the wall
/// clock is never read here. Milliseconds are truncated to whole seconds
/// because RFC 3339 has no fractional requirement and the `<ts>` grammar is
/// second-precision.
fn ts(created_at_ms: u64) -> Option<Timestamp> {
    let secs = i64::try_from(created_at_ms / 1000).ok()?;
    OffsetDateTime::from_unix_timestamp(secs)
        .ok()
        .map(Timestamp::new)
}

/// The v2 per-room position space is the room's committed timeline events
/// **ranked densely in canonical `(lamport, event_id)` order**: the genesis
/// is `0` and every later committed event is exactly one past its
/// predecessor. Two rules make that hold:
///
/// - **Filter to committed events FIRST.** Only kinds the record commits to
///   the timeline hold a position. A stored row [`materialize`] drops (a
///   `member.invited`, an out-of-vocabulary `agent.status`) is not a
///   committed event and consumes no position — otherwise the next rendered
///   event would jump a position and leave a hole resync cannot fill.
/// - **Rank densely over the survivors.** The store's raw Lamport value is
///   NOT a position — concurrent siblings share a lamport, and the record
///   requires strictly-increasing, gap-free positions, which only the dense
///   rank over the canonical order provides.
///
/// [`positioned`] applies both: it filters a canonical tail to committed
/// events and assigns each its dense rank. Ranking happens at the read
/// boundary (`TypedSupervisor::committed_events` for timeline/resync/archive,
/// and the supervisor's typed push paths for `Push::Event`), so one
/// consistent rank serves every position the protocol exposes.
///
/// A causally-incomplete row (a missing parent leaves `lamport` unset) is
/// excluded from the canonical order, hence holds no position.
///
/// # Stability across convergence
///
/// The canonical order is **not stable across convergence**: a late-arriving
/// concurrent event can interleave below the frontier and shift the ranks of
/// already-served events after it. Positions are therefore a per-room,
/// eventually-consistent sequence the client treats as authoritative only at
/// read time. The push paths are reorder-aware (see
/// [`supervisor::collect_committed`](crate::supervisor)): when a new event's
/// rank is at or below an already-served rank, the stream emits an explicit
/// `gap` from the first shifted position so the client discards and resyncs
/// rather than trusting a silently renumbered suffix.
pub(crate) fn positioned(
    rows: &[&StoredEvent],
    snapshot: &MembershipSnapshot,
) -> Vec<(u64, Event)> {
    let mut out = Vec::new();
    let mut rank = 0u64;
    for se in rows {
        if let Some(event) = materialize(se, 0, snapshot) {
            out.push((rank, Event { pos: rank, ..event }));
            rank += 1;
        }
    }
    out
}

/// Whether a stored event is a committed timeline event (its kind is one the
/// record commits and, for `agent.status`, its label is in the closed
/// vocabulary). Snapshot-free: the committed check depends only on the signed
/// content, not on membership. [`positioned`]'s callers and the rank lookups
/// use this to keep non-committed rows from consuming a position.
pub(crate) fn is_committed(se: &StoredEvent) -> bool {
    let Ok(ev) = SignedEvent::decode(&se.wire.signed) else {
        return false;
    };
    ts(ev.created_at).is_some() && kind_content(&ev.content).is_some()
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
fn role_from_wire(wire: &str) -> Option<Role> {
    match wire {
        "admin" => Some(Role::Authority),
        "member" | "agent" => Some(Role::Member),
        _ => None,
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

/// Fold one stored event, at its dense canonical rank `pos` (see
/// [`positioned`]), into its committed v2 [`Event`], or `None` for an event
/// kind the protocol does not commit to the displayed timeline. Pure: no IO,
/// no clock beyond the signed `created_at`.
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
pub fn materialize(se: &StoredEvent, pos: u64, snapshot: &MembershipSnapshot) -> Option<Event> {
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
        at: ts(ev.created_at)?,
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
        Some((created_at_ms, Some(kind))) => {
            ts(created_at_ms).map_or(LastEvent::Absent, |at| LastEvent::Present { at, kind })
        }
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
            role: role_from_wire(&c.role)?,
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
            target: pipe_target(&c.target_hint)?,
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
/// The hint is authored as a loopback target. A malformed or non-loopback
/// target is omitted rather than projected as a fabricated endpoint.
fn pipe_target(hint: &str) -> Option<Target> {
    let addr: std::net::SocketAddr = hint.parse().ok()?;
    if !addr.ip().is_loopback() || addr.port() == 0 {
        return None;
    }
    Some(Target {
        host: addr.ip().to_string(),
        port: u64::from(addr.port()),
    })
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

/// A `LastSeen` variant from an optional author-dated instant.
#[must_use]
pub fn last_seen(at_ms: Option<u64>) -> LastSeen {
    match at_ms.and_then(ts) {
        Some(at) => LastSeen::Present { at },
        None => LastSeen::Absent,
    }
}

/// A `LatestStatus` variant from an optional (label, instant) pair. An
/// out-of-vocabulary label is `Absent` rather than fabricated.
#[must_use]
pub fn latest_status(label: Option<(&str, u64)>) -> LatestStatus {
    match label {
        Some((l, ms)) => match (status_label(l), ts(ms)) {
            (Ok(label), Some(at)) => LatestStatus::Present { label, at },
            _ => LatestStatus::Absent,
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
    use iroh_rooms::room::build_member_invited;
    use iroh_rooms::room::{
        build_member_joined, build_member_left, build_room_created, derive_room_id,
        MembershipSnapshot, RoomId, RoomMembership,
    };
    use jeliya_api::Severity;

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
        let refs: Vec<&StoredEvent> = rows.iter().collect();
        let events: Vec<Event> = positioned(&refs, &snapshot)
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind.kind(), EventKind::RoomCreated);
        assert_eq!(events[1].kind.kind(), EventKind::Message);
        // Positions are the dense rank over the canonical order: genesis is
        // 0 and each later committed event is exactly one past its
        // predecessor.
        assert_eq!(events[0].pos, 0);
        assert_eq!(events[1].pos, 1);
    }

    #[test]
    fn concurrent_siblings_get_dense_gap_free_positions() {
        // Two messages authored against the SAME parent are concurrent
        // siblings: the store's derived Lamport clock assigns them the same
        // value. The v2 position space must still be dense, strictly
        // increasing, and gap-free, so positions are the rank over the
        // canonical `(lamport, event_id)` order — never the raw lamport.
        let fx = fixture();
        let mut store = EventStore::open_in_memory().unwrap();
        let ctx = ValidationContext::for_room(fx.room_id);
        let genesis = validate_wire_bytes(&fx.genesis.to_bytes(), &ctx).unwrap();
        let genesis_id = genesis.event_id;
        store.insert(&genesis).unwrap();

        // Two distinct authors (two devices), both children of the genesis.
        let device_b = SigningKey::generate();
        let a = validate_wire_bytes(
            &build_message_text(
                &fx.identity,
                &fx.device,
                &fx.room_id,
                "sibling a",
                None,
                None,
                &[],
                &[genesis_id],
                TS + 1,
            )
            .to_bytes(),
            &ctx,
        )
        .unwrap();
        let b = validate_wire_bytes(
            &build_message_text(
                &fx.identity,
                &device_b,
                &fx.room_id,
                "sibling b",
                None,
                None,
                &[],
                &[genesis_id],
                TS + 1,
            )
            .to_bytes(),
            &ctx,
        )
        .unwrap();
        store.insert(&a).unwrap();
        store.insert(&b).unwrap();

        let rows = store.room_tail(&fx.room_id, 100).unwrap();
        assert_eq!(rows.len(), 3);
        // The two siblings share one lamport: the raw store value is NOT
        // unique, which is exactly why it cannot be the served position.
        assert_eq!(rows[1].lamport, rows[2].lamport);

        let snapshot = snapshot_of(&fx);
        let refs: Vec<&StoredEvent> = rows.iter().collect();
        let events: Vec<Event> = positioned(&refs, &snapshot)
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        let positions: Vec<u64> = events.iter().map(|e| e.pos).collect();
        assert_eq!(positions, [0, 1, 2], "dense and gap-free across siblings");
    }

    #[test]
    fn non_committed_events_consume_no_position() {
        // A `member.invited` row is stored but NOT committed to the timeline
        // (`invite.mint` authors no committed event). It must consume no
        // position: the message after it sits immediately after the genesis,
        // not a rank higher, so the served space stays dense and gap-free.
        let fx = fixture();
        let mut store = EventStore::open_in_memory().unwrap();
        let ctx = ValidationContext::for_room(fx.room_id);
        let genesis = validate_wire_bytes(&fx.genesis.to_bytes(), &ctx).unwrap();
        let genesis_id = genesis.event_id;
        store.insert(&genesis).unwrap();

        let invitee = SigningKey::generate();
        let invited = validate_wire_bytes(
            &build_member_invited(
                &fx.identity,
                &fx.device,
                &fx.room_id,
                &[0x07; 16],
                &[0x09; 32],
                "member",
                &invitee.identity_key(),
                None,
                None,
                &[genesis_id],
                TS + 1,
            )
            .to_bytes(),
            &ctx,
        )
        .unwrap();
        store.insert(&invited).unwrap();
        let msg = validate_wire_bytes(
            &build_message_text(
                &fx.identity,
                &fx.device,
                &fx.room_id,
                "after the invite",
                None,
                None,
                &[],
                &[genesis_id],
                TS + 2,
            )
            .to_bytes(),
            &ctx,
        )
        .unwrap();
        store.insert(&msg).unwrap();

        let snapshot = snapshot_of(&fx);
        let rows = store.room_tail(&fx.room_id, 100).unwrap();
        assert_eq!(
            rows.len(),
            3,
            "invite row is stored even though not committed"
        );
        let refs: Vec<&StoredEvent> = rows.iter().collect();
        let events: Vec<Event> = positioned(&refs, &snapshot)
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        // The invite consumed no position: genesis 0, message 1, no hole.
        assert_eq!(events.len(), 2, "the invite is not a committed event");
        assert_eq!(events[0].kind.kind(), EventKind::RoomCreated);
        assert_eq!(events[0].pos, 0);
        assert_eq!(events[1].kind.kind(), EventKind::Message);
        assert_eq!(events[1].pos, 1, "no position hole where the invite sat");
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
