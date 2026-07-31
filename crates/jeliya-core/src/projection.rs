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

use std::collections::BTreeSet;

use jeliya_api::{
    Audience, Author, Cursor, Event, EventId, EventKind, EventKindContent, FileId, InviteId,
    LastEvent, LastSeen, LatestStatus, Liveness, PipeId, Progress, Role, RoomId, Standing,
    StatusLabel, SubjectId, Target, Timestamp, Truncated,
};
use time::OffsetDateTime;

use iroh_rooms::events::{Content, SignedEvent};
use iroh_rooms::experimental::store::StoredEvent;
use iroh_rooms::identity::IdentityKey;
use iroh_rooms::room::{MembershipSnapshot, Role as IrohRole, Status as IrohStatus};

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
/// non-repudiable author clock) into the wire `<ts>` (RFC 3339, `Z` offset),
/// or `None` when the instant is not representable on the wire.
///
/// The signed timestamp is the only clock a projection may serve; the wall
/// clock is never read here. Milliseconds are truncated to whole seconds
/// because RFC 3339 has no fractional requirement and the `<ts>` grammar is
/// second-precision.
///
/// **There is no epoch fallback.** An unrepresentable instant is a missing
/// fact, and the record forbids turning a missing fact into a known one: an
/// event carrying one is not a committed event at all (see [`is_committed`]),
/// and a projection that needs one and cannot have it answers its exact typed
/// error rather than serving `1970-01-01T00:00:00Z` as though it were signed.
pub(crate) fn ts(created_at_ms: u64) -> Option<Timestamp> {
    let secs = i64::try_from(created_at_ms / 1000).ok()?;
    OffsetDateTime::from_unix_timestamp(secs)
        .ok()
        .map(Timestamp::new)
}

/// [`ts`] retaining the sub-second part, for a `<ts>` the **caller supplied**
/// rather than an author dated.
///
/// An invite's `expires_at` is chosen by the client, signed verbatim, and then
/// echoed by `invite.mint` and served again by `invite.list`. Truncating it to
/// whole seconds would make the daemon answer with an instant the caller did
/// not name — and a faithful `op_id` retry that resent the reply's value would
/// then carry a different body than the original request. RFC 3339 permits the
/// fractional part, so the value round-trips exactly.
pub(crate) fn ts_millis(at_ms: u64) -> Option<Timestamp> {
    let nanos = i128::from(at_ms).checked_mul(1_000_000)?;
    OffsetDateTime::from_unix_timestamp_nanos(nanos)
        .ok()
        .map(Timestamp::new)
}

/// The signed departure facts a room's history carries.
///
/// The upstream fold collapses a voluntary leave and an administrative removal
/// into one `Status::Removed`; protocol v2 keeps `left` and `removed` distinct,
/// so the two sets are folded from the signed `member.left` / `member.removed`
/// content and consulted wherever a `standing` is served. Carrying them as one
/// value is what lets **every** standing answer — the caller's own, a roster
/// row's, and a committed event's `author.standing` — come from one derivation
/// instead of three, one of which used to be the constant `active`.
#[derive(Debug, Default, Clone)]
pub(crate) struct Departures {
    /// Subjects an authority removed.
    pub removed: BTreeSet<IdentityKey>,
    /// Subjects that left voluntarily.
    pub left: BTreeSet<IdentityKey>,
}

impl Departures {
    /// Fold the departure facts out of a room's canonical tail. Pure: it reads
    /// only signed content, so the push paths (which already hold the tail) can
    /// build it without a second store handle.
    pub(crate) fn from_rows<'a>(rows: impl IntoIterator<Item = &'a StoredEvent>) -> Self {
        let mut out = Self::default();
        for stored in rows {
            let Ok(event) = SignedEvent::decode(&stored.wire.signed) else {
                continue;
            };
            match event.content {
                // A re-join supersedes both prior terminal facts.
                Content::MemberJoined(_) => {
                    out.removed.remove(&event.sender_id);
                    out.left.remove(&event.sender_id);
                }
                Content::MemberRemoved(c) => {
                    out.left.remove(&c.member_id);
                    out.removed.insert(c.member_id);
                }
                Content::MemberLeft(c) => {
                    out.removed.remove(&c.member_id);
                    out.left.insert(c.member_id);
                }
                _ => {}
            }
        }
        out
    }

    /// The v2 `standing` for one subject, refining the fold's terminal status
    /// with the signed fact that caused it. An administrative removal dominates
    /// a concurrent self-leave.
    #[must_use]
    pub(crate) fn standing_of(&self, status: IrohStatus, identity: &IdentityKey) -> Standing {
        let left = self.left.contains(identity);
        let removed =
            self.removed.contains(identity) || (!left && matches!(status, IrohStatus::Removed));
        standing(removed, left)
    }
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
    departures: &Departures,
) -> Vec<(u64, Event)> {
    let mut out = Vec::new();
    let mut rank = 0u64;
    for se in rows {
        if let Some(event) = materialize(se, 0, snapshot, departures) {
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
///
/// `standing` is the **same derivation `room.members` serves**, folded from the
/// signed departure facts. An earlier revision hardcoded `active` here, which
/// told a client that a removed member's messages were authored by someone who
/// still belongs — attribution a UI uses to decide how much to trust something,
/// invented. One derivation, both places.
fn author(snapshot: &MembershipSnapshot, departures: &Departures, sender: &IdentityKey) -> Author {
    match snapshot.member(sender) {
        Some(member) => Author::Resolved {
            subject_id: SubjectId::new(sender.to_string()),
            role: role(member.role),
            standing: departures.standing_of(member.status, &member.identity),
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

/// Map an on-wire progress percent to the v2 `progress` variant. Absent is the
/// no-progress arm; a reported percent must be in the record's inclusive
/// `0..=100`.
///
/// A percent outside that range is **refused, never clamped**: saturating an
/// out-of-range value to `100` would report a task as complete on the strength
/// of a number the author never signed. The row carrying it is therefore not a
/// committed event (see [`kind_content`]), the same answer an out-of-vocabulary
/// label gets.
pub(crate) fn progress(pct: Option<u64>) -> Option<Progress> {
    match pct {
        Some(p) => u8::try_from(p)
            .ok()
            .filter(|percent| *percent <= 100)
            .map(|percent| Progress::Reported { percent }),
        None => Some(Progress::Absent),
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
pub(crate) fn materialize(
    se: &StoredEvent,
    pos: u64,
    snapshot: &MembershipSnapshot,
    departures: &Departures,
) -> Option<Event> {
    let ev = SignedEvent::decode(&se.wire.signed).ok()?;
    materialize_signed(pos, &se.event_id, &ev, snapshot, departures)
}

/// Fold one decoded signed event plus its position into a committed v2
/// [`Event`]. Returns `None` for a content kind with no committed `EventKind`
/// (`member.invited`), and for an `agent.status` whose label is outside the
/// closed vocabulary (refused, never reclassified).
#[must_use]
pub(crate) fn materialize_signed(
    pos: u64,
    event_id: &iroh_rooms::events::EventId,
    ev: &SignedEvent,
    snapshot: &MembershipSnapshot,
    departures: &Departures,
) -> Option<Event> {
    let kind = kind_content(&ev.content)?;
    Some(Event {
        pos,
        event_id: EventId::new(bare_event_hex(event_id)),
        at: ts(ev.created_at)?,
        author: author(snapshot, departures, &ev.sender_id),
        kind,
    })
}

/// One committed event's recency evidence, for the `room.list` `last_event`
/// projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Recency {
    /// The signed `created_at` in **milliseconds**. Ordering is compared on
    /// this, not on [`Self::at`]: the wire `<ts>` is second-precision, so
    /// comparing the converted value would tie every pair of events authored
    /// within the same second and let the id tiebreak pick the older one.
    pub created_at_ms: u64,
    /// The same instant in its wire form.
    pub at: Timestamp,
    /// The committed kind.
    pub kind: EventKind,
}

/// The recency evidence of a stored event — `None` for a row that is not a
/// committed event at all (`member.invited`, an out-of-vocabulary status, an
/// out-of-range progress) or whose instant is not representable on the wire.
///
/// It returns evidence only when **every** part is real. An earlier revision
/// returned `(ts, None)` for a non-committed row, and because the caller took
/// the max over *every* row and then dropped a kindless winner, a room whose
/// newest row was an invitation reported `last_event: absent` while holding a
/// timeline full of messages. The recency projection is defined over committed
/// events, so a non-committed row must not win the max in the first place.
#[must_use]
pub(crate) fn stored_event_recency(se: &StoredEvent) -> Option<Recency> {
    let ev = SignedEvent::decode(&se.wire.signed).ok()?;
    Some(Recency {
        created_at_ms: ev.created_at,
        at: ts(ev.created_at)?,
        kind: kind_content(&ev.content)?.kind(),
    })
}

/// Build the `last_event` variant for a room row: the newest committed event's
/// author-dated instant and kind, or `Absent` when the room has no committed
/// event. `Absent` means exactly "no committed event", never "we had one and
/// could not describe it".
#[must_use]
pub(crate) fn last_event(recency: Option<Recency>) -> LastEvent {
    match recency {
        Some(Recency { at, kind, .. }) => LastEvent::Present { at, kind },
        None => LastEvent::Absent,
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
            // An out-of-vocabulary label — or a progress percent outside the
            // record's inclusive `0..=100` — must not become a fabricated known
            // state: the event is omitted from the committed timeline rather
            // than reclassified or clamped.
            let label = status_label(&c.status).ok()?;
            EventKindContent::AgentStatus {
                label,
                progress: progress(c.progress_pct)?,
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
///
/// The target is projected **verbatim**, including a non-loopback host. The
/// loopback policy is a *publish-side* refusal this daemon applies before
/// authoring (`pipe_target_refused`); a target another peer signed is a fact
/// about that peer's pipe, and suppressing it would hide from the operator
/// exactly the pipe worth seeing. Only a hint that is not an address at all
/// leaves the event unrepresentable, and such an event holds no position.
fn pipe_target(hint: &str) -> Option<Target> {
    let addr: std::net::SocketAddr = hint.parse().ok()?;
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
    use iroh_rooms::pipes::{build_pipe_closed, build_pipe_opened};
    use iroh_rooms::room::build_member_invited;
    use iroh_rooms::room::{
        build_member_joined, build_member_left, build_member_removed, build_room_created,
        derive_room_id, MembershipSnapshot, RoomId, RoomMembership,
    };
    use jeliya_api::Severity;
    use serde_json::{json, Value};

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
        mat_with(fx, wire, pos, &Departures::default())
    }

    fn mat_with(fx: &Fixture, wire: &WireEvent, pos: u64, departures: &Departures) -> Event {
        let snapshot = snapshot_of(fx);
        let ev = decode(wire);
        let ctx = ValidationContext::for_room(fx.room_id);
        let event_id = validate_wire_bytes(&wire.to_bytes(), &ctx)
            .map_or(iroh_rooms::events::EventId::from_bytes([0x0f; 32]), |v| {
                v.event_id
            });
        materialize_signed(pos, &event_id, &ev, &snapshot, departures).expect("materializes")
    }

    // ------------------------------------------------------------------
    // Every currently representable committed event, as a table
    // ------------------------------------------------------------------

    /// One authored event per committed kind this build can represent, with
    /// the **encoded** `{kind, content}` the record fixes for it.
    ///
    /// The record closes `kind` at ten. Nine are exercised here as real signed
    /// events folded through [`materialize_signed`] and compared to a
    /// hand-written expectation transcribed from `docs/protocol-v2.md`'s
    /// committed-event table — not from this module's output.
    ///
    /// **`invite_revoked` is the tenth and is absent, irreducibly.** The active
    /// Iroh Room SDK has no convergent signed invite-revocation content, so
    /// there is no event to author and none to project; a locally synthesized
    /// tombstone would be a "committed" fact no peer converges on.
    /// [`no_kind_is_silently_uncovered`] asserts that gap explicitly rather
    /// than letting nine-of-ten read as complete coverage.
    fn committed_event_table(fx: &Fixture) -> Vec<(EventKind, WireEvent, Value)> {
        let joiner_identity = SigningKey::generate();
        let joiner_device = SigningKey::generate();
        let joiner = joiner_identity.identity_key();
        let binding =
            DeviceBinding::create(&fx.room_id, &joiner_identity, joiner_device.device_key());
        let removed_subject = SigningKey::generate().identity_key();
        let peer = SigningKey::generate().identity_key();

        vec![
            (
                EventKind::RoomCreated,
                fx.genesis.clone(),
                json!({ "kind": "room_created", "content": { "name": "Build Iroh Rooms MVP" } }),
            ),
            (
                EventKind::Message,
                build_message_text(
                    &fx.identity,
                    &fx.device,
                    &fx.room_id,
                    "hello",
                    None,
                    None,
                    &[],
                    &[],
                    TS + 1,
                ),
                json!({ "kind": "message", "content": { "body": "hello" } }),
            ),
            (
                EventKind::AgentStatus,
                build_agent_status(
                    &fx.identity,
                    &fx.device,
                    &fx.room_id,
                    "working",
                    None,
                    &[],
                    Some(60),
                    &[],
                    TS + 1,
                ),
                json!({
                    "kind": "agent_status",
                    "content": {
                        "label": "working",
                        "progress": { "state": "reported", "percent": 60 },
                    },
                }),
            ),
            (
                EventKind::MemberJoined,
                build_member_joined(
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
                ),
                json!({
                    "kind": "member_joined",
                    "content": { "subject_id": joiner.to_string(), "role": "member" },
                }),
            ),
            (
                EventKind::MemberLeft,
                build_member_left(&fx.identity, &fx.device, &fx.room_id, None, &[], TS + 3),
                json!({
                    "kind": "member_left",
                    "content": { "subject_id": fx.identity.identity_key().to_string() },
                }),
            ),
            (
                EventKind::MemberRemoved,
                build_member_removed(
                    &fx.identity,
                    &fx.device,
                    &fx.room_id,
                    &removed_subject,
                    None,
                    None,
                    &[],
                    TS + 4,
                ),
                json!({
                    "kind": "member_removed",
                    "content": {
                        "subject_id": removed_subject.to_string(),
                        "by": fx.identity.identity_key().to_string(),
                    },
                }),
            ),
            (
                EventKind::FileShared,
                build_file_shared(
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
                    TS + 5,
                ),
                json!({
                    "kind": "file_shared",
                    "content": {
                        "file_id": format!("file_{}", "11".repeat(16)),
                        "name": "PRD.pdf",
                        "bytes": 123,
                        "digest": HashRef::from_bytes([0xcc; 32]).to_string(),
                    },
                }),
            ),
            (
                EventKind::PipePublished,
                build_pipe_opened(
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
                    TS + 6,
                ),
                json!({
                    "kind": "pipe_published",
                    "content": {
                        "pipe_id": "ef".repeat(16),
                        "target": { "host": "127.0.0.1", "port": 3000 },
                        "audience": { "state": "subjects", "subject_ids": [peer.to_string()] },
                    },
                }),
            ),
            (
                EventKind::PipeRevoked,
                build_pipe_closed(
                    &fx.identity,
                    &fx.device,
                    &fx.room_id,
                    [0xef; 16],
                    Some("closed"),
                    &[],
                    TS + 7,
                ),
                json!({
                    "kind": "pipe_revoked",
                    "content": { "pipe_id": "ef".repeat(16) },
                }),
            ),
        ]
    }

    /// Every representable committed kind encodes to exactly the `{kind,
    /// content}` the record fixes for it — same discriminant spelling, same
    /// field set, no extra keys, and no JSON `null` anywhere.
    #[test]
    fn every_representable_event_encodes_to_its_record_shape() {
        let fx = fixture();
        for (kind, wire, expected) in committed_event_table(&fx) {
            let event = mat(&fx, &wire, 7);
            assert_eq!(event.kind.kind(), kind, "kind discriminant");
            let encoded = serde_json::to_value(&event).expect("an event serializes");

            // The common header the record fixes on every committed event.
            let obj = encoded.as_object().expect("an event is an object");
            let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                ["at", "author", "content", "event_id", "kind", "pos"],
                "{kind:?} carries exactly the record's event fields"
            );
            assert_eq!(
                obj["pos"],
                json!(7),
                "{kind:?} serves the rank it was given"
            );

            // The kind-and-content pair, transcribed from the record.
            let expected = expected.as_object().expect("expectation is an object");
            assert_eq!(obj["kind"], expected["kind"], "{kind:?} wire spelling");
            assert_eq!(obj["content"], expected["content"], "{kind:?} content");

            assert_no_nulls(&encoded, &format!("{kind:?}"));
        }
    }

    /// The nine kinds above are the nine this build can represent, and
    /// `invite_revoked` is the tenth. The gap is asserted so nine-of-ten can
    /// never read as coverage.
    #[test]
    fn no_kind_is_silently_uncovered() {
        let fx = fixture();
        let covered: std::collections::HashSet<EventKind> = committed_event_table(&fx)
            .into_iter()
            .map(|(kind, _, _)| kind)
            .collect();
        // The record's ten, in its own order.
        let all = [
            EventKind::RoomCreated,
            EventKind::Message,
            EventKind::AgentStatus,
            EventKind::MemberJoined,
            EventKind::MemberLeft,
            EventKind::MemberRemoved,
            EventKind::InviteRevoked,
            EventKind::FileShared,
            EventKind::PipePublished,
            EventKind::PipeRevoked,
        ];
        assert_eq!(all.len(), 10, "the record closes `kind` at ten");
        let uncovered: Vec<EventKind> = all
            .into_iter()
            .filter(|kind| !covered.contains(kind))
            .collect();
        assert_eq!(
            uncovered,
            vec![EventKind::InviteRevoked],
            "exactly one kind is unrepresentable, and it is the upstream-blocked one"
        );
    }

    fn assert_no_nulls(value: &Value, label: &str) {
        match value {
            Value::Null => panic!("{label} encodes a JSON null"),
            Value::Object(map) => map.values().for_each(|v| assert_no_nulls(v, label)),
            Value::Array(items) => items.iter().for_each(|v| assert_no_nulls(v, label)),
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Attribution
    // ------------------------------------------------------------------

    #[test]
    fn room_created_has_authority_author_and_typed_content() {
        let fx = fixture();
        let e = mat(&fx, &fx.genesis, 0);
        assert_eq!(e.pos, 0);
        assert_eq!(e.kind.kind(), EventKind::RoomCreated);
        match e.author {
            Author::Resolved { role, standing, .. } => {
                assert_eq!(role, Role::Authority);
                assert_eq!(standing, Standing::Active);
            }
            Author::Unresolved => panic!("genesis author must resolve"),
        }
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
        let e = mat(&fx, &wire, 1);
        assert_eq!(e.author, Author::Unresolved);
    }

    /// **The author's `standing` is derived, not assumed.** An earlier
    /// revision hardcoded `active` on every resolved author, which told a
    /// client that a departed member's messages were authored by someone who
    /// still belongs. Attribution a UI uses to decide how much to trust
    /// something must not be invented.
    #[test]
    fn a_resolved_authors_standing_is_the_signed_departure_fact() {
        let fx = fixture();
        let author_key = fx.identity.identity_key();
        let wire = build_message_text(
            &fx.identity,
            &fx.device,
            &fx.room_id,
            "before leaving",
            None,
            None,
            &[],
            &[],
            TS + 1,
        );

        for (departures, expected) in [
            (Departures::default(), Standing::Active),
            (
                Departures {
                    left: BTreeSet::from([author_key]),
                    ..Departures::default()
                },
                Standing::Left,
            ),
            (
                Departures {
                    removed: BTreeSet::from([author_key]),
                    ..Departures::default()
                },
                Standing::Removed,
            ),
            (
                // A removal dominates a concurrent self-leave.
                Departures {
                    removed: BTreeSet::from([author_key]),
                    left: BTreeSet::from([author_key]),
                },
                Standing::Removed,
            ),
        ] {
            let e = mat_with(&fx, &wire, 1, &departures);
            let Author::Resolved { standing, .. } = e.author else {
                panic!("the author resolves");
            };
            assert_eq!(standing, expected, "departures {departures:?}");
        }
    }

    /// `Departures` folds `member.left` / `member.removed` out of a real tail,
    /// and a re-join supersedes both.
    #[test]
    fn departures_fold_from_the_canonical_tail() {
        let fx = fixture();
        let mut store = EventStore::open_in_memory().unwrap();
        let ctx = ValidationContext::for_room(fx.room_id);
        let genesis = validate_wire_bytes(&fx.genesis.to_bytes(), &ctx).unwrap();
        store.insert(&genesis).unwrap();
        let left = validate_wire_bytes(
            &build_member_left(
                &fx.identity,
                &fx.device,
                &fx.room_id,
                None,
                &[genesis.event_id],
                TS + 1,
            )
            .to_bytes(),
            &ctx,
        )
        .unwrap();
        store.insert(&left).unwrap();

        let rows = store.room_tail(&fx.room_id, 100).unwrap();
        let departures = Departures::from_rows(rows.iter());
        assert!(departures.left.contains(&fx.identity.identity_key()));
        assert!(departures.removed.is_empty());
        assert_eq!(
            departures.standing_of(
                iroh_rooms::room::Status::Active,
                &fx.identity.identity_key()
            ),
            Standing::Left
        );
    }

    // ------------------------------------------------------------------
    // Refusals: an unknown or out-of-range fact never becomes a known one
    // ------------------------------------------------------------------

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
            materialize_signed(1, &id, &ev, &snapshot, &Departures::default()).is_none(),
            "an unrecognized label must not become a fabricated known state"
        );
    }

    /// A progress percent outside the record's inclusive `0..=100` is
    /// **refused, never clamped**. Saturating it to `100` would report a task
    /// as complete on the strength of a number the author never signed.
    #[test]
    fn an_out_of_range_progress_percent_is_refused_not_clamped() {
        assert_eq!(progress(None), Some(Progress::Absent));
        assert_eq!(progress(Some(0)), Some(Progress::Reported { percent: 0 }));
        assert_eq!(
            progress(Some(100)),
            Some(Progress::Reported { percent: 100 })
        );
        for out_of_range in [101_u64, 255, 256, u64::MAX] {
            assert_eq!(
                progress(Some(out_of_range)),
                None,
                "percent {out_of_range} is not representable"
            );
        }
    }

    /// An unrepresentable instant leaves the row uncommitted rather than
    /// dating it to the Unix epoch.
    #[test]
    fn an_unrepresentable_instant_has_no_wire_form() {
        assert!(ts(0).is_some(), "the epoch itself is representable");
        assert!(ts(TS).is_some());
        assert!(
            ts(u64::MAX).is_none(),
            "an instant past the wire domain has no `<ts>`"
        );
    }

    /// A `member.invited` authors no committed event, so it holds no position
    /// and cannot win the recency projection.
    #[test]
    fn member_invited_is_not_a_committed_event() {
        let fx = fixture();
        let invitee = SigningKey::generate();
        let wire = build_member_invited(
            &fx.identity,
            &fx.device,
            &fx.room_id,
            &[0x07; 16],
            &[0x09; 32],
            "member",
            &invitee.identity_key(),
            None,
            None,
            &[],
            TS + 1,
        );
        let snapshot = snapshot_of(&fx);
        let ev = decode(&wire);
        let id = iroh_rooms::events::EventId::from_bytes([0x05; 32]);
        assert!(
            materialize_signed(1, &id, &ev, &snapshot, &Departures::default()).is_none(),
            "invite.mint authors no committed timeline event"
        );
    }

    /// `last_event` is `absent` **only** when the room has no committed event,
    /// never as a stand-in for one that could not be described.
    #[test]
    fn last_event_is_absent_only_for_a_room_with_no_committed_event() {
        assert_eq!(last_event(None), LastEvent::Absent);
        let at = ts(TS).unwrap();
        assert_eq!(
            last_event(Some(Recency {
                created_at_ms: TS,
                at,
                kind: EventKind::Message,
            })),
            LastEvent::Present {
                at,
                kind: EventKind::Message
            }
        );
    }

    /// Recency is ordered on the **signed millisecond**, not the
    /// second-precision wire form. Two events authored inside one second would
    /// otherwise tie, and the id tiebreak would pick whichever hashed lower —
    /// reporting a room's newest event as an older one at random.
    #[test]
    fn recency_orders_within_a_single_second() {
        let fx = fixture();
        let a = stored_event_recency_of(&fx, TS + 100);
        let b = stored_event_recency_of(&fx, TS + 900);
        assert_eq!(
            a.at, b.at,
            "both truncate to the same wire instant, which is why the raw ms matters"
        );
        assert!(
            b.created_at_ms > a.created_at_ms,
            "the raw signed instants still order"
        );
    }

    fn stored_event_recency_of(fx: &Fixture, created_at: u64) -> Recency {
        let mut store = EventStore::open_in_memory().unwrap();
        let ctx = ValidationContext::for_room(fx.room_id);
        let genesis = validate_wire_bytes(&fx.genesis.to_bytes(), &ctx).unwrap();
        let genesis_id = genesis.event_id;
        store.insert(&genesis).unwrap();
        let msg = validate_wire_bytes(
            &build_message_text(
                &fx.identity,
                &fx.device,
                &fx.room_id,
                "timed",
                None,
                None,
                &[],
                &[genesis_id],
                created_at,
            )
            .to_bytes(),
            &ctx,
        )
        .unwrap();
        store.insert(&msg).unwrap();
        let rows = store.room_tail(&fx.room_id, 100).unwrap();
        rows.iter()
            .filter_map(stored_event_recency)
            .find(|r| r.kind == EventKind::Message)
            .expect("the message is a committed event")
    }

    /// A pipe target another peer signed is projected **verbatim**, including
    /// a non-loopback host: the loopback rule is this daemon's publish-side
    /// policy, and suppressing a peer's target would hide the pipe most worth
    /// seeing. Only a hint that is not an address at all is unrepresentable.
    #[test]
    fn a_signed_pipe_target_is_projected_verbatim() {
        assert_eq!(
            pipe_target("127.0.0.1:3000"),
            Some(Target {
                host: "127.0.0.1".into(),
                port: 3000
            })
        );
        assert_eq!(
            pipe_target("192.168.1.10:22"),
            Some(Target {
                host: "192.168.1.10".into(),
                port: 22
            }),
            "a peer's non-loopback target is a fact, not something to hide"
        );
        assert_eq!(pipe_target("not-an-address"), None);
    }

    // ------------------------------------------------------------------
    // Positions
    // ------------------------------------------------------------------

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
        let departures = Departures::from_rows(rows.iter());
        let events: Vec<Event> = positioned(&refs, &snapshot, &departures)
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind.kind(), EventKind::RoomCreated);
        assert_eq!(events[1].kind.kind(), EventKind::Message);
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
        assert_eq!(rows[1].lamport, rows[2].lamport);

        let snapshot = snapshot_of(&fx);
        let refs: Vec<&StoredEvent> = rows.iter().collect();
        let departures = Departures::from_rows(rows.iter());
        let events: Vec<Event> = positioned(&refs, &snapshot, &departures)
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
        let departures = Departures::from_rows(rows.iter());
        let events: Vec<Event> = positioned(&refs, &snapshot, &departures)
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        assert_eq!(events.len(), 2, "the invite is not a committed event");
        assert_eq!(events[0].pos, 0);
        assert_eq!(events[1].pos, 1, "no position hole where the invite sat");
    }

    /// `is_committed` and `materialize` agree exactly: every push path relies
    /// on "is_committed implies materializable", so a divergence would panic
    /// the push loop rather than merely mis-serve a row.
    #[test]
    fn is_committed_implies_materializable() {
        let fx = fixture();
        let mut store = EventStore::open_in_memory().unwrap();
        let ctx = ValidationContext::for_room(fx.room_id);
        let genesis = validate_wire_bytes(&fx.genesis.to_bytes(), &ctx).unwrap();
        let genesis_id = genesis.event_id;
        store.insert(&genesis).unwrap();
        // A committed message, an uncommitted invite, and an out-of-vocabulary
        // status in one tail.
        for wire in [
            build_message_text(
                &fx.identity,
                &fx.device,
                &fx.room_id,
                "committed",
                None,
                None,
                &[],
                &[genesis_id],
                TS + 1,
            ),
            build_member_invited(
                &fx.identity,
                &fx.device,
                &fx.room_id,
                &[0x07; 16],
                &[0x09; 32],
                "member",
                &SigningKey::generate().identity_key(),
                None,
                None,
                &[genesis_id],
                TS + 2,
            ),
            build_agent_status(
                &fx.identity,
                &fx.device,
                &fx.room_id,
                "running_tests",
                None,
                &[],
                None,
                &[genesis_id],
                TS + 3,
            ),
        ] {
            let validated = validate_wire_bytes(&wire.to_bytes(), &ctx).unwrap();
            store.insert(&validated).unwrap();
        }

        let snapshot = snapshot_of(&fx);
        let rows = store.room_tail(&fx.room_id, 100).unwrap();
        let departures = Departures::from_rows(rows.iter());
        for se in &rows {
            assert_eq!(
                is_committed(se),
                materialize(se, 0, &snapshot, &departures).is_some(),
                "is_committed and materialize must never disagree"
            );
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
}
