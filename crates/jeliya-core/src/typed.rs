//! The typed protocol-v2 projection layer over [`RoomSupervisor`] (#165).
//!
//! Every protocol-facing read the engine serves is produced here as a typed
//! `jeliya-api` value — never a `serde_json::Value`. The supervisor keeps its
//! runtime machinery (node sessions, blob transfer, pipe forwarding); this
//! layer owns the *shape* every answer takes at the v2 boundary, converting
//! internal/Iroh structures through [`crate::projection`]. The authoritative
//! statement of every shape is `docs/protocol-v2.md`.
//!
//! Two deliberate boundaries, per the issue's non-goals:
//!
//! - **Signed-event and liveness semantics are unchanged.** The same folds,
//!   snapshots, and transfer flows run; only their served representation is
//!   re-typed.
//! - **Internal persistence JSON stays JSON.** `localstate.rs` and
//!   `identity.rs` are untouched; only the protocol-facing projections move
//!   to typed shapes.
//!
//! # The validation pipeline
//!
//! [`dispatch`] runs the record's **normative validation order** once, for
//! every operation, rather than leaving each body to re-implement it:
//!
//! | Stage | Runs for | Refusal |
//! |---|---|---|
//! | 1 structural | every operation ([`validate_structure`]) | `invalid_argument` |
//! | 2 subject | every operation except `subject.ensure` | `subject_absent` |
//! | 3 dedup | the `op_id`-deduplicated class (the engine's ledger) | the original reply |
//! | 4 room index | every operation whose `in` carries `room_id` ([`request_room`]) | `room_not_available` |
//! | 5 standing | as step 4, minus `room.archive` ([`standing_exempt`]) | `membership_ended` |
//! | 6 role | the four operations in [`requires_authority`] | `insufficient_standing` |
//! | 7 semantics | the operation body | the operation's own codes |
//!
//! Steps 4–6 produce one [`RoomContext`] — the room, its snapshot, the
//! caller's resolved role and standing, and the room's departure facts — which
//! is handed to the body. A body therefore never re-derives an authorization
//! answer the pipeline already settled, which is what keeps the gate from
//! being a rule with a quiet per-operation carve-out. `room.list` and the
//! three `stream.*` operations are the only room-shaped reads that do not take
//! one: `room.list` carries no `room_id` (it enumerates every room, including
//! left ones), and `stream.*` is connection-scoped and authorized by its host.
//!
//! # Paging and positions
//!
//! The v2 record fixes one continuation mechanism: every paging operation
//! takes a required [`Page`] (`cursor`, `direction`, `limit`) and answers a
//! [`Truncated`]. `cursor` and `direction` are closed wire types, so the codec
//! refuses a malformed one at step 1 before this layer sees it; `limit` is
//! bounded against a **served** limit only the daemon knows, so its step-1
//! bound check lives in [`validate_structure`] — ahead of the subject, room,
//! standing, role, and storage stages for all six paging operations.
//!
//! Positions are the **dense rank** over the room's canonical
//! `(lamport, event_id)` order — `0` for the genesis, exactly one past the
//! predecessor for every later committed event. The store's raw Lamport
//! value is not a position (concurrent siblings share one), so ranking
//! happens here at the read boundary and in the supervisor's typed push
//! paths, keeping the timeline, the push stream, and `stream.resync` in one
//! consistent position space. Cursor/direction paging is applied over that
//! space here.
//!
//! # Honesty
//!
//! No projection in this module invents a fact it does not hold. There is no
//! epoch timestamp for an unreadable instant, no default `member` role, no
//! default `active` standing, and no empty identifier in an error. Where a
//! fact is missing the answer is the operation's exact typed code — most often
//! `membership_unresolved` for a caller or member the fold cannot resolve, and
//! the `*_index_unreadable` code for a row a projection cannot compose.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use jeliya_api::*;
use serde::{Deserialize, Serialize};

use iroh_rooms::events::constants::SHORT_ID_LEN;
use iroh_rooms::events::{capability_hash, Content, EventType, SignedEvent};
use iroh_rooms::room::{MembershipSnapshot, RoomId as IrohRoomId};

use crate::error::{CoreError, CoreResult, ErrorKind};
use crate::projection::{self as proj, file_handle, Departures};
use crate::supervisor::{RemoveMemberOutcome, RoomSupervisor};

/// The daemon's served limits, surfaced in `hello`/`VersionInfo` and enforced
/// here. Values are the record's served-limits object; the shared-file maximum
/// is the size policy's 100 MiB (`max_shared_file_bytes`), the message-body and
/// page bounds come from the same constants the v1 surface enforced.
#[must_use]
pub fn limits() -> Limits {
    Limits {
        max_shared_file_bytes: crate::supervisor::FILE_UPLOAD_MAX_BYTES,
        max_message_body_bytes: 16_384,
        max_frame_bytes: 128 * 1024 * 1024,
        max_inflight_requests: 64,
        max_subscriptions_per_connection: 64,
        max_connections: 64,
        max_concurrent_transfers: 8,
        max_transfer_bytes_inflight: 256 * 1024 * 1024,
        transfer_connect_allowance_ms: 5_000,
        transfer_floor_bits_per_second: 8_192,
        transfer_stall_ms: 30_000,
        timeline_page_max: 1_024,
        idle_timeout_ms: 600_000,
        pairing_code_ttl_ms: 900_000,
        pairing_code_max_attempts: 5,
        browser_session_ttl_ms: 86_400_000,
    }
}

/// The fleet projection's per-(subject, room) aggregation.
type FleetAggMap = BTreeMap<(String, String), (Liveness, Option<(String, u64)>, Option<u64>)>;

/// Per-agent signal accumulation over a room's stored events: known device
/// keys, the newest status, and the newest event ts.
type AgentSignalsMap = BTreeMap<
    iroh_rooms::identity::IdentityKey,
    (
        BTreeSet<iroh_rooms::identity::DeviceKey>,
        Option<(String, u64)>,
        Option<u64>,
    ),
>;

/// A paging window over the v2 position space. `cursor` and `direction` are
/// the record's required paging fields; `limit` is bounded to
/// `timeline_page_max` (refused, never clamped — the bound check lives in the
/// engine before this layer).
#[derive(Debug, Clone, Copy)]
struct Window {
    /// Resolved starting position (inclusive lower bound), or `None` for the
    /// very start.
    from: Option<u64>,
    /// Page size.
    limit: usize,
    /// `true` = forward (newer first-encountered), `false` = backward.
    forward: bool,
}

impl Window {
    fn resolve(page: &Page) -> Result<Self, ApiError> {
        let from = match &page.cursor {
            Cursor::Start => None,
            Cursor::At { pos } => Some(*pos),
        };
        // The record requires `limit` in `1..=timeline_page_max`, refused —
        // never clamped and never defaulted.
        let max = limits().timeline_page_max;
        if page.limit == 0 || page.limit > max {
            return Err(ApiError::InvalidArgument {
                field: "in.limit".into(),
                reason: InvalidReason::Bound { min: 1, max },
            });
        }
        // The bound above already proved `limit <= timeline_page_max`, which
        // fits `usize` on every supported target, so this conversion cannot
        // saturate a caller's page size into a different one.
        let limit = usize::try_from(page.limit).map_err(|_| ApiError::InvalidArgument {
            field: "in.limit".into(),
            reason: InvalidReason::Bound { min: 1, max },
        })?;
        let forward = matches!(page.direction, Direction::Forward);
        Ok(Self {
            from,
            limit,
            forward,
        })
    }
}

/// Apply a [`Window`] to a position-ordered list of committed events,
/// returning the page plus the one continuation mechanism.
///
/// **Forward** reads newer-from-cursor: the first `limit` events at or after
/// `from`, and a `more` cursor naming the next unread position. **Backward**
/// reads older-from-cursor: the newest `limit` events strictly before `from`
/// (or the newest `limit` overall for a `start` cursor), served in
/// first-encountered (ascending) order, with the continuation cursor naming
/// the position the *oldest* returned event sits at, so the next backward
/// page reads strictly below it — no omission and no duplication.
fn page_events(events: Vec<Event>, window: Window) -> (Vec<Event>, Truncated) {
    if window.forward {
        let filtered: Vec<Event> = events
            .into_iter()
            .filter(|e| window.from.is_none_or(|f| e.pos >= f))
            .collect();
        let mut page: Vec<Event> = filtered
            .into_iter()
            .take(window.limit.saturating_add(1))
            .collect();
        let truncated = if page.len() > window.limit {
            page.truncate(window.limit);
            proj::truncated(page.last().map(|e| e.pos + 1))
        } else {
            Truncated::Complete
        };
        (page, truncated)
    } else {
        // Backward: candidates are strictly below `from` (or all of them for a
        // start cursor), newest first for selection.
        let mut candidates: Vec<Event> = events
            .into_iter()
            .filter(|e| window.from.is_none_or(|f| e.pos < f))
            .collect();
        candidates.reverse(); // newest first
        let mut page: Vec<Event> = candidates
            .into_iter()
            .take(window.limit.saturating_add(1))
            .collect();
        let truncated = if page.len() > window.limit {
            // Drop the extra (oldest) probe event, then continue from the
            // oldest KEPT event so the next page reads strictly below it.
            page.truncate(window.limit);
            proj::truncated(page.last().map(|e| e.pos))
        } else {
            Truncated::Complete
        };
        page.reverse(); // serve in first-encountered (ascending) order
        (page, truncated)
    }
}

/// Apply a [`Window`] to an index-addressed list (status history, file list,
/// pipe list), honoring direction and returning the one continuation
/// mechanism. These lists are position-by-index, not by committed `pos`, so
/// the cursor is an index. Forward reads `limit` rows from `cursor`; backward
/// reads the `limit` rows ending just before `cursor` (or the newest `limit`
/// for a start cursor), served in forward (ascending-index) order, with the
/// continuation cursor naming the index the next backward page reads below.
fn page_indexed<T: Clone>(items: Vec<T>, window: Window) -> (Vec<T>, Truncated) {
    let total = items.len();
    if window.forward {
        let start = window.from.map_or(0, |f| (f as usize).min(total));
        let end = (start + window.limit).min(total);
        let page: Vec<T> = items[start..end].to_vec();
        let truncated = if end < total {
            proj::truncated(Some(end as u64))
        } else {
            Truncated::Complete
        };
        (page, truncated)
    } else {
        // Backward: the newest `limit` rows strictly before `from` (or the
        // newest `limit` overall for a start cursor).
        let end = window.from.map_or(total, |f| (f as usize).min(total));
        let start = end.saturating_sub(window.limit);
        let page: Vec<T> = items[start..end].to_vec();
        let truncated = if start > 0 {
            proj::truncated(Some(start as u64))
        } else {
            Truncated::Complete
        };
        (page, truncated)
    }
}

/// What validation-order steps 4–6 resolved for one room-bearing operation:
/// the room the caller named, its membership fold, the caller's **resolved**
/// role and standing, and the room's signed departure facts.
///
/// Every field is a fact the pipeline established before step 7 ran. A body
/// that takes a `RoomContext` therefore cannot reach a state where it must
/// guess the caller's role or standing — the case the record removes the
/// fabricated `member` and `active` defaults for — because a caller the fold
/// could not resolve never produced one.
pub(crate) struct RoomContext {
    /// The parsed room id.
    room_id: IrohRoomId,
    /// The membership fold at read time.
    snapshot: MembershipSnapshot,
    /// The caller's resolved role.
    role: Role,
    /// The caller's resolved standing.
    standing: Standing,
    /// The room's signed departure facts (`left` vs `removed`).
    departures: Departures,
    /// The caller's own subject, as the fold resolved it.
    self_key: iroh_rooms::identity::IdentityKey,
}

/// The typed projection facade. Cheap to construct; borrows the supervisor.
pub(crate) struct TypedSupervisor<'a> {
    sup: &'a RoomSupervisor,
}

impl<'a> TypedSupervisor<'a> {
    /// Wrap a supervisor.
    #[must_use]
    pub(crate) fn new(sup: &'a RoomSupervisor) -> Self {
        Self { sup }
    }

    /// Parse a `<room_id>` for the room-index stage.
    ///
    /// The record defines every identifier as an **opaque string with no
    /// published format**, so a `room_id` this daemon cannot parse is not a
    /// malformed argument — it is a room that is not available, and it answers
    /// exactly what an unknown-or-unauthorized room answers. Refusing it as
    /// `invalid_argument` instead would hand a caller a second, distinguishable
    /// answer for "no such room", which is the membership oracle
    /// `room_not_available` exists as one code to prevent.
    fn parse_room(api_room: &RoomId) -> Result<IrohRoomId, ApiError> {
        api_room
            .as_str()
            .trim()
            .parse()
            .map_err(|_| ApiError::RoomNotAvailable {
                room_id: api_room.clone(),
            })
    }

    fn parse_subject(subject_id: &SubjectId) -> CoreResult<iroh_rooms::identity::IdentityKey> {
        subject_id.as_str().trim().parse().map_err(|e| {
            CoreError::invalid(format!("invalid subject_id (expected 64-char hex): {e}"))
        })
    }

    fn parse_file(file_id: &FileId) -> CoreResult<[u8; 16]> {
        let trimmed = file_id.as_str().trim();
        let hex_part = trimmed.strip_prefix("file_").unwrap_or(trimmed);
        let bytes = hex::decode(hex_part)
            .map_err(|_| CoreError::invalid(format!("invalid file_id {trimmed:?}")))?;
        <[u8; 16]>::try_from(bytes.as_slice())
            .map_err(|_| CoreError::invalid(format!("invalid file_id {trimmed:?}")))
    }

    /// Validation-order steps 4, 5, and 6 for one room-bearing operation, in
    /// that order and in one place.
    ///
    /// - **4 room index** — an unparseable, unknown, or unreadable-to-this-
    ///   caller room is `room_not_available` echoing the id the caller sent;
    ///   an index this daemon cannot read at all is `room_index_unreadable`.
    /// - **5 standing** — a `left` or `removed` caller is `membership_ended`,
    ///   *except* for the operations defined over a former membership.
    /// - **6 role** — the four authority operations refuse a plain member with
    ///   `insufficient_standing { required, held }`.
    ///
    /// The caller's role and standing are **resolved, never defaulted**: a
    /// caller the fold has no row for is `membership_unresolved`, because the
    /// alternative — assuming `member`/`active` — is a fabricated
    /// authorization answer, and the fail-closed direction of that assumption
    /// does not make it true.
    async fn room_context(
        &self,
        api_room: &RoomId,
        standing_exempt: bool,
        authority: bool,
    ) -> Result<RoomContext, ApiError> {
        // Step 4.
        let room_id = Self::parse_room(api_room)?;
        let snapshot = self
            .sup
            .readable_snapshot(&room_id)
            .await
            .map_err(|error| match error.kind {
                ErrorKind::Internal => ApiError::RoomIndexUnreadable,
                _ => core_to_api_room(error, api_room),
            })?;
        let self_key = self.sup.local_identity_key().map_err(core_to_api)?;
        let store = self
            .sup
            .open_store()
            .map_err(|_| ApiError::RoomIndexUnreadable)?;
        let departures = crate::supervisor::departure_sets(&store, &room_id)
            .map_err(|_| ApiError::RoomIndexUnreadable)?;
        drop(store);

        // The caller's own membership facts, resolved from the signed fold.
        let member = snapshot
            .member(&self_key)
            .ok_or_else(|| ApiError::MembershipUnresolved {
                room_id: api_room.clone(),
                subject_id: SubjectId::new(self_key.to_string()),
            })?;
        let role = proj::role(member.role);
        let standing = departures.standing_of(member.status, &member.identity);

        // Step 5.
        if !standing_exempt && standing != Standing::Active {
            return Err(ApiError::MembershipEnded {
                room_id: api_room.clone(),
                standing,
            });
        }
        // Step 6.
        if authority && role != Role::Authority {
            return Err(ApiError::InsufficientStanding {
                room_id: api_room.clone(),
                required: Role::Authority,
                held: role,
            });
        }
        Ok(RoomContext {
            room_id,
            snapshot,
            role,
            standing,
            departures,
            self_key,
        })
    }

    // ------------------------------------------------------------------
    // Subject and daemon
    // ------------------------------------------------------------------

    /// `subject.ensure` — establish the local subject exactly once; a second
    /// call returns the same subject with `created: false` (naturally
    /// idempotent, never an `identity_exists` refusal).
    ///
    /// A creation this daemon cannot persist is `subject_store_unwritable`, the
    /// one operation error the record gives `subject.ensure`. It does not reach
    /// the roomless fallback: that answers `room_index_unreadable`, which names
    /// an index this operation never opened and denies the caller the only code
    /// that tells them their fresh subject was not written to disk.
    pub fn subject_ensure(&self) -> Result<SubjectEnsureOut, ApiError> {
        let existing = crate::identity::load_profile(self.sup.data_dir())
            .map_err(|_| ApiError::SubjectStoreUnwritable)?;
        if let Some(profile) = existing {
            return Ok(SubjectEnsureOut {
                subject_id: SubjectId::new(profile.identity_id),
                device_id: DeviceId::new(profile.device_id),
                created: false,
            });
        }
        let profile = match crate::identity::create(self.sup.data_dir()) {
            Ok(profile) => profile,
            // The TOCTOU loser: `create_new(true)` is the atomic guard, so a
            // concurrent `subject.ensure` can win the race between the read
            // above and this write. v2 removed `identity_exists`, and the
            // operation is naturally idempotent, so the loser re-reads what the
            // winner wrote and reports it as the existing subject it is.
            Err(err) if err.kind == ErrorKind::IdentityExists => {
                let profile = crate::identity::load_profile(self.sup.data_dir())
                    .ok()
                    .flatten()
                    .ok_or(ApiError::SubjectStoreUnwritable)?;
                return Ok(SubjectEnsureOut {
                    subject_id: SubjectId::new(profile.identity_id),
                    device_id: DeviceId::new(profile.device_id),
                    created: false,
                });
            }
            Err(_) => return Err(ApiError::SubjectStoreUnwritable),
        };
        Ok(SubjectEnsureOut {
            subject_id: SubjectId::new(profile.identity_id),
            device_id: DeviceId::new(profile.device_id),
            created: true,
        })
    }

    /// Whether a local subject exists (the step-2 precondition).
    pub(crate) fn subject_present(&self) -> Result<bool, ApiError> {
        crate::identity::load_profile(self.sup.data_dir())
            .map(|p| p.is_some())
            .map_err(|_| ApiError::NotReady)
    }

    /// The `hello` `subject` fact: present with ids, its stated absence, or
    /// `not_ready` when the subject store cannot be read (corrupt or
    /// permission-denied — the connection must not be invited to run
    /// `subject.ensure` against unreadable existing state).
    pub fn subject_state(&self) -> Result<SubjectState, ApiError> {
        match crate::identity::load_profile(self.sup.data_dir()) {
            Ok(Some(p)) => Ok(SubjectState::Present {
                subject_id: SubjectId::new(p.identity_id),
                device_id: DeviceId::new(p.device_id),
            }),
            Ok(None) => Ok(SubjectState::Absent),
            Err(_) => Err(ApiError::NotReady),
        }
    }

    // ------------------------------------------------------------------
    // Rooms
    // ------------------------------------------------------------------

    /// `room.create` — bring a room into existence with the caller as its
    /// authority; works with no network.
    ///
    /// A name outside `1..=128` bytes after trimming, or one with no
    /// non-whitespace character, is `room_name_invalid` carrying the same
    /// closed `reason` variant `invalid_argument` uses — a `bound` arm for
    /// length and a `format` arm for whitespace-only, per the record's stated
    /// bounds.
    pub async fn room_create(&self, req: &RoomCreate) -> Result<RoomCreateOut, ApiError> {
        let name = req.name.trim();
        if name.is_empty() {
            // Empty-after-trimming covers both "no characters at all" and
            // "whitespace only"; the record spells the latter as `format`.
            return Err(ApiError::RoomNameInvalid {
                reason: if req.name.is_empty() {
                    InvalidReason::Bound { min: 1, max: 128 }
                } else {
                    InvalidReason::Format
                },
            });
        }
        if name.len() > 128 {
            return Err(ApiError::RoomNameInvalid {
                reason: InvalidReason::Bound { min: 1, max: 128 },
            });
        }
        let room_id_str = self.sup.create_room(name).map_err(core_to_api)?;
        let api_room = RoomId::new(room_id_str);
        let room_id = Self::parse_room(&api_room)?;
        // The genesis is the room's origin event at pos 0, found **by kind**
        // rather than as the one-row tail. `created_at` is the instant its
        // author signed — read back from the persisted event, never the wall
        // clock, and never an epoch stand-in if it will not convert.
        let store = self.sup.open_store().map_err(core_to_api)?;
        let (event_id, created_at) = store
            .by_type(&room_id, EventType::RoomCreated)
            .map_err(|_| ApiError::RoomIndexUnreadable)?
            .iter()
            .find_map(|se| {
                let ev = SignedEvent::decode(&se.wire.signed).ok()?;
                Some((se.event_id, proj::ts(ev.created_at)?))
            })
            .ok_or(ApiError::RoomIndexUnreadable)?;
        Ok(RoomCreateOut {
            room_id: api_room,
            name: name.to_string(),
            role: Role::Authority,
            standing: Standing::Active,
            event_id: proj::event_id(&event_id),
            pos: 0,
            created_at,
        })
    }

    /// `room.list` — every room this identity holds, in what standing, with
    /// recency and capabilities, from local evidence with zero network
    /// activity.
    pub async fn room_list(&self) -> Result<RoomListOut, ApiError> {
        // A subject with no room store genuinely holds no rooms, and the empty
        // list means exactly that. A subject-less daemon is a different fact
        // and answers `subject_absent` — the record removes v1's pre-identity
        // `room.list` carve-out precisely because one precondition answered two
        // ways let an empty list stand in for a missing subject. The pipeline's
        // step 2 already refuses that case; this arm keeps the answer the same
        // for a direct caller rather than reintroducing the carve-out below it.
        if !self.sup.db_path().exists() {
            return Ok(RoomListOut { rooms: Vec::new() });
        }
        let self_key = self.sup.local_identity_key().map_err(core_to_api)?;
        let room_ids: Vec<IrohRoomId> = crate::localstate::load(self.sup.data_dir())
            .map_err(|_| ApiError::RoomIndexUnreadable)?
            .rooms
            .keys()
            .filter_map(|room_id| room_id.parse().ok())
            .collect();
        let mut rooms = Vec::with_capacity(room_ids.len());
        for room_id in room_ids {
            // A locally indexed room whose log will not fold is a read
            // failure, not an absent room: silently dropping it would present
            // an incomplete index as complete. Surface `room_index_unreadable`
            // rather than a misleading partial list.
            let snapshot = self
                .sup
                .snapshot_for(&room_id)
                .await
                .map_err(|_| ApiError::RoomIndexUnreadable)?;
            if RoomSupervisor::require_local_room_access(&snapshot, &self_key).is_err() {
                continue;
            }
            let name = {
                let store = self.sup.open_store().map_err(core_to_api)?;
                crate::supervisor::genesis_name(&store, &room_id).or_else(|| {
                    crate::localstate::local_name(self.sup.data_dir(), &room_id.to_string())
                })
            };
            // The caller's own role and standing come from the fold, never a
            // default. A row in the local index whose fold has no membership
            // for this caller cannot be described truthfully, so it answers
            // `membership_unresolved` rather than a row claiming `member` /
            // `active`.
            let api_room = proj::room_id(&room_id);
            let store = self.sup.open_store().map_err(core_to_api)?;
            let departures = crate::supervisor::departure_sets(&store, &room_id)
                .map_err(|_| ApiError::RoomIndexUnreadable)?;
            let member =
                snapshot
                    .member(&self_key)
                    .ok_or_else(|| ApiError::MembershipUnresolved {
                        room_id: api_room.clone(),
                        subject_id: SubjectId::new(self_key.to_string()),
                    })?;
            let role = proj::role(member.role);
            let standing = departures.standing_of(member.status, &member.identity);
            // Recency: the newest COMMITTED event's author-dated instant and
            // kind, max by timestamp over the COMPLETE history (never the wall
            // clock, never a bounded window — a clock-ahead peer's older row
            // with the greatest signed instant must not fall out and move the
            // projection backward). Non-committed rows are excluded from the
            // max, not merely dropped after winning it: an invitation is not a
            // timeline event, and letting one win would report `absent` for a
            // room whose timeline is full.
            let recency = store
                .room_tail(&room_id, u32::MAX)
                .map_err(|_| ApiError::RoomIndexUnreadable)?
                .iter()
                .filter_map(|se| proj::stored_event_recency(se).map(|r| (r, se.event_id)))
                // Ordered on the signed millisecond, not the second-precision
                // wire form, so two events in the same second still order.
                // `event_id` breaks a true tie so the answer is deterministic.
                .max_by(|a, b| {
                    a.0.created_at_ms
                        .cmp(&b.0.created_at_ms)
                        .then_with(|| a.1.cmp(&b.1))
                })
                .map(|(recency, _)| recency);
            let last_event = proj::last_event(recency);
            let live = self.sup.is_open(&room_id);
            // The same signed-membership set `room.members` serves, so the
            // count and the roster can never disagree. An outstanding
            // invitation is not a member.
            let member_count = snapshot.members().filter(|m| m.device.is_some()).count() as u64;
            let capabilities = room_capabilities(standing, role, live);
            // `name` is a required `<string>`: a room whose genesis name is
            // unreadable and that carries no local override cannot produce a
            // truthful row, and `""` would render as an unnamed room that does
            // not exist. That is an unreadable index entry, and it says so.
            let name = name.ok_or(ApiError::RoomIndexUnreadable)?;
            rooms.push(RoomRow {
                room_id: api_room,
                name,
                standing,
                live,
                role,
                member_count,
                last_event,
                capabilities,
            });
        }
        Ok(RoomListOut { rooms })
    }

    /// `room.activate` — make a room live on this device; returns reachability
    /// and capabilities, **not history**. Naturally idempotent.
    pub async fn room_activate(
        &self,
        ctx: &RoomContext,
        req: &RoomActivate,
    ) -> Result<RoomActivateOut, ApiError> {
        self.sup
            .activate_room(req.room_id.as_ref(), &[])
            .await
            .map_err(|error| match error.kind {
                // A room the caller may act in that will not come live is a
                // transport fact, and it has its own code.
                ErrorKind::Internal | ErrorKind::PeerUnreachable => {
                    ApiError::TransportUnavailable {
                        room_id: req.room_id.clone(),
                    }
                }
                _ => core_to_api_room(error, &req.room_id),
            })?;
        let live = self.sup.is_open(&ctx.room_id);
        let reachability = self.reachability(&ctx.room_id).await;
        Ok(RoomActivateOut {
            room_id: req.room_id.clone(),
            live,
            reachability,
            capabilities: room_capabilities(ctx.standing, ctx.role, live),
        })
    }

    /// `room.deactivate` — stop live participation without changing membership.
    pub async fn room_deactivate(
        &self,
        _ctx: &RoomContext,
        req: &RoomDeactivate,
    ) -> Result<RoomDeactivateOut, ApiError> {
        match self.sup.close_room(req.room_id.as_ref()).await {
            Ok(()) => {}
            // Deactivation is naturally idempotent. A concurrent or repeated
            // close is success, but only after the pipeline's standing stage
            // has proved this is still an active membership.
            Err(err) if err.kind == ErrorKind::RoomNotOpen => {}
            Err(err) => return Err(core_to_api_room(err, &req.room_id)),
        }
        Ok(RoomDeactivateOut {
            room_id: req.room_id.clone(),
            live: false,
        })
    }

    /// `room.leave` — author a signed departure every member converges on.
    pub async fn room_leave(
        &self,
        ctx: &RoomContext,
        req: &RoomLeave,
    ) -> Result<RoomLeaveOut, ApiError> {
        let event_id_hex = self
            .sup
            .leave_room(req.room_id.as_ref())
            .await
            .map_err(|error| match error.kind {
                // The creator is permanently the sole authority and cannot
                // leave; that refusal has its own code, not a generic one.
                ErrorKind::PipeDenied | ErrorKind::NotAMember if ctx.role == Role::Authority => {
                    ApiError::SoleAuthorityCannotLeave {
                        room_id: req.room_id.clone(),
                    }
                }
                _ => core_to_api_room(error, &req.room_id),
            })?;
        let pos = self.pos_of_event(&ctx.room_id, &event_id_hex)?;
        Ok(RoomLeaveOut {
            room_id: req.room_id.clone(),
            event_id: EventId::new(event_id_hex),
            pos,
            standing: Standing::Left,
        })
    }

    /// `room.timeline` — read committed history identically whether or not
    /// the room is live. A left or removed caller is `membership_ended` at the
    /// pipeline's standing stage — only `room.archive` and `room.list` are
    /// defined over a former membership.
    pub fn room_timeline(
        &self,
        ctx: &RoomContext,
        req: &RoomTimeline,
    ) -> Result<RoomTimelineOut, ApiError> {
        let events = self.committed_events(ctx)?;
        let (page, truncated) = page_events(events, Window::resolve(&req.page)?);
        Ok(RoomTimelineOut {
            room_id: req.room_id.clone(),
            events: page,
            truncated,
        })
    }

    /// `room.members` — the authoritative signed answer to who belongs. No
    /// presence, no reachability.
    ///
    /// `joined_at` is the join event's author-dated instant. A member whose
    /// join this daemon cannot date is `membership_unresolved` naming that
    /// member: the record removes compatibility-nullability, and the epoch is
    /// not a truthful stand-in for an instant nobody signed.
    pub fn room_members(
        &self,
        ctx: &RoomContext,
        req: &RoomMembers,
    ) -> Result<RoomMembersOut, ApiError> {
        let mut members = Vec::new();
        // The roster is **signed membership**, not the fold's every-known-row
        // set. The upstream snapshot also yields an `Invited` row for an
        // identity that was offered a capability and has not redeemed it; that
        // identity is not a member, has no join to date, and belongs to
        // `invite.list`. A joined identity carries a device binding, and keeps
        // it after leaving or being removed, so the binding is exactly the
        // "did this subject ever join" test — the same one the room read
        // boundary uses. Including an invited row here would either date it to
        // a fabricated instant or, as an earlier revision did, fail the whole
        // roster with `membership_unresolved` the moment any invite is
        // outstanding.
        for m in ctx.snapshot.members().filter(|m| m.device.is_some()) {
            let joined_at = self.joined_at(&ctx.room_id, &m.identity).ok_or_else(|| {
                ApiError::MembershipUnresolved {
                    room_id: req.room_id.clone(),
                    subject_id: SubjectId::new(m.identity.to_string()),
                }
            })?;
            members.push(MemberRow {
                subject_id: SubjectId::new(m.identity.to_string()),
                role: proj::role(m.role),
                standing: ctx.departures.standing_of(m.status, &m.identity),
                joined_at,
            });
        }
        Ok(RoomMembersOut {
            room_id: req.room_id.clone(),
            members,
        })
    }

    /// `room.archive` — open a left or removed room as a local read-only
    /// archive; normatively zero network activity and zero durable mutation.
    /// Exempt from the pipeline's standing stage, so it checks the converse
    /// here: an active membership is `room_still_active`.
    pub fn room_archive(
        &self,
        ctx: &RoomContext,
        req: &RoomArchive,
    ) -> Result<RoomArchiveOut, ApiError> {
        if ctx.standing == Standing::Active {
            return Err(ApiError::RoomStillActive {
                room_id: req.room_id.clone(),
            });
        }
        let events = self.committed_events(ctx)?;
        let (page, truncated) = page_events(events, Window::resolve(&req.page)?);
        Ok(RoomArchiveOut {
            room_id: req.room_id.clone(),
            standing: ctx.standing,
            events: page,
            truncated,
        })
    }

    /// `room.peers` — observed transport facts for one live room. Requires
    /// liveness, so a non-live room is `room_not_live`.
    pub async fn room_peers(
        &self,
        ctx: &RoomContext,
        req: &RoomPeers,
    ) -> Result<RoomPeersOut, ApiError> {
        let session = self
            .sup
            .session(&ctx.room_id)
            .map_err(|_| ApiError::RoomNotLive {
                room_id: req.room_id.clone(),
            })?;
        let peers = self.peer_rows(&session.node).await;
        let reachability = reachability_from_peers(&peers, self.sup.is_open(&ctx.room_id));
        Ok(RoomPeersOut {
            room_id: req.room_id.clone(),
            reachability,
            peers,
        })
    }

    /// `member.remove` — room authority removes a joined member as a signed
    /// fact. A repeat against the same terminal removal returns that original
    /// fact without authoring another event.
    pub async fn member_remove(
        &self,
        ctx: &RoomContext,
        req: &MemberRemove,
    ) -> Result<MemberRemoveOut, ApiError> {
        let room_id = ctx.room_id;
        let subject = Self::parse_subject(&req.subject_id).map_err(core_to_api)?;
        let outcome = self
            .sup
            .remove_member(&room_id, &subject)
            .await
            .map_err(|error| core_to_api_room(error, &req.room_id))?;
        let event_id = match outcome {
            RemoveMemberOutcome::Removed(event_id) => event_id,
            RemoveMemberOutcome::Authority => {
                return Err(ApiError::AuthorityCannotBeRemoved {
                    room_id: req.room_id.clone(),
                    subject_id: req.subject_id.clone(),
                })
            }
            RemoveMemberOutcome::Unknown => {
                return Err(ApiError::MemberUnknown {
                    room_id: req.room_id.clone(),
                    subject_id: req.subject_id.clone(),
                })
            }
        };
        let pos = self.pos_of_event(&room_id, &event_id)?;
        Ok(MemberRemoveOut {
            room_id: req.room_id.clone(),
            subject_id: req.subject_id.clone(),
            event_id: EventId::new(event_id),
            pos,
            standing: Standing::Removed,
        })
    }

    // ------------------------------------------------------------------
    // Invitations
    // ------------------------------------------------------------------

    /// `invite.mint` — mint one key-bound capability exactly one named
    /// identity can redeem. Authority-only: the role gate is validation-order
    /// step 6, applied by [`dispatch`] before this body runs.
    pub async fn invite_mint(
        &self,
        ctx: &RoomContext,
        req: &InviteMint,
    ) -> Result<InviteMintOut, ApiError> {
        // `role` accepts `member` only today; `authority` is
        // `role_not_grantable` carrying the role that was requested.
        if req.role != Role::Member {
            return Err(ApiError::RoleNotGrantable {
                requested: req.role,
            });
        }
        // v2 `expires_at` is an absolute instant; the supervisor takes a
        // relative spec. Convert absolute -> seconds-from-now. A past or
        // already-expiring expiry is refused rather than minting a capability
        // that is born expired yet labelled `outstanding` — the reply's
        // redeemability must agree with the capability's signed expiry. The
        // taxonomy has no expiry-specific code, so it is the step-1 bound it
        // actually is, naming the field and the instant it must exceed.
        //
        // The bound arm's `min`/`max` are **both** whole seconds since the
        // epoch, the same domain the refused `<ts>` sits in. An earlier
        // revision paired a seconds `min` with a milliseconds `max`, so a
        // client reading the pair saw a window ~1000x too wide and could not
        // tell which unit either number was in.
        //
        // The instant is handed to the runtime **absolutely**, not as a
        // relative spec it would re-resolve against a later clock: the expiry
        // the caller asked for must be the expiry the capability is signed
        // with, or the reply, the `invite.list` row, and the capability itself
        // can all name different instants.
        //
        // `max` is the largest instant a `<ts>` can carry — `time` is built
        // without `large-dates`, so year 9999 is the ceiling. It is a
        // **representability** ceiling, not a maximum invite TTL: the record
        // defines no maximum TTL (`invites.json` records its absence as an open
        // resource gap), so an invented threshold would refuse expiries v2
        // permits. An earlier revision served `u32::MAX` here — borrowed from
        // the unrelated `room_tail(_, u32::MAX)` idiom, with no basis in the
        // timestamp domain — which advertised an "inclusive maximum" the daemon
        // happily minted a thousand-fold past. A bound no two implementations
        // can agree on is the thing this arm exists to prevent.
        const TS_MAX_SECS: u64 = 253_402_300_799; // 9999-12-31T23:59:59Z
        let now_ms = crate::now_ms();
        // Milliseconds, not whole seconds: the instant is the caller's, and it
        // is signed verbatim so the reply, the `invite.list` row, and the
        // capability all name the one instant that was asked for.
        let expires_ms =
            u64::try_from(req.expires_at.into_inner().unix_timestamp_nanos() / 1_000_000)
                .unwrap_or(0);
        if expires_ms <= now_ms {
            return Err(ApiError::InvalidArgument {
                field: "in.expires_at".into(),
                reason: InvalidReason::Bound {
                    min: now_ms / 1000 + 1,
                    max: TS_MAX_SECS,
                },
            });
        }
        let ticket = self
            .sup
            .create_invite_at(
                req.room_id.as_ref(),
                req.subject_id.as_str(),
                "member",
                Some(expires_ms),
            )
            .await
            .map_err(|error| core_to_api_room(error, &req.room_id))?;
        // The ticket is the capability string; its id is derivable from the
        // ticket itself for the reply.
        let parsed: iroh_rooms::room::RoomInviteTicket = ticket
            .trim()
            .parse()
            .map_err(|_| ApiError::InviteIndexUnreadable)?;
        // `expires_at` is the instant the capability was **signed** with, read
        // back off the minted ticket — not the instant the caller asked for.
        // The two differ: the request is converted to a whole-second relative
        // spec the supervisor then applies to its own clock, so echoing the
        // request would promise an expiry the capability does not carry and
        // would disagree with the `invite.list` row for the same invite, which
        // serves the signed value.
        let expires_at = parsed
            .expires_at
            .and_then(proj::ts_millis)
            .ok_or(ApiError::InviteIndexUnreadable)?;
        let _ = ctx;
        Ok(InviteMintOut {
            invite_id: proj::invite_id(&parsed.invite_id),
            room_id: req.room_id.clone(),
            subject_id: req.subject_id.clone(),
            role: req.role,
            expires_at,
            capability: ticket,
            redeemability: Redeemability::Outstanding,
        })
    }

    /// `invite.redeem` — convert a capability into signed membership.
    ///
    /// The only operation a non-member can reach, so **every** capability
    /// failure is one of the four fieldless-or-instant redemption-side codes
    /// and never a room-scoped one: `room_not_available` here would name a
    /// room to a caller who is not in it, which is exactly the probe the
    /// non-oracle property forbids.
    ///
    /// `joined` reports whether *this* call authored the membership. It is read
    /// from the store — the signed `member_joined` for this subject and its
    /// dense position — rather than asserted, because without it a replay is
    /// byte-identical to a fresh join.
    pub async fn invite_redeem(&self, req: &InviteRedeem) -> Result<InviteRedeemOut, ApiError> {
        // The absolute expiry the capability itself carries, so an expired one
        // reports the instant it expired rather than the epoch.
        let ticket: Option<iroh_rooms::room::RoomInviteTicket> = req.capability.trim().parse().ok();
        let expired_at = ticket
            .as_ref()
            .and_then(|t| t.expires_at)
            .and_then(proj::ts_millis);
        // Keyed on the capability being redeemed, so "already a member" means
        // "this capability already authored a join" and not "I have joined this
        // room at some point" — the two differ for a subject that was removed
        // and walked back in on a fresh capability.
        let joined_before = ticket
            .as_ref()
            .and_then(|t| self.membership_event(&t.room_id, Some(&t.invite_id)));

        let room_id_str = self
            .sup
            .join_room(&req.capability, None, &[])
            .await
            .map_err(|error| redemption_error(error, expired_at))?;
        let api_room = RoomId::new(room_id_str);
        let room_id = Self::parse_room(&api_room).map_err(|_| ApiError::CapabilityInvalid)?;
        let snapshot = self
            .sup
            .snapshot_for(&room_id)
            .await
            .map_err(|_| ApiError::CapabilityInvalid)?;
        let self_key = self.sup.local_identity_key().map_err(core_to_api)?;
        let subject_id = SubjectId::new(self_key.to_string());
        // Role and standing come from the fold, never a default: a redeemer the
        // fold cannot resolve is `membership_unresolved`, not a fabricated
        // active member.
        let member = snapshot
            .member(&self_key)
            .ok_or_else(|| ApiError::MembershipUnresolved {
                room_id: api_room.clone(),
                subject_id: subject_id.clone(),
            })?;
        let store = self.sup.open_store().map_err(core_to_api)?;
        let departures = crate::supervisor::departure_sets(&store, &room_id)
            .map_err(|_| ApiError::RoomIndexUnreadable)?;
        drop(store);
        // The same key as the pre-call lookup, so the reply names the join
        // *this* capability authored. Unkeyed, a rejoin would report the
        // superseded pre-removal event — an `event_id` belonging to a
        // membership the room already terminated, beside a `standing` of
        // `active` the fold correctly derives from the new one.
        let (event_id, pos) = ticket
            .as_ref()
            .and_then(|t| self.membership_event(&room_id, Some(&t.invite_id)))
            .or_else(|| self.membership_event(&room_id, None))
            .ok_or_else(|| ApiError::MembershipUnresolved {
                room_id: api_room.clone(),
                subject_id: subject_id.clone(),
            })?;
        Ok(InviteRedeemOut {
            room_id: api_room,
            subject_id,
            role: proj::role(member.role),
            standing: departures.standing_of(member.status, &member.identity),
            event_id,
            pos,
            // `false` when a join authored by *this* capability already existed
            // before the call reached the runtime — a replay. A rejoin after a
            // departure carries a different capability and so reports `true`.
            joined: joined_before.is_none(),
        })
    }

    /// `invite.list` — fold the accepted invitation and redemption facts into
    /// an authority-only typed index. Capability secrets and hashes never
    /// enter the returned rows.
    pub fn invite_list(
        &self,
        ctx: &RoomContext,
        req: &InviteList,
    ) -> Result<InviteListOut, ApiError> {
        let room_id = ctx.room_id;
        let store = self
            .sup
            .open_store()
            .map_err(|_| ApiError::InviteIndexUnreadable)?;
        let stored = store
            .room_tail(&room_id, u32::MAX)
            .map_err(|_| ApiError::InviteIndexUnreadable)?;

        // Decode the two membership facts that define this index once. A
        // corrupt/unrepresentable row makes the index unreadable; silently
        // skipping it would present a partial answer as complete.
        let mut joins = Vec::new();
        for se in stored
            .iter()
            .filter(|se| se.event_type == EventType::MemberJoined)
        {
            let event = SignedEvent::decode(&se.wire.signed)
                .map_err(|_| ApiError::InviteIndexUnreadable)?;
            let Content::MemberJoined(join) = event.content else {
                return Err(ApiError::InviteIndexUnreadable);
            };
            joins.push((event.sender_id, join));
        }

        let now = crate::now_ms();
        let mut rows = Vec::new();
        for se in stored
            .iter()
            .filter(|se| se.event_type == EventType::MemberInvited)
        {
            let event = SignedEvent::decode(&se.wire.signed)
                .map_err(|_| ApiError::InviteIndexUnreadable)?;
            let Content::MemberInvited(invite) = event.content else {
                return Err(ApiError::InviteIndexUnreadable);
            };
            let expires_ms = invite.expires_at.ok_or(ApiError::InviteIndexUnreadable)?;
            // The same millisecond-fidelity conversion `invite.mint` serves,
            // so the two answers about one invite's expiry cannot disagree.
            let expires_at = proj::ts_millis(expires_ms).ok_or(ApiError::InviteIndexUnreadable)?;
            let role = match invite.role.as_str() {
                "admin" => Role::Authority,
                "member" | "agent" => Role::Member,
                _ => return Err(ApiError::InviteIndexUnreadable),
            };
            let redeemed = joins.iter().any(|(sender, join)| {
                join.via_invite_id == invite.invite_id
                    && *sender == invite.invitee_key
                    && join.device_binding.identity_key == invite.invitee_key
                    && join.role == invite.role
                    && capability_hash(&room_id, &join.via_invite_id, &join.capability_secret)
                        == invite.capability_hash
            });
            let redeemability = if redeemed {
                Redeemability::Redeemed
            } else if now > expires_ms {
                Redeemability::Expired
            } else {
                Redeemability::Outstanding
            };
            rows.push(InviteRow {
                invite_id: proj::invite_id(&invite.invite_id),
                subject_id: SubjectId::new(invite.invitee_key.to_string()),
                role,
                expires_at,
                redeemability,
            });
        }
        let (invites, truncated) = page_indexed(rows, Window::resolve(&req.page)?);
        Ok(InviteListOut {
            room_id: req.room_id.clone(),
            invites,
            truncated,
        })
    }

    /// `invite.revoke` — withdraw an outstanding capability before expiry.
    ///
    /// **Not implemented in this build.** The active Iroh Room SDK has no
    /// convergent signed invite-revocation event, and a local store tombstone
    /// would be a withdrawal no peer converges on — a fact this daemon would
    /// assert and no other member could see. It is refused rather than faked;
    /// see the acceptance matrix on #165 for the upstream blocker.
    pub async fn invite_revoke(
        &self,
        _ctx: &RoomContext,
        req: &InviteRevoke,
    ) -> Result<InviteRevokeOut, ApiError> {
        let _ = req;
        Err(ApiError::NotReady)
    }

    // ------------------------------------------------------------------
    // Timeline
    // ------------------------------------------------------------------

    /// `message.send` — author a message.
    ///
    /// `at` is the instant the event's author **signed**, read back from the
    /// committed event, not the wall clock at reply time: the record defines
    /// `<ts>` on a committed fact as the non-repudiable author date, and the
    /// two are not the same number.
    pub async fn message_send(
        &self,
        ctx: &RoomContext,
        req: &MessageSend,
    ) -> Result<MessageSendOut, ApiError> {
        let limit = limits().max_message_body_bytes;
        let body_len = req.body.len() as u64;
        if body_len > limit {
            return Err(ApiError::MessageTooLarge {
                declared_bytes: body_len,
                limit_bytes: limit,
            });
        }
        if req.body.is_empty() {
            // An empty body carries no message. The record states no minimum,
            // so this is the step-1 bound it is, never `message_too_large`.
            return Err(ApiError::InvalidArgument {
                field: "in.body".into(),
                reason: InvalidReason::Bound { min: 1, max: limit },
            });
        }
        let event_id_hex = self
            .sup
            .send_message(req.room_id.as_ref(), &req.body)
            .await
            .map_err(|error| core_to_api_room(error, &req.room_id))?;
        let (pos, at) = self.committed_pos_and_instant(&ctx.room_id, &event_id_hex)?;
        Ok(MessageSendOut {
            room_id: req.room_id.clone(),
            event_id: EventId::new(event_id_hex),
            pos,
            at,
        })
    }

    /// `status.post` — author an agent status. Open to any active member:
    /// member and agent are a classification, not a permission.
    pub async fn status_post(
        &self,
        ctx: &RoomContext,
        req: &StatusPost,
    ) -> Result<StatusPostOut, ApiError> {
        let label = status_label_wire(req.label);
        let progress_pct = match req.progress {
            Progress::Reported { percent } => Some(u64::from(percent)),
            Progress::Absent => None,
        };
        let event_id_hex = self
            .sup
            .post_status(req.room_id.as_ref(), label, None, progress_pct, &[])
            .await
            .map_err(|error| core_to_api_room(error, &req.room_id))?;
        let (pos, at) = self.committed_pos_and_instant(&ctx.room_id, &event_id_hex)?;
        Ok(StatusPostOut {
            room_id: req.room_id.clone(),
            event_id: EventId::new(event_id_hex),
            pos,
            at,
            severity: req.label.severity(),
        })
    }

    /// `status.history` — read one subject's status history, one entry per
    /// real posted event, chronological. The daemon MUST NOT interpolate,
    /// smooth, or fabricate a point.
    ///
    /// An entry is served **iff** its event is a committed one: an
    /// out-of-vocabulary label, an out-of-range progress percent, or an
    /// unrepresentable instant leaves the row uncommitted, and it is omitted
    /// here exactly as it is from `room.timeline`. One rule, both projections
    /// — a row that is a status entry in one and not an event in the other
    /// would be two answers to one question.
    pub fn status_history(
        &self,
        ctx: &RoomContext,
        req: &StatusHistory,
    ) -> Result<StatusHistoryOut, ApiError> {
        let identity = Self::parse_subject(&req.subject_id).map_err(core_to_api)?;
        let store = self.sup.open_store().map_err(core_to_api)?;
        let rows = store
            .room_tail(&ctx.room_id, u32::MAX)
            .map_err(|_| ApiError::RoomIndexUnreadable)?;
        let mut entries = Vec::new();
        let mut authored_any = false;
        for se in &rows {
            if se.event_type != EventType::AgentStatus {
                continue;
            }
            let Ok(ev) = SignedEvent::decode(&se.wire.signed) else {
                continue;
            };
            if ev.sender_id != identity {
                continue;
            }
            authored_any = true;
            let Content::AgentStatus(c) = ev.content else {
                continue;
            };
            let (Ok(label), Some(at), Some(progress)) = (
                status_label_parse(&c.status),
                proj::ts(ev.created_at),
                proj::progress(c.progress_pct),
            ) else {
                continue; // not a committed event; not a status entry either
            };
            entries.push(StatusEntry {
                at,
                label,
                severity: label.severity(),
                progress,
            });
        }
        if !authored_any {
            return Err(ApiError::StatusSubjectUnknown {
                room_id: req.room_id.clone(),
                subject_id: req.subject_id.clone(),
            });
        }
        let (page, truncated) = page_indexed(entries, Window::resolve(&req.page)?);
        Ok(StatusHistoryOut {
            room_id: req.room_id.clone(),
            subject_id: req.subject_id.clone(),
            entries: page,
            truncated,
        })
    }

    /// `fleet.list` — the agent fleet projection, no tallies. Scope is the
    /// caller's authorized room set; a room the caller cannot see contributes
    /// nothing and its absence is indistinguishable from it not existing.
    pub async fn fleet_list(&self) -> Result<FleetListOut, ApiError> {
        let now = crate::now_ms();
        let self_id = self.sup.local_identity_key().map_err(core_to_api)?;
        let known: BTreeSet<String> = crate::localstate::load(self.sup.data_dir())
            .map_err(|_| ApiError::FleetProjectionUnavailable)?
            .rooms
            .keys()
            .cloned()
            .collect();
        let scans: Vec<IrohRoomId> = if self.sup.db_path().exists() {
            known.iter().filter_map(|s| s.parse().ok()).collect()
        } else {
            Vec::new()
        };
        // Per (subject, room) aggregation of the newest status + liveness.
        let mut agents: FleetAggMap = BTreeMap::new();
        for room_id in scans {
            let Ok(snapshot) = self.sup.snapshot_for(&room_id).await else {
                continue;
            };
            if RoomSupervisor::require_local_room_access(&snapshot, &self_id).is_err() {
                continue;
            }
            let rows = {
                let store = self
                    .sup
                    .open_store()
                    .map_err(|_| ApiError::FleetProjectionUnavailable)?;
                store
                    .room_tail(&room_id, u32::MAX)
                    .map_err(|_| ApiError::FleetProjectionUnavailable)?
            };
            // **Agent-ness is derived, not declared.** The record: "An agent is
            // a member that has authored at least one `status.post` event.
            // Agent-ness is derived here, not declared: it is a
            // classification, not a permission, so it is not a `role` and
            // appears in no membership row."
            //
            // An earlier revision read the upstream fold's `Role::Agent`
            // instead, which is a membership row and gets the answer wrong in
            // both directions: a member that posts statuses never appeared in
            // the fleet, and one that holds the upstream role but has never
            // posted appeared with `latest_status: absent` — an agent the
            // operator has no evidence for.
            //
            // Membership is still required: the subject must have joined, which
            // is the same device-binding test the roster uses.
            let agent_ids: BTreeSet<iroh_rooms::identity::IdentityKey> = rows
                .iter()
                .filter_map(|se| SignedEvent::decode(&se.wire.signed).ok())
                .filter(|ev| matches!(ev.content, Content::AgentStatus(_)))
                .map(|ev| ev.sender_id)
                .filter(|id| {
                    snapshot
                        .member(id)
                        .is_some_and(|member| member.device.is_some())
                })
                .collect();
            if agent_ids.is_empty() {
                continue;
            }
            let mut signals: AgentSignalsMap = BTreeMap::new();
            for se in &rows {
                let Ok(ev) = SignedEvent::decode(&se.wire.signed) else {
                    continue;
                };
                if let Content::MemberJoined(c) = &ev.content {
                    if agent_ids.contains(&c.device_binding.identity_key) {
                        signals
                            .entry(c.device_binding.identity_key)
                            .or_default()
                            .0
                            .insert(c.device_binding.device_key);
                    }
                }
                if !agent_ids.contains(&ev.sender_id) {
                    continue;
                }
                let sig = signals.entry(ev.sender_id).or_default();
                sig.0.insert(ev.device_id);
                sig.2 = Some(sig.2.map_or(ev.created_at, |t: u64| t.max(ev.created_at)));
                if let Content::AgentStatus(c) = &ev.content {
                    let newer = match &sig.1 {
                        Some((_, ts)) => ev.created_at >= *ts,
                        None => true,
                    };
                    if newer {
                        sig.1 = Some((c.status.clone(), ev.created_at));
                    }
                }
            }
            let session = self.sup.session_opt(&room_id);
            for identity in &agent_ids {
                let (devices, latest, last_seen) = signals.remove(identity).unwrap_or_default();
                let connected = session.as_deref().is_some_and(|s| {
                    devices.iter().any(|dev| {
                        crate::supervisor::endpoint_id_of(*dev).is_ok_and(|id| {
                            s.node.peer_state(id)
                                == Some(iroh_rooms::experimental::session::PeerConnState::Connected)
                        })
                    })
                });
                let liveness = crate::fleet::derive_liveness(
                    connected,
                    latest.as_ref().map(|(l, ts)| (l.as_str(), *ts)),
                    now,
                );
                agents.insert(
                    (identity.to_string(), room_id.to_string()),
                    (proj::liveness(liveness), latest, last_seen),
                );
            }
        }
        let agents = agents
            .into_iter()
            .map(
                |((subject, room), (liveness, latest, last_seen))| FleetRow {
                    subject_id: SubjectId::new(subject),
                    room_id: RoomId::new(room),
                    liveness,
                    latest_status: proj::latest_status(
                        latest.as_ref().map(|(l, ts)| (l.as_str(), *ts)),
                    ),
                    last_seen: proj::last_seen(last_seen),
                },
            )
            .collect();
        Ok(FleetListOut { agents })
    }

    // ------------------------------------------------------------------
    // Files
    // ------------------------------------------------------------------

    /// `file.share` — share bytes into a room.
    ///
    /// **Not implemented on the typed WS surface in this build.** The record
    /// streams the bytes alongside the declaration, and that framing is not
    /// wired to the codec yet; the host-staged half below is what carries a
    /// real share today. It is refused rather than fabricating an event for
    /// bytes nobody sent.
    pub async fn file_share(
        &self,
        _ctx: &RoomContext,
        req: &FileShare,
    ) -> Result<FileShareOut, ApiError> {
        let _ = req;
        Err(ApiError::NotReady)
    }

    /// Host-side half of `file.share`: the host has already staged the bytes
    /// under the daemon data directory and supplies the typed declaration.
    /// The runtime result is converted directly to the protocol-v2 output;
    /// no JSON projection is built or decoded inside core.
    pub(crate) async fn share_staged_file(
        &self,
        req: &FileShare,
        path: &Path,
    ) -> Result<FileShareOut, ApiError> {
        let actual_bytes = std::fs::metadata(path)
            .map_err(|_| ApiError::FileIndexUnreadable)?
            .len();
        if actual_bytes != req.declared_bytes {
            // A stream that does not match its declaration is a size
            // disagreement, never `digest_mismatch` — accusing an honest peer
            // of corruption for a size mismatch is the false accusation the
            // size policy forbids.
            return Err(ApiError::DeclaredSizeMismatch {
                declared_bytes: req.declared_bytes,
                observed_bytes: actual_bytes,
            });
        }
        let limit = limits().max_shared_file_bytes;
        if actual_bytes > limit {
            return Err(ApiError::FileTooLarge {
                declared_bytes: actual_bytes,
                limit_bytes: limit,
                enforced_at: EnforcedAt::StageStream,
            });
        }
        let shared = self
            .sup
            .share_file(
                req.room_id.as_ref(),
                path.to_string_lossy().as_ref(),
                Some(&req.name),
                Some(&req.declared_content_type),
            )
            .await
            .map_err(|error| core_to_api_room(error, &req.room_id))?;
        let room_id = Self::parse_room(&req.room_id)?;
        let pos = self.pos_of_event(&room_id, &shared.event_id)?;
        Ok(FileShareOut {
            room_id: req.room_id.clone(),
            file_id: FileId::new(shared.file_id),
            event_id: EventId::new(shared.event_id),
            pos,
            bytes: shared.bytes,
            digest: shared.digest,
        })
    }

    /// `file.list` — files shared into a room, provider availability as a
    /// protocol fact rather than an inference from membership display state.
    pub async fn file_list(
        &self,
        ctx: &RoomContext,
        req: &FileList,
    ) -> Result<FileListOut, ApiError> {
        let room_id = ctx.room_id;
        let snapshot = &ctx.snapshot;
        let store = self
            .sup
            .open_store()
            .map_err(|_| ApiError::FileIndexUnreadable)?;
        let events = store
            .by_type(&room_id, EventType::FileShared)
            .map_err(|_| ApiError::FileIndexUnreadable)?;
        let session = self.sup.session_opt(&room_id);
        let peer_paths: std::collections::HashMap<_, _> = if let Some(session) = session.as_deref()
        {
            session
                .node
                .peer_paths()
                .await
                .into_iter()
                .map(|(device, path, _relay)| (device, path.label()))
                .collect()
        } else {
            std::collections::HashMap::new()
        };
        let peer_entries: std::collections::HashMap<_, _> = session
            .as_deref()
            .map(|session| session.node.peer_entries().into_iter().collect())
            .unwrap_or_default();
        let room_id_str = room_id.to_string();
        let mut files = Vec::with_capacity(events.len());
        for se in &events {
            let Ok(ev) = SignedEvent::decode(&se.wire.signed) else {
                continue;
            };
            let Content::FileShared(f) = ev.content else {
                continue;
            };
            let providers: Vec<iroh_rooms::identity::DeviceKey> = match &f.providers {
                Some(list) if !list.is_empty() => list.clone(),
                _ => vec![ev.device_id],
            };
            let fetchable = providers.iter().any(|provider| {
                crate::supervisor::endpoint_id_of(*provider)
                    .ok()
                    .and_then(|endpoint| peer_entries.get(&endpoint))
                    .is_some_and(|entry| {
                        entry.state == iroh_rooms::experimental::session::PeerConnState::Connected
                    })
            });
            let file_id = file_handle(&f.file_id);
            let local_device = snapshot
                .member(&ctx.self_key)
                .and_then(|member| member.device);
            let self_hosted = local_device.is_some_and(|device| providers.contains(&device))
                || crate::localstate::fetched_file(self.sup.data_dir(), &room_id_str, &file_id)
                    .is_some();
            let provider_rows = provider_rows(&providers, snapshot, &peer_entries, &peer_paths);
            // `shared_at` is the sharer's signed instant. A row whose instant
            // will not convert is an unreadable index entry, not a row dated
            // to the epoch.
            let shared_at = proj::ts(ev.created_at).ok_or(ApiError::FileIndexUnreadable)?;
            files.push(FileRow {
                file_id: FileId::new(file_id),
                name: f.name.clone(),
                bytes: f.size_bytes,
                digest: f.blob_hash.to_string(),
                declared_content_type: f.mime_type.clone(),
                shared_by: SubjectId::new(ev.sender_id.to_string()),
                shared_at,
                providers: provider_rows,
                fetchable,
                self_hosted,
            });
        }
        // Paging over the file rows (position = index within the file index).
        let (page, truncated) = page_indexed(files, Window::resolve(&req.page)?);
        Ok(FileListOut {
            room_id: req.room_id.clone(),
            files: page,
            truncated,
        })
    }

    /// `file.fetch` — fetch a file's bytes from a provider; the daemon holds
    /// the bytes and `file.read` streams them out. Requires liveness.
    ///
    /// `provider_unreachable` carries the **attempted** provider rows — the
    /// same `{subject_id, device_id, link}` evidence `file.list` serves — so a
    /// client can say *why* each attempt failed. An error whose typed field is
    /// an empty array is prose wearing a schema's clothes.
    pub async fn file_fetch(
        &self,
        ctx: &RoomContext,
        req: &FileFetch,
    ) -> Result<FileFetchOut, ApiError> {
        let room_id = ctx.room_id;
        let file_id = Self::parse_file(&req.file_id).map_err(|_| ApiError::FileUnknown {
            file_id: req.file_id.clone(),
        })?;
        // The signed share is read first: it is what names the digest to verify
        // against and the providers an unreachable fetch must report.
        let shared = {
            let store = self
                .sup
                .open_store()
                .map_err(|_| ApiError::FileIndexUnreadable)?;
            store
                .by_type(&room_id, EventType::FileShared)
                .map_err(|_| ApiError::FileIndexUnreadable)?
                .iter()
                .filter_map(|se| {
                    let ev = SignedEvent::decode(&se.wire.signed).ok()?;
                    match ev.content {
                        Content::FileShared(f) if f.file_id == file_id => Some((f, ev.device_id)),
                        _ => None,
                    }
                })
                .next()
        };
        let Some((shared, author_device)) = shared else {
            return Err(ApiError::FileUnknown {
                file_id: req.file_id.clone(),
            });
        };
        let digest = shared.blob_hash.to_string();

        let result = match self
            .sup
            .fetch_file(req.room_id.as_ref(), req.file_id.as_str(), None)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                return Err(self
                    .fetch_error(ctx, req, error, &shared, author_device, &digest)
                    .await)
            }
        };
        // A provider device the fold cannot bind to a subject leaves the reply
        // with no truthful `provider.subject_id`. It is a fold this daemon
        // could not complete, not a named subject whose membership is
        // unresolved: `<subject_id>` and `<device_id>` are distinct opaque
        // domains, so putting the device key in a subject field would hand the
        // client an identifier that resolves in neither.
        let provider_subject = ctx
            .snapshot
            .identity_of_device(&result.provider_device)
            .ok_or(ApiError::RoomIndexUnreadable)?;
        Ok(FileFetchOut {
            room_id: req.room_id.clone(),
            file_id: req.file_id.clone(),
            bytes: result.bytes,
            digest,
            provider: ProviderRef {
                subject_id: SubjectId::new(provider_subject.to_string()),
                device_id: DeviceId::new(result.provider_device.to_string()),
            },
        })
    }

    /// `file.read` — stream locally held bytes out (the header; bytes follow).
    ///
    /// "No such file in this room" and "its bytes are not held here" are two
    /// different facts with two different codes. The supervisor answers both
    /// with one internal kind, so the signed share is resolved here first:
    /// answering `file_not_fetched` for a file that was never shared would
    /// assert it exists and merely awaits a fetch.
    pub async fn file_read(
        &self,
        ctx: &RoomContext,
        req: &FileRead,
    ) -> Result<FileReadOut, ApiError> {
        let unknown = || ApiError::FileUnknown {
            file_id: req.file_id.clone(),
        };
        let file_id = Self::parse_file(&req.file_id).map_err(|_| unknown())?;
        {
            let store = self
                .sup
                .open_store()
                .map_err(|_| ApiError::FileIndexUnreadable)?;
            let shared = store
                .by_type(&ctx.room_id, EventType::FileShared)
                .map_err(|_| ApiError::FileIndexUnreadable)?
                .iter()
                .filter_map(|se| SignedEvent::decode(&se.wire.signed).ok())
                .any(|ev| matches!(ev.content, Content::FileShared(f) if f.file_id == file_id));
            if !shared {
                return Err(unknown());
            }
        }
        let local = self
            .sup
            .local_file(req.room_id.as_ref(), req.file_id.as_str())
            .await
            .map_err(|error| match error.kind {
                // The share exists, so the only remaining cause is that this
                // daemon holds no bytes for it.
                ErrorKind::FileUnavailable => ApiError::FileNotFetched {
                    file_id: req.file_id.clone(),
                },
                _ => core_to_api_room(error, &req.room_id),
            })?;
        Ok(FileReadOut {
            room_id: req.room_id.clone(),
            file_id: req.file_id.clone(),
            bytes: local.bytes,
            declared_content_type: local.mime,
        })
    }

    /// `transfer.cancel` — cancel a transfer by the `op_id` that started it.
    ///
    /// This build tracks no transfers by `op_id` (that needs upstream progress
    /// and cancellation handles), so **no** `transfer_op_id` names an in-flight
    /// transfer for this principal — which is exactly what
    /// `transfer_unknown { transfer_op_id }` states, and it is the operation's
    /// own distinctive code. It is a true statement about this daemon rather
    /// than a stand-in code: a `cancelled` outcome would claim an effect that
    /// did not happen, and the record's non-oracle rule already makes
    /// `transfer_unknown` the answer for a transfer the caller may not see.
    pub async fn transfer_cancel(
        &self,
        req: &TransferCancel,
    ) -> Result<TransferCancelOut, ApiError> {
        Err(ApiError::TransferUnknown {
            transfer_op_id: req.transfer_op_id.clone(),
        })
    }

    // ------------------------------------------------------------------
    // Pipes
    // ------------------------------------------------------------------

    /// `pipe.publish` — publish a pipe to a loopback target. `audience: room`
    /// authorizes every active member; `audience: subjects` authorizes the
    /// named list. A non-loopback target or an out-of-range port is
    /// `pipe_target_refused` carrying the rejected target verbatim.
    pub async fn pipe_publish(
        &self,
        ctx: &RoomContext,
        req: &PipePublish,
    ) -> Result<PipePublishOut, ApiError> {
        let room_id = ctx.room_id;
        let snapshot = &ctx.snapshot;
        // The target must be loopback (IPv4 127.0.0.0/8 or IPv6 ::1) with a
        // port in 1..=65535. Anything else is pipe_target_refused carrying the
        // rejected target verbatim, never the generic policy_refused.
        // Parse the address, bracketing an IPv6 host so `::1` resolves (the
        // bare `host:port` form is ambiguous for a bare IPv6 literal).
        let host = if req.target.host.contains(':') && !req.target.host.starts_with('[') {
            format!("[{}]", req.target.host)
        } else {
            req.target.host.clone()
        };
        let target_addr: Option<std::net::SocketAddr> =
            format!("{host}:{}", req.target.port).parse().ok();
        let port_ok = (1..=65535).contains(&req.target.port);
        let loopback = target_addr.as_ref().is_some_and(is_loopback_addr);
        if !port_ok || !loopback {
            return Err(ApiError::PipeTargetRefused {
                target: req.target.clone(),
            });
        }
        let target_addr = target_addr.expect("parsed above");

        // Resolve the audience to a concrete authorized-subject set.
        let allowed: Vec<iroh_rooms::identity::IdentityKey> = match &req.audience {
            Audience::Room => snapshot.active_members().map(|m| m.identity).collect(),
            Audience::Subjects { subject_ids } => subject_ids
                .iter()
                .map(Self::parse_subject)
                .collect::<CoreResult<Vec<_>>>()
                .map_err(core_to_api)?,
        };
        if allowed.is_empty() {
            return Err(ApiError::PolicyRefused {
                room_id: req.room_id.clone(),
            });
        }

        let target_hint = target_addr.to_string();
        let (pipe_id, event_id) = self
            .sup
            .pipe_expose_multi(&room_id, target_addr, &target_hint, &allowed)
            .await
            .map_err(|error| core_to_api_room(error, &req.room_id))?;
        let pos = self.pos_of_event(&room_id, &event_id)?;
        Ok(PipePublishOut {
            room_id: req.room_id.clone(),
            pipe_id: crate::projection::pipe_id(&pipe_id),
            target: req.target.clone(),
            audience: req.audience.clone(),
            event_id: EventId::new(event_id),
            pos,
        })
    }

    /// `pipe.list` — pipes in a room, with publisher reachability and local
    /// connection as two separately named facts.
    pub async fn pipe_list(
        &self,
        ctx: &RoomContext,
        req: &PipeList,
    ) -> Result<PipeListOut, ApiError> {
        let room_id = ctx.room_id;
        let store = self
            .sup
            .open_store()
            .map_err(|_| ApiError::PipeIndexUnreadable)?;
        let session = self.sup.session_opt(&room_id);
        let local_identity = Some(ctx.self_key);
        let peer_paths: std::collections::HashMap<_, _> = if let Some(session) = session.as_deref()
        {
            session
                .node
                .peer_paths()
                .await
                .into_iter()
                .map(|(device, path, _relay)| (device, path.label()))
                .collect()
        } else {
            std::collections::HashMap::new()
        };
        let peer_entries: std::collections::HashMap<_, _> = session
            .as_deref()
            .map(|session| session.node.peer_entries().into_iter().collect())
            .unwrap_or_default();
        let closed =
            closed_pipe_ids(&store, &room_id).map_err(|_| ApiError::PipeIndexUnreadable)?;
        let opened = store
            .by_type(&room_id, EventType::PipeOpened)
            .map_err(|_| ApiError::PipeIndexUnreadable)?;
        let mut pipes = Vec::new();
        for se in opened {
            if se.lamport.is_none() {
                continue;
            }
            let Ok(ev) = SignedEvent::decode(&se.wire.signed) else {
                continue;
            };
            let Content::PipeOpened(p) = ev.content else {
                continue;
            };
            if closed.contains(&p.pipe_id) {
                continue; // revoked pipes are not listed
            }
            // The publisher's signed instant. A pipe whose announcement will not
            // convert to a wire instant is an unreadable index row, never one
            // dated to the epoch.
            let published_at = proj::ts(ev.created_at).ok_or(ApiError::PipeIndexUnreadable)?;
            let connected = self
                .sup
                .pipe_connection_open(&room_id, p.pipe_id, &p.owner_id);
            let owner_endpoint = crate::supervisor::endpoint_id_of(p.owner_endpoint).ok();
            let link = if local_identity.as_ref() == Some(&p.owner_id) && session.is_some() {
                Link::Direct {
                    since: published_at,
                }
            } else {
                owner_endpoint
                    .as_ref()
                    .and_then(|endpoint| peer_entries.get(endpoint))
                    .map_or(
                        Link::NotConnected {
                            reason: LinkReason::NeverDialed,
                        },
                        |entry| {
                            peer_link(
                                entry,
                                owner_endpoint
                                    .as_ref()
                                    .and_then(|endpoint| peer_paths.get(endpoint).copied()),
                            )
                        },
                    )
            };
            pipes.push((
                se.lamport.unwrap(),
                PipeRow {
                    pipe_id: proj::pipe_id(&p.pipe_id),
                    published_by: SubjectId::new(p.owner_id.to_string()),
                    device_id: DeviceId::new(p.owner_endpoint.to_string()),
                    published_at,
                    link,
                    connected,
                },
            ));
        }
        pipes.sort_by_key(|(pos, _)| *pos);
        let all: Vec<PipeRow> = pipes.into_iter().map(|(_, p)| p).collect();
        let (page, truncated) = page_indexed(all, Window::resolve(&req.page)?);
        Ok(PipeListOut {
            room_id: req.room_id.clone(),
            pipes: page,
            truncated,
        })
    }

    /// `pipe.connect` — connect to a pipe. Requires liveness. A caller outside
    /// the pipe's audience answers `pipe_unknown`, indistinguishable from no
    /// such pipe.
    ///
    /// A caller *inside* the audience is told the difference, because for them
    /// it is not a disclosure: a pipe its publisher withdrew answers
    /// `pipe_revoked` with the withdrawal's signed instant, so a client stops
    /// retrying a tunnel that is never coming back rather than reading a
    /// permanent withdrawal as a not-yet-synced announcement.
    pub async fn pipe_connect(
        &self,
        ctx: &RoomContext,
        req: &PipeConnect,
    ) -> Result<PipeConnectOut, ApiError> {
        let local_addr = match self
            .sup
            .pipe_connect(req.room_id.as_ref(), req.pipe_id.as_str())
            .await
        {
            Ok(addr) => addr,
            Err(error) => return Err(self.pipe_connect_error(ctx, req, error).await),
        };
        // The local endpoint is one this daemon just bound, so it parses as a
        // socket address by construction; a value that does not is a runtime
        // inconsistency, not a `port: 0` to be served as though real.
        let local = parse_target(&local_addr).ok_or_else(|| ApiError::PipeUnreachable {
            pipe_id: req.pipe_id.clone(),
            link: Link::NotConnected {
                reason: LinkReason::NoRoute,
            },
        })?;
        Ok(PipeConnectOut {
            room_id: req.room_id.clone(),
            pipe_id: req.pipe_id.clone(),
            connection_id: local_addr,
            local,
        })
    }

    /// `pipe.release` — release a local connection, named by the connection.
    pub async fn pipe_release(&self, req: &PipeRelease) -> Result<PipeReleaseOut, ApiError> {
        if !self.sup.pipe_release(&req.connection_id) {
            return Err(ApiError::ConnectionUnknown {
                connection_id: req.connection_id.clone(),
            });
        }
        Ok(PipeReleaseOut {
            connection_id: req.connection_id.clone(),
            released: true,
        })
    }

    /// `pipe.revoke` — withdraw a published pipe as a signed fact.
    ///
    /// Restricted to the pipe's **publisher**, which is a narrower relation
    /// than role: an authority that did not publish a pipe cannot revoke it
    /// either, and that refusal is `pipe_not_publisher`, not
    /// `insufficient_standing`. The runtime enforces exactly that relation —
    /// it admitted the room's administrator until this was corrected, which let
    /// a role bypass a relation the record deliberately made narrower than any
    /// role. `revoked_at` is the withdrawal event's signed instant, not the
    /// wall clock at reply time.
    pub async fn pipe_revoke(
        &self,
        ctx: &RoomContext,
        req: &PipeRevoke,
    ) -> Result<PipeRevokeOut, ApiError> {
        let event_id = self
            .sup
            .pipe_close(req.room_id.as_ref(), req.pipe_id.as_str())
            .await
            .map_err(|error| match error.kind {
                ErrorKind::PipeDenied => ApiError::PipeNotPublisher {
                    pipe_id: req.pipe_id.clone(),
                },
                ErrorKind::InvalidParams => ApiError::PipeUnknown {
                    pipe_id: req.pipe_id.clone(),
                },
                _ => core_to_api_room(error, &req.room_id),
            })?;
        let (pos, revoked_at) = self.committed_pos_and_instant(&ctx.room_id, &event_id)?;
        Ok(PipeRevokeOut {
            room_id: req.room_id.clone(),
            pipe_id: req.pipe_id.clone(),
            event_id: EventId::new(event_id),
            pos,
            revoked_at,
        })
    }

    // ------------------------------------------------------------------
    // Shared helpers
    // ------------------------------------------------------------------

    /// All committed timeline events for a room, ascending by position —
    /// the dense rank over the canonical `(lamport, event_id)` order.
    fn committed_events(&self, ctx: &RoomContext) -> Result<Vec<Event>, ApiError> {
        let store = self.sup.open_store().map_err(core_to_api)?;
        let rows = store
            .room_tail(&ctx.room_id, u32::MAX)
            .map_err(|_| ApiError::RoomIndexUnreadable)?;
        let refs: Vec<&iroh_rooms::experimental::store::StoredEvent> = rows.iter().collect();
        Ok(proj::positioned(&refs, &ctx.snapshot, &ctx.departures)
            .into_iter()
            .map(|(_, e)| e)
            .collect())
    }

    /// The dense canonical position **and the author-signed instant** of one
    /// committed event, looked up by its event id.
    ///
    /// Mutation replies use this so the `pos` they serve is the same rank the
    /// timeline, resync, and push stream serve for that event — never the raw
    /// lamport, and never the head's position (which a later concurrent event
    /// can share) — and so the `at` / `revoked_at` they serve is the instant the
    /// author signed rather than the wall clock at reply time. Ranking counts
    /// only committed kinds (see [`proj::positioned`]), so a non-committed row
    /// consumes no position.
    fn committed_pos_and_instant(
        &self,
        room_id: &IrohRoomId,
        event_id_hex: &str,
    ) -> Result<(u64, Timestamp), ApiError> {
        let store = self.sup.open_store().map_err(core_to_api)?;
        let rows = store
            .room_tail(room_id, u32::MAX)
            .map_err(|_| ApiError::RoomIndexUnreadable)?;
        let mut rank = 0u64;
        for se in &rows {
            if !proj::is_committed(se) {
                continue;
            }
            if proj::event_id(&se.event_id).as_str() == event_id_hex {
                // `is_committed` already proved both halves convert.
                let at = SignedEvent::decode(&se.wire.signed)
                    .ok()
                    .and_then(|ev| proj::ts(ev.created_at))
                    .ok_or(ApiError::RoomIndexUnreadable)?;
                return Ok((rank, at));
            }
            rank += 1;
        }
        Err(ApiError::RoomIndexUnreadable)
    }

    /// [`Self::committed_pos_and_instant`] when only the position is wanted.
    fn pos_of_event(&self, room_id: &IrohRoomId, event_id_hex: &str) -> Result<u64, ApiError> {
        self.committed_pos_and_instant(room_id, event_id_hex)
            .map(|(pos, _)| pos)
    }

    /// The local subject's `member_joined` event in a room, with its dense
    /// canonical position — the event `invite.redeem` reports. Looking for the
    /// *membership* event specifically (not merely the newest event this
    /// subject authored) is what lets `joined` tell a fresh join from a replay.
    ///
    /// The join is identified by **the capability that authored it**, not by
    /// "any join by me". A subject that was removed and later redeemed a new
    /// capability has two `member_joined` rows, and "have I ever joined this
    /// room" cannot tell that rejoin from a replay: it would report the fresh
    /// membership as `joined: false` and name the superseded, pre-removal event
    /// — a reply whose `standing` says `active` while its `event_id` belongs to
    /// a membership the room already terminated. Matching `via_invite_id`
    /// against the ticket keeps a replay of one capability answering from its
    /// own original join, which is what the record's idempotence requires.
    ///
    /// `via_invite_id` of `None` keeps the unkeyed "any join by me" behaviour,
    /// for the callers that hold no ticket.
    fn membership_event(
        &self,
        room_id: &IrohRoomId,
        via_invite_id: Option<&[u8; SHORT_ID_LEN]>,
    ) -> Option<(EventId, u64)> {
        let self_key = self.sup.local_identity_key().ok()?;
        let store = self.sup.open_store().ok()?;
        let rows = store.room_tail(room_id, u32::MAX).ok()?;
        let mut rank = 0u64;
        for se in &rows {
            if !proj::is_committed(se) {
                continue;
            }
            if let Ok(ev) = SignedEvent::decode(&se.wire.signed) {
                let is_join = match &ev.content {
                    Content::MemberJoined(c) => {
                        c.device_binding.identity_key == self_key
                            && via_invite_id.is_none_or(|id| &c.via_invite_id == id)
                    }
                    // The authority never authors a join: its membership is the
                    // genesis it signed, which no capability authorized.
                    Content::RoomCreated(_) => via_invite_id.is_none() && ev.sender_id == self_key,
                    _ => false,
                };
                if is_join {
                    return Some((proj::event_id(&se.event_id), rank));
                }
            }
            rank += 1;
        }
        None
    }

    /// The author-dated instant a subject joined, when discoverable. `None`
    /// when this daemon holds no dating evidence — the caller answers
    /// `membership_unresolved` rather than dating the row to the epoch.
    fn joined_at(
        &self,
        room_id: &IrohRoomId,
        subject: &iroh_rooms::identity::IdentityKey,
    ) -> Option<Timestamp> {
        let store = self.sup.open_store().ok()?;
        let rows = store.by_type(room_id, EventType::MemberJoined).ok()?;
        for se in rows {
            let Ok(ev) = SignedEvent::decode(&se.wire.signed) else {
                continue;
            };
            if let Content::MemberJoined(c) = &ev.content {
                if &c.device_binding.identity_key == subject {
                    return proj::ts(ev.created_at);
                }
            }
        }
        // The authority authors no join: its membership is the genesis it
        // signed, so that event's instant is its `joined_at`.
        //
        // The genesis is found **by kind**, not as `room_tail(room_id, 1)`.
        // That call returns the newest row, not the origin, so on any room with
        // more than one event it dated the authority from whatever happened
        // last — an outstanding invitation was enough to move the roster's
        // `joined_at` forward on every read.
        let ev = genesis_event(&store, room_id)?;
        if &ev.sender_id == subject {
            return proj::ts(ev.created_at);
        }
        None
    }

    /// The exact refusal a failed `file.fetch` earns, with the **attempted**
    /// provider evidence the record's `provider_unreachable` requires.
    async fn fetch_error(
        &self,
        ctx: &RoomContext,
        req: &FileFetch,
        error: CoreError,
        shared: &iroh_rooms::files::FileShared,
        author_device: iroh_rooms::identity::DeviceKey,
        digest: &str,
    ) -> ApiError {
        match error.kind {
            ErrorKind::FileUnavailable | ErrorKind::FileUnauthorized => {
                // The record requires the **attempted** set, not the candidate
                // set: "a client can say *why* each one failed instead of only
                // that the fetch did". The runtime narrows the candidates the
                // same two ways before dialing — it drops a device whose
                // endpoint will not resolve, and it never dials itself — so
                // both filters are applied here or the error would name a
                // provider no attempt was ever made against.
                let self_device = self
                    .sup
                    .local_identity_key()
                    .ok()
                    .and_then(|identity| ctx.snapshot.member(&identity))
                    .and_then(|member| member.device)
                    .and_then(|device| crate::supervisor::endpoint_id_of(device).ok());
                let candidates: Vec<iroh_rooms::identity::DeviceKey> = match &shared.providers {
                    Some(list) if !list.is_empty() => list.clone(),
                    _ => vec![author_device],
                };
                let providers: Vec<iroh_rooms::identity::DeviceKey> = candidates
                    .into_iter()
                    .filter(|device| match crate::supervisor::endpoint_id_of(*device) {
                        Ok(endpoint) => Some(endpoint) != self_device,
                        Err(_) => false,
                    })
                    .collect();
                let (peer_entries, peer_paths) = self.peer_evidence(&ctx.room_id).await;
                let attempted =
                    provider_rows(&providers, &ctx.snapshot, &peer_entries, &peer_paths)
                        .into_iter()
                        .map(|row| AttemptedProvider {
                            subject_id: row.subject_id,
                            device_id: row.device_id,
                            link: row.link,
                        })
                        .collect();
                ApiError::ProviderUnreachable {
                    file_id: req.file_id.clone(),
                    providers: attempted,
                }
            }
            // Both halves are real: `expected` is the digest the share signed,
            // and `observed` is the digest the rejected bytes actually hash to,
            // recomputed by the supervisor from the bytes the upstream mismatch
            // arm hands back. An earlier revision served `observed: ""`, which
            // satisfied the schema and invented the fact. A mismatch this
            // daemon somehow cannot describe is not reported as a digest
            // comparison at all.
            ErrorKind::HashMismatch => match &error.detail {
                Some(observed) => ApiError::DigestMismatch {
                    expected: digest.to_owned(),
                    observed: observed.clone(),
                },
                None => ApiError::FileIndexUnreadable,
            },
            ErrorKind::RoomNotOpen => ApiError::RoomNotLive {
                room_id: req.room_id.clone(),
            },
            _ => core_to_api_room(error, &req.room_id),
        }
    }

    /// The exact refusal a failed `pipe.connect` earns, naming the pipe the
    /// caller asked for and the link this daemon actually observed.
    async fn pipe_connect_error(
        &self,
        ctx: &RoomContext,
        req: &PipeConnect,
        error: CoreError,
    ) -> ApiError {
        match error.kind {
            ErrorKind::PeerUnreachable => ApiError::PipeUnreachable {
                pipe_id: req.pipe_id.clone(),
                link: self.pipe_owner_link(ctx, &req.pipe_id).await,
            },
            ErrorKind::RoomNotOpen => ApiError::RoomNotLive {
                room_id: req.room_id.clone(),
            },
            // A deliberate withdrawal is `pipe_revoked`, and only for a caller
            // the publisher already admitted: the record lists it beside
            // `pipe_unknown` among `pipe.connect`'s errors precisely so a
            // client can stop retrying a tunnel that is never coming back.
            //
            // Entitlement is checked **first**, and the close event is only
            // resolved for a caller inside the announcement's audience. An
            // unconditional lookup would answer `pipe_revoked` to an outsider
            // and so confirm the pipe ever existed, which is the existence
            // oracle the record forbids — "a caller MUST NOT be able to
            // distinguish a pipe it is not entitled to from one that does not
            // exist". For everyone else "no such pipe" and "you are outside its
            // audience" remain one indistinguishable answer.
            ErrorKind::InvalidParams | ErrorKind::PipeDenied => {
                match self.revoked_pipe_instant(ctx, &req.pipe_id) {
                    Some(revoked_at) => ApiError::PipeRevoked {
                        pipe_id: req.pipe_id.clone(),
                        revoked_at,
                    },
                    None => ApiError::PipeUnknown {
                        pipe_id: req.pipe_id.clone(),
                    },
                }
            }
            _ => core_to_api_room(error, &req.room_id),
        }
    }

    /// The instant a withdrawn pipe was revoked — **only** for a caller the
    /// publisher authorized, and `None` for everyone and everything else.
    ///
    /// Three facts must all hold, in this order, or the answer is `None` and
    /// the caller keeps the indistinguishable `pipe_unknown`:
    ///
    /// 1. this daemon holds the governing `pipe.opened`;
    /// 2. the caller is its publisher or is named in `allowed_members` — the
    ///    audience resolved at publish time, which is what makes the disclosure
    ///    a fact the caller was already entitled to;
    /// 3. a `pipe.closed` for it is committed here.
    ///
    /// The instant is the one the close event's author **signed**, read exactly
    /// as [`Self::committed_pos_and_instant`] reads it, so the `revoked_at` a
    /// connector is told matches the `revoked_at` the publisher's own
    /// `pipe.revoke` reply reported for the same event. A wall clock read at
    /// reply time would name a different instant on every daemon.
    fn revoked_pipe_instant(&self, ctx: &RoomContext, pipe_id: &PipeId) -> Option<Timestamp> {
        let raw: [u8; SHORT_ID_LEN] = hex::decode(pipe_id.as_str()).ok()?.try_into().ok()?;
        let store = self.sup.open_store().ok()?;
        let opened = store
            .by_type(&ctx.room_id, EventType::PipeOpened)
            .ok()?
            .into_iter()
            .filter_map(|se| SignedEvent::decode(&se.wire.signed).ok())
            .find_map(|ev| match ev.content {
                Content::PipeOpened(p) if p.pipe_id == raw => Some(p),
                _ => None,
            })?;
        if opened.owner_id != ctx.self_key && !opened.allowed_members.contains(&ctx.self_key) {
            return None;
        }
        store
            .by_type(&ctx.room_id, EventType::PipeClosed)
            .ok()?
            .into_iter()
            .filter_map(|se| SignedEvent::decode(&se.wire.signed).ok())
            .find_map(|ev| match &ev.content {
                Content::PipeClosed(c) if c.pipe_id == raw => proj::ts(ev.created_at),
                _ => None,
            })
    }

    /// The observed link to a pipe's publisher device, for
    /// `pipe_unreachable.link`.
    async fn pipe_owner_link(&self, ctx: &RoomContext, pipe_id: &PipeId) -> Link {
        let never = Link::NotConnected {
            reason: LinkReason::NeverDialed,
        };
        let Ok(raw) = hex::decode(pipe_id.as_str()) else {
            return never;
        };
        let Ok(raw) = <[u8; 16]>::try_from(raw.as_slice()) else {
            return never;
        };
        let Some(session) = self.sup.session_opt(&ctx.room_id) else {
            return never;
        };
        let Some(opened) = session.node.pipe_opened(raw).await else {
            return never;
        };
        let (peer_entries, peer_paths) = self.peer_evidence(&ctx.room_id).await;
        provider_rows(
            &[opened.owner_endpoint],
            &ctx.snapshot,
            &peer_entries,
            &peer_paths,
        )
        .into_iter()
        .next()
        .map_or(never, |row| row.link)
    }

    /// This daemon's observed per-device transport evidence for one room.
    async fn peer_evidence(&self, room_id: &IrohRoomId) -> (PeerEntryMap, PeerPathMap) {
        let session = self.sup.session_opt(room_id);
        let paths: PeerPathMap = if let Some(session) = session.as_deref() {
            session
                .node
                .peer_paths()
                .await
                .into_iter()
                .map(|(device, path, _relay)| (device, path.label()))
                .collect()
        } else {
            std::collections::HashMap::new()
        };
        let entries: PeerEntryMap = session
            .as_deref()
            .map(|session| session.node.peer_entries().into_iter().collect())
            .unwrap_or_default();
        (entries, paths)
    }

    /// The per-device link rows for one live node.
    async fn peer_rows(&self, node: &iroh_rooms::experimental::session::Node) -> Vec<PeerRow> {
        let paths: std::collections::HashMap<_, _> = node
            .peer_paths()
            .await
            .into_iter()
            .map(|(device, path, _relay)| (device, path.label()))
            .collect();
        node.peer_entries()
            .into_iter()
            .filter_map(|(device, entry)| {
                let identity = entry.identity.as_ref()?;
                Some(PeerRow {
                    subject_id: SubjectId::new(identity.to_string()),
                    device_id: DeviceId::new(device.to_string()),
                    link: peer_link(&entry, paths.get(&device).copied()),
                })
            })
            .collect()
    }

    /// Whole-room reachability from the live session.
    async fn reachability(&self, room_id: &IrohRoomId) -> Reachability {
        if !self.sup.is_open(room_id) {
            return Reachability::Offline;
        }
        match self.sup.session(room_id) {
            Ok(session) => {
                let peers = self.peer_rows(&session.node).await;
                reachability_from_peers(&peers, true)
            }
            Err(_) => Reachability::Connecting,
        }
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// The operations that need an **open room session** in this build.
///
/// The record names three that require liveness — `file.fetch`,
/// `pipe.connect`, and `room.peers` — because authoring is a local CRDT write
/// that converges later. This daemon's authoring path publishes through a live
/// node instead, so `message.send`, `status.post`, `file.share`,
/// `pipe.publish`, and `pipe.revoke` are additionally refused `room_not_live`
/// on a quiescent room.
///
/// That divergence is a **runtime gap, recorded here rather than papered
/// over**: `capabilities` is normatively "would not be refused *at the instant
/// the reply was composed*", so it must describe what this daemon actually
/// serves. Advertising `message.send` on a non-live room because the record
/// says it should work is exactly the drift the array exists to prevent — the
/// fix is to make offline authoring work, not to claim it already does.
const LIVENESS_GATED: [CapabilityToken; 8] = [
    // The record's three.
    CapabilityToken::RoomPeers,
    CapabilityToken::FileFetch,
    CapabilityToken::PipeConnect,
    // This build's five additional authoring refusals.
    CapabilityToken::MessageSend,
    CapabilityToken::StatusPost,
    CapabilityToken::FileShare,
    CapabilityToken::PipePublish,
    CapabilityToken::PipeRevoke,
];

/// The room-scoped operations a caller may invoke **right now**, as the
/// record's operation-name capability tokens.
///
/// A token is present *iff* the operation would not be refused on membership,
/// standing, lifecycle, or **liveness** grounds at the instant the reply was
/// composed. Three corrections over an earlier revision, each observable by a
/// client that tried the advertised token:
///
/// - **`invite.list` is authority-only.** The invite index is the authority's
///   own record of what it issued; offering the token to a plain member
///   advertised a read that answers `insufficient_standing`.
/// - **`pipe.revoke` is not authority-gated.** It is restricted to the pipe's
///   publisher — a narrower relation than role, refused with
///   `pipe_not_publisher` — so an ordinary member that published a pipe may
///   revoke it and an authority that did not, may not.
/// - **`room.archive` belongs to a *former* membership, and only that.** It
///   answers `room_still_active` for an active member, so advertising it there
///   promised a read that cannot succeed; conversely `room.timeline` and
///   `room.members` answer `membership_ended` once a membership has ended, so
///   advertising them there promised the same.
///
/// `room.activate` and `room.deactivate` are naturally idempotent and never
/// refused on liveness in either direction, so both are present for any active
/// member. Only room-scoped tokens appear: `room.list`, `fleet.list`,
/// `subject.ensure`, `daemon.stop`, `invite.redeem`, `transfer.cancel`, and
/// `pipe.release` are not answers about *this* room.
fn room_capabilities(standing: Standing, role: Role, live: bool) -> Vec<CapabilityToken> {
    use CapabilityToken::*;
    if standing != Standing::Active {
        // A former membership reaches exactly one room-scoped operation.
        return vec![RoomArchive];
    }
    let mut caps = vec![
        RoomActivate,
        RoomDeactivate,
        RoomLeave,
        RoomTimeline,
        RoomMembers,
        StatusHistory,
        FileList,
        FileRead,
        PipeList,
        StreamSubscribe,
        StreamUnsubscribe,
        StreamResync,
    ];
    if live {
        caps.extend(LIVENESS_GATED);
    }
    if role == Role::Authority {
        caps.extend([InviteMint, InviteList, InviteRevoke, MemberRemove]);
    }
    caps
}

/// Whole-room reachability from per-device links and liveness.
fn reachability_from_peers(peers: &[PeerRow], live: bool) -> Reachability {
    if !live {
        return Reachability::Offline;
    }
    let any_connected = peers
        .iter()
        .any(|p| matches!(p.link, Link::Direct { .. } | Link::Relay { .. }));
    if any_connected {
        Reachability::Connected
    } else {
        Reachability::Alone
    }
}

/// This daemon's observed peer connection entries for one room, keyed by the
/// P2P endpoint the device binds.
type PeerEntryMap = std::collections::HashMap<
    iroh_rooms::experimental::session::EndpointId,
    iroh_rooms::experimental::session::PeerEntry,
>;

/// This daemon's observed path labels for one room's peers.
type PeerPathMap =
    std::collections::HashMap<iroh_rooms::experimental::session::EndpointId, &'static str>;

/// The per-device provider rows for a set of devices — the one
/// `{subject_id, device_id, link}` shape `room.peers`, `file.list`, and
/// `provider_unreachable` all serve. A device the fold cannot bind to a subject
/// is omitted rather than attributed to an invented one.
fn provider_rows(
    devices: &[iroh_rooms::identity::DeviceKey],
    snapshot: &MembershipSnapshot,
    peer_entries: &PeerEntryMap,
    peer_paths: &PeerPathMap,
) -> Vec<PeerRow> {
    devices
        .iter()
        .filter_map(|device| {
            let subject = snapshot.identity_of_device(device)?;
            let endpoint = crate::supervisor::endpoint_id_of(*device).ok();
            let link = endpoint
                .as_ref()
                .and_then(|endpoint| peer_entries.get(endpoint))
                .map_or(
                    Link::NotConnected {
                        reason: LinkReason::NeverDialed,
                    },
                    |entry| {
                        peer_link(
                            entry,
                            endpoint
                                .as_ref()
                                .and_then(|endpoint| peer_paths.get(endpoint).copied()),
                        )
                    },
                );
            Some(PeerRow {
                subject_id: SubjectId::new(subject.to_string()),
                device_id: DeviceId::new(device.to_string()),
                link,
            })
        })
        .collect()
}

/// The v2 wire label for a status label (snake_case, matching the Iroh
/// content vocabulary the runtime authors).
fn status_label_wire(label: StatusLabel) -> &'static str {
    match label {
        StatusLabel::Online => "online",
        StatusLabel::Idle => "idle",
        StatusLabel::Claiming => "claiming",
        StatusLabel::Working => "working",
        StatusLabel::Done => "done",
        StatusLabel::Failed => "failed",
        StatusLabel::Blocked => "blocked",
    }
}

/// Parse an on-wire status label into the closed vocabulary.
fn status_label_parse(label: &str) -> CoreResult<StatusLabel> {
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

/// Whether a socket address is loopback (IPv4 127.0.0.0/8 or IPv6 ::1).
fn is_loopback_addr(addr: &std::net::SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// Convert the runtime's observed peer state to the one shared v2 link
/// vocabulary. The state-change clock is observability data (the `since`
/// field), not a signed room-event timestamp.
///
/// A `since` that will not convert to a wire instant makes the link
/// `not_connected { no_route }` rather than a connection dated to the epoch:
/// the `direct` and `relay` arms carry `since` as a required field, so a link
/// this daemon cannot date is a link it cannot assert.
fn peer_link(entry: &iroh_rooms::experimental::session::PeerEntry, path: Option<&str>) -> Link {
    use iroh_rooms::experimental::session::{OfflineReason, PeerConnState};

    if entry.state == PeerConnState::Connected {
        let since = proj::ts(entry.last_change_ms);
        return match (path, since) {
            (Some("relay"), Some(since)) => Link::Relay { since },
            (Some("direct" | "mixed"), Some(since)) => Link::Direct { since },
            _ => Link::NotConnected {
                reason: LinkReason::NoRoute,
            },
        };
    }
    let reason = match entry.offline_reason {
        OfflineReason::NeverDialed => LinkReason::NeverDialed,
        OfflineReason::Unreachable | OfflineReason::TransportError => LinkReason::DialFailed,
        OfflineReason::LinkDropped | OfflineReason::Deauthorized => LinkReason::Closed,
    };
    Link::NotConnected { reason }
}

/// A room's `room_created` origin event, looked up **by kind**.
///
/// Never `room_tail(room_id, 1)`: that is the newest row, which equals the
/// genesis only on a room that has exactly one event.
fn genesis_event(
    store: &iroh_rooms::experimental::store::EventStore,
    room_id: &IrohRoomId,
) -> Option<SignedEvent> {
    store
        .by_type(room_id, EventType::RoomCreated)
        .ok()?
        .iter()
        .find_map(|se| SignedEvent::decode(&se.wire.signed).ok())
}

/// A `"host:port"` address as the v2 `target` object, or `None` when the
/// string is not a socket address.
///
/// It parses rather than splits on the last colon: a bare IPv6 literal has
/// several, and the old split produced `port: 0` for anything it could not
/// read — a port the caller would dial and a listener that never bound.
fn parse_target(addr: &str) -> Option<Target> {
    let parsed: std::net::SocketAddr = addr.parse().ok()?;
    Some(Target {
        host: parsed.ip().to_string(),
        port: u64::from(parsed.port()),
    })
}

/// The closed pipe ids (revoked), for the pipe list filter.
fn closed_pipe_ids(
    store: &iroh_rooms::experimental::store::EventStore,
    room_id: &IrohRoomId,
) -> CoreResult<BTreeSet<[u8; 16]>> {
    let mut ids = BTreeSet::new();
    for se in store
        .by_type(room_id, EventType::PipeClosed)
        .map_err(|e| CoreError::internal(format!("could not read pipe.closed events: {e}")))?
    {
        if let Ok(ev) = SignedEvent::decode(&se.wire.signed) {
            if let Content::PipeClosed(c) = ev.content {
                ids.insert(c.pipe_id);
            }
        }
    }
    Ok(ids)
}

// ---------------------------------------------------------------------------
// The normative validation pipeline (see the module docs)
// ---------------------------------------------------------------------------

/// The paging fields an operation carries, if any. Exactly the record's **six
/// paging operations** — `room.timeline`, `room.archive`, `status.history`,
/// `invite.list`, `file.list`, and `pipe.list` — answer `Some`, and they all
/// take the same three fields with the same bounds.
fn request_page(call: &TypedCall) -> Option<&Page> {
    match call {
        TypedCall::RoomTimeline(r) => Some(&r.page),
        TypedCall::RoomArchive(r) => Some(&r.page),
        TypedCall::StatusHistory(r) => Some(&r.page),
        TypedCall::InviteList(r) => Some(&r.page),
        TypedCall::FileList(r) => Some(&r.page),
        TypedCall::PipeList(r) => Some(&r.page),
        _ => None,
    }
}

/// **Validation-order step 1** — the structural checks the codec could not
/// make, applied before every later stage for every operation.
///
/// The codec owns the rest of step 1: it decodes each request into its typed
/// shape, so a missing key, an unrecognised key, a wrong JSON type, and a
/// malformed `cursor` or `direction` are all refused at the edge. What is left
/// here is what needs a **served** value the wire types cannot express:
///
/// - `limit` against `timeline_page_max`, for all six paging operations. The
///   record refuses an out-of-range page size, never silently clamps it, and
///   never answers `resource_exhausted` for it.
/// - `member.remove`'s `subject_id` format, so a malformed identity is a
///   structural refusal rather than reaching the authority gate.
///
/// This runs **ahead of the subject stage**, per the record's normative table
/// and its stated reason: step 1 discloses only value formats and served
/// limits — both already published in `hello` — and never daemon state, so
/// putting it first costs the non-oracle property nothing while sparing a
/// caller a round trip that reports the wrong problem.
pub(crate) fn validate_structure(call: &TypedCall) -> Result<(), ApiError> {
    if let Some(page) = request_page(call) {
        Window::resolve(page)?;
    }
    if let TypedCall::MemberRemove(r) = call {
        if r.subject_id
            .as_str()
            .trim()
            .parse::<iroh_rooms::identity::IdentityKey>()
            .is_err()
        {
            return Err(ApiError::InvalidArgument {
                field: "in.subject_id".into(),
                reason: InvalidReason::Format,
            });
        }
    }
    Ok(())
}

/// **Validation-order steps 4–6 scope** — the `room_id` an operation's `in`
/// carries, or `None` for one that carries none.
///
/// The record makes this decidable from the schema rather than by convention:
/// the eight operations with no `room_id` are `subject.ensure`, `daemon.stop`,
/// `room.list`, `invite.redeem`, `fleet.list`, `transfer.cancel`, and
/// `pipe.release` — plus, in this dispatcher, nothing else, because the three
/// `stream.*` operations are connection-scoped and their host authorizes them
/// against the same typed reads (see the module docs).
fn request_room(call: &TypedCall) -> Option<&RoomId> {
    match call {
        TypedCall::RoomActivate(r) => Some(&r.room_id),
        TypedCall::RoomDeactivate(r) => Some(&r.room_id),
        TypedCall::RoomLeave(r) => Some(&r.room_id),
        TypedCall::RoomTimeline(r) => Some(&r.room_id),
        TypedCall::RoomMembers(r) => Some(&r.room_id),
        TypedCall::RoomArchive(r) => Some(&r.room_id),
        TypedCall::RoomPeers(r) => Some(&r.room_id),
        TypedCall::MemberRemove(r) => Some(&r.room_id),
        TypedCall::InviteMint(r) => Some(&r.room_id),
        TypedCall::InviteList(r) => Some(&r.room_id),
        TypedCall::InviteRevoke(r) => Some(&r.room_id),
        TypedCall::MessageSend(r) => Some(&r.room_id),
        TypedCall::StatusPost(r) => Some(&r.room_id),
        TypedCall::StatusHistory(r) => Some(&r.room_id),
        TypedCall::FileShare(r) => Some(&r.room_id),
        TypedCall::FileList(r) => Some(&r.room_id),
        TypedCall::FileFetch(r) => Some(&r.room_id),
        TypedCall::FileRead(r) => Some(&r.room_id),
        TypedCall::PipePublish(r) => Some(&r.room_id),
        TypedCall::PipeList(r) => Some(&r.room_id),
        TypedCall::PipeConnect(r) => Some(&r.room_id),
        TypedCall::PipeRevoke(r) => Some(&r.room_id),
        TypedCall::StreamSubscribe(r) => Some(&r.room_id),
        TypedCall::StreamUnsubscribe(r) => Some(&r.room_id),
        TypedCall::StreamResync(r) => Some(&r.room_id),
        TypedCall::SubjectEnsure(_)
        | TypedCall::DaemonStop(_)
        | TypedCall::RoomCreate(_)
        | TypedCall::RoomList(_)
        | TypedCall::InviteRedeem(_)
        | TypedCall::FleetList(_)
        | TypedCall::TransferCancel(_)
        | TypedCall::PipeRelease(_) => None,
    }
}

/// **Validation-order step 5 exception** — the operations defined over a
/// *former* membership, which therefore skip the standing stage.
///
/// The record names exactly two: `room.archive` and `room.list`. Only
/// `room.archive` appears here, because `room.list` carries no `room_id` and so
/// never reaches step 4 in the first place. `room.archive` exists *to* open a
/// room the caller has left, so refusing it on standing would make it
/// unreachable in every state it is defined for; it checks the converse
/// (`room_still_active`) in its own body.
fn standing_exempt(call: &TypedCall) -> bool {
    matches!(call, TypedCall::RoomArchive(_))
}

/// **Validation-order step 6 scope** — the four operations that require
/// `role: "authority"`, and no others.
///
/// `member.remove`, `invite.mint`, `invite.revoke`, and `invite.list`. Every
/// other operation is open to any active member; in particular `pipe.revoke`
/// is **not** here — it is restricted to the pipe's publisher, a narrower
/// relation than role that answers `pipe_not_publisher`.
fn requires_authority(call: &TypedCall) -> bool {
    matches!(
        call,
        TypedCall::MemberRemove(_)
            | TypedCall::InviteMint(_)
            | TypedCall::InviteRevoke(_)
            | TypedCall::InviteList(_)
    )
}

// ---------------------------------------------------------------------------
// The typed dispatch table (the engine's v2 surface)
// ---------------------------------------------------------------------------

/// One typed operation in, its typed output out, or a typed error. This is
/// the engine's v2 dispatch seam: the codec hands a decoded [`Call`] here and
/// gets a `Result<Output, ApiError>` to encode.
///
/// The dispatch is **total by construction**: the codec's router already
/// refused any `op` not in the 33, so the downcast to the concrete input type
/// cannot fail.
pub(crate) async fn dispatch(
    sup: &RoomSupervisor,
    call: TypedCall,
) -> Result<TypedReply, ApiError> {
    let t = TypedSupervisor::new(sup);

    // Step 1 — structural decode and bounds, for every operation.
    validate_structure(&call)?;

    // Step 2 — subject precondition. `subject_absent` outranks
    // `room_not_available` and every later stage, so a subject-less caller can
    // never use error-code selection as a room probe.
    //
    // Two operations skip it, and the stage table's own rule is why: a stage
    // "runs only where its precondition is meaningful". `subject.ensure` skips
    // it because it exists to create the subject. `daemon.stop` skips it
    // because its precondition is a running daemon, not a local subject — it
    // reads and writes no subject state, and `daemon_stop_does_not_require_a_
    // subject` in the corpus settles that it must succeed on a fresh daemon.
    //
    // Refusing it here would also be a **refusal that still acts**: the stop is
    // sequenced by `Engine::execute_with` before dispatch runs (the reply must
    // flush before teardown), so a step-2 refusal would tell the client the
    // operation failed while the daemon exited anyway. The record's stage table
    // names only `subject.ensure`, and this second exemption is recorded as a
    // divergence on #165 rather than left as a contradiction between the reply
    // and the effect.
    let skips_subject = matches!(call, TypedCall::SubjectEnsure(_) | TypedCall::DaemonStop(_));
    if !skips_subject && !t.subject_present()? {
        return Err(ApiError::SubjectAbsent);
    }

    // Step 3 — dedup. It lives in `Engine::execute_with`, which consults the
    // ledger after running steps 1 and 2 and before calling this function, so a
    // structurally invalid or subject-less request never binds an `op_id` to a
    // refusal a corrected retry would then replay.

    // Steps 4, 5, and 6 — room index, standing, role — for every operation
    // whose `in` carries a `room_id`, in one place with one ordering.
    let ctx = match request_room(&call) {
        Some(api_room) => Some(
            t.room_context(api_room, standing_exempt(&call), requires_authority(&call))
                .await?,
        ),
        None => None,
    };
    // Every room-bearing arm below unwraps this; `request_room` and the match
    // are the same total enumeration, so the two cannot drift apart without a
    // compile error in `request_room`'s exhaustive match.
    let room = || {
        ctx.as_ref()
            .expect("a room-bearing call resolved a context")
    };

    // Step 7 — operation semantics.
    match call {
        TypedCall::SubjectEnsure(_) => t.subject_ensure().map(TypedReply::SubjectEnsure),
        TypedCall::RoomCreate(r) => t.room_create(&r).await.map(TypedReply::RoomCreate),
        TypedCall::RoomList(_) => t.room_list().await.map(TypedReply::RoomList),
        TypedCall::RoomActivate(r) => t
            .room_activate(room(), &r)
            .await
            .map(TypedReply::RoomActivate),
        TypedCall::RoomDeactivate(r) => t
            .room_deactivate(room(), &r)
            .await
            .map(TypedReply::RoomDeactivate),
        TypedCall::RoomLeave(r) => t.room_leave(room(), &r).await.map(TypedReply::RoomLeave),
        TypedCall::RoomTimeline(r) => t.room_timeline(room(), &r).map(TypedReply::RoomTimeline),
        TypedCall::RoomMembers(r) => t.room_members(room(), &r).map(TypedReply::RoomMembers),
        TypedCall::RoomArchive(r) => t.room_archive(room(), &r).map(TypedReply::RoomArchive),
        TypedCall::RoomPeers(r) => t.room_peers(room(), &r).await.map(TypedReply::RoomPeers),
        TypedCall::MemberRemove(r) => t
            .member_remove(room(), &r)
            .await
            .map(TypedReply::MemberRemove),
        TypedCall::InviteMint(r) => t.invite_mint(room(), &r).await.map(TypedReply::InviteMint),
        TypedCall::InviteList(r) => t.invite_list(room(), &r).map(TypedReply::InviteList),
        TypedCall::InviteRevoke(r) => t
            .invite_revoke(room(), &r)
            .await
            .map(TypedReply::InviteRevoke),
        TypedCall::InviteRedeem(r) => t.invite_redeem(&r).await.map(TypedReply::InviteRedeem),
        TypedCall::MessageSend(r) => t
            .message_send(room(), &r)
            .await
            .map(TypedReply::MessageSend),
        TypedCall::StatusPost(r) => t.status_post(room(), &r).await.map(TypedReply::StatusPost),
        TypedCall::StatusHistory(r) => t.status_history(room(), &r).map(TypedReply::StatusHistory),
        TypedCall::FleetList(_) => t.fleet_list().await.map(TypedReply::FleetList),
        TypedCall::FileShare(r) => t.file_share(room(), &r).await.map(TypedReply::FileShare),
        TypedCall::FileList(r) => t.file_list(room(), &r).await.map(TypedReply::FileList),
        TypedCall::FileFetch(r) => t.file_fetch(room(), &r).await.map(TypedReply::FileFetch),
        TypedCall::FileRead(r) => t.file_read(room(), &r).await.map(TypedReply::FileRead),
        TypedCall::TransferCancel(r) => t.transfer_cancel(&r).await.map(TypedReply::TransferCancel),
        TypedCall::PipePublish(r) => t
            .pipe_publish(room(), &r)
            .await
            .map(TypedReply::PipePublish),
        TypedCall::PipeList(r) => t.pipe_list(room(), &r).await.map(TypedReply::PipeList),
        TypedCall::PipeConnect(r) => t
            .pipe_connect(room(), &r)
            .await
            .map(TypedReply::PipeConnect),
        TypedCall::PipeRelease(r) => t.pipe_release(&r).await.map(TypedReply::PipeRelease),
        TypedCall::PipeRevoke(r) => t.pipe_revoke(room(), &r).await.map(TypedReply::PipeRevoke),
        // Stream operations are connection-scoped: the host resolves them
        // against the connection's own subscription set, not the supervisor.
        // Reaching this dispatcher means no such subscription exists on the
        // caller's connection, and the refusal names the room it asked about
        // rather than an invented empty id. The room-access stages above have
        // already run, so this is never a membership oracle.
        TypedCall::StreamSubscribe(r) => Err(ApiError::SubscriptionUnknown { room_id: r.room_id }),
        TypedCall::StreamUnsubscribe(r) => {
            Err(ApiError::SubscriptionUnknown { room_id: r.room_id })
        }
        TypedCall::StreamResync(r) => Err(ApiError::SubscriptionUnknown { room_id: r.room_id }),
        TypedCall::DaemonStop(_) => Ok(TypedReply::DaemonStop(DaemonStopOut { stopping: true })),
    }
}

/// A typed call: one of the 33 operations with its concrete input.
#[derive(Debug)]
pub enum TypedCall {
    /// subject.ensure
    SubjectEnsure(SubjectEnsure),
    /// daemon.stop
    DaemonStop(DaemonStop),
    /// room.create
    RoomCreate(RoomCreate),
    /// room.list
    RoomList(RoomList),
    /// room.activate
    RoomActivate(RoomActivate),
    /// room.deactivate
    RoomDeactivate(RoomDeactivate),
    /// room.leave
    RoomLeave(RoomLeave),
    /// room.timeline
    RoomTimeline(RoomTimeline),
    /// room.members
    RoomMembers(RoomMembers),
    /// room.archive
    RoomArchive(RoomArchive),
    /// room.peers
    RoomPeers(RoomPeers),
    /// member.remove
    MemberRemove(MemberRemove),
    /// invite.mint
    InviteMint(InviteMint),
    /// invite.list
    InviteList(InviteList),
    /// invite.revoke
    InviteRevoke(InviteRevoke),
    /// invite.redeem
    InviteRedeem(InviteRedeem),
    /// message.send
    MessageSend(MessageSend),
    /// status.post
    StatusPost(StatusPost),
    /// status.history
    StatusHistory(StatusHistory),
    /// fleet.list
    FleetList(FleetList),
    /// file.share
    FileShare(FileShare),
    /// file.list
    FileList(FileList),
    /// file.fetch
    FileFetch(FileFetch),
    /// file.read
    FileRead(FileRead),
    /// transfer.cancel
    TransferCancel(TransferCancel),
    /// pipe.publish
    PipePublish(PipePublish),
    /// pipe.list
    PipeList(PipeList),
    /// pipe.connect
    PipeConnect(PipeConnect),
    /// pipe.release
    PipeRelease(PipeRelease),
    /// pipe.revoke
    PipeRevoke(PipeRevoke),
    /// stream.subscribe
    StreamSubscribe(StreamSubscribe),
    /// stream.unsubscribe
    StreamUnsubscribe(StreamUnsubscribe),
    /// stream.resync
    StreamResync(StreamResync),
}

impl TypedCall {
    /// The operation's wire name (its capability token).
    #[must_use]
    pub fn path(&self) -> &'static str {
        match self {
            TypedCall::SubjectEnsure(_) => "subject.ensure",
            TypedCall::DaemonStop(_) => "daemon.stop",
            TypedCall::RoomCreate(_) => "room.create",
            TypedCall::RoomList(_) => "room.list",
            TypedCall::RoomActivate(_) => "room.activate",
            TypedCall::RoomDeactivate(_) => "room.deactivate",
            TypedCall::RoomLeave(_) => "room.leave",
            TypedCall::RoomTimeline(_) => "room.timeline",
            TypedCall::RoomMembers(_) => "room.members",
            TypedCall::RoomArchive(_) => "room.archive",
            TypedCall::RoomPeers(_) => "room.peers",
            TypedCall::MemberRemove(_) => "member.remove",
            TypedCall::InviteMint(_) => "invite.mint",
            TypedCall::InviteList(_) => "invite.list",
            TypedCall::InviteRevoke(_) => "invite.revoke",
            TypedCall::InviteRedeem(_) => "invite.redeem",
            TypedCall::MessageSend(_) => "message.send",
            TypedCall::StatusPost(_) => "status.post",
            TypedCall::StatusHistory(_) => "status.history",
            TypedCall::FleetList(_) => "fleet.list",
            TypedCall::FileShare(_) => "file.share",
            TypedCall::FileList(_) => "file.list",
            TypedCall::FileFetch(_) => "file.fetch",
            TypedCall::FileRead(_) => "file.read",
            TypedCall::TransferCancel(_) => "transfer.cancel",
            TypedCall::PipePublish(_) => "pipe.publish",
            TypedCall::PipeList(_) => "pipe.list",
            TypedCall::PipeConnect(_) => "pipe.connect",
            TypedCall::PipeRelease(_) => "pipe.release",
            TypedCall::PipeRevoke(_) => "pipe.revoke",
            TypedCall::StreamSubscribe(_) => "stream.subscribe",
            TypedCall::StreamUnsubscribe(_) => "stream.unsubscribe",
            TypedCall::StreamResync(_) => "stream.resync",
        }
    }

    /// A stable hash of the canonical request body, for telling a faithful
    /// `op_id` retry from a conflicting reuse. The typed input serializes
    /// deterministically (serde field order is declaration order), so two
    /// calls with equal bodies hash alike and two with different bodies do
    /// not — which is exactly the fidelity the dedup ledger needs.
    #[must_use]
    pub fn body_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let body = match self {
            TypedCall::SubjectEnsure(r) => serde_json::to_vec(r),
            TypedCall::DaemonStop(r) => serde_json::to_vec(r),
            TypedCall::RoomCreate(r) => serde_json::to_vec(r),
            TypedCall::RoomList(r) => serde_json::to_vec(r),
            TypedCall::RoomActivate(r) => serde_json::to_vec(r),
            TypedCall::RoomDeactivate(r) => serde_json::to_vec(r),
            TypedCall::RoomLeave(r) => serde_json::to_vec(r),
            TypedCall::RoomTimeline(r) => serde_json::to_vec(r),
            TypedCall::RoomMembers(r) => serde_json::to_vec(r),
            TypedCall::RoomArchive(r) => serde_json::to_vec(r),
            TypedCall::RoomPeers(r) => serde_json::to_vec(r),
            TypedCall::MemberRemove(r) => serde_json::to_vec(r),
            TypedCall::InviteMint(r) => serde_json::to_vec(r),
            TypedCall::InviteList(r) => serde_json::to_vec(r),
            TypedCall::InviteRevoke(r) => serde_json::to_vec(r),
            TypedCall::InviteRedeem(r) => serde_json::to_vec(r),
            TypedCall::MessageSend(r) => serde_json::to_vec(r),
            TypedCall::StatusPost(r) => serde_json::to_vec(r),
            TypedCall::StatusHistory(r) => serde_json::to_vec(r),
            TypedCall::FleetList(r) => serde_json::to_vec(r),
            TypedCall::FileShare(r) => serde_json::to_vec(r),
            TypedCall::FileList(r) => serde_json::to_vec(r),
            TypedCall::FileFetch(r) => serde_json::to_vec(r),
            TypedCall::FileRead(r) => serde_json::to_vec(r),
            TypedCall::TransferCancel(r) => serde_json::to_vec(r),
            TypedCall::PipePublish(r) => serde_json::to_vec(r),
            TypedCall::PipeList(r) => serde_json::to_vec(r),
            TypedCall::PipeConnect(r) => serde_json::to_vec(r),
            TypedCall::PipeRelease(r) => serde_json::to_vec(r),
            TypedCall::PipeRevoke(r) => serde_json::to_vec(r),
            TypedCall::StreamSubscribe(r) => serde_json::to_vec(r),
            TypedCall::StreamUnsubscribe(r) => serde_json::to_vec(r),
            TypedCall::StreamResync(r) => serde_json::to_vec(r),
        }
        .unwrap_or_default();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        // The operation path is part of the fingerprint: two operations with
        // structurally identical inputs (pipe.connect and pipe.revoke both
        // serialize as {room_id, pipe_id}) must not read as a faithful replay
        // of one another. Hash the path alongside the body.
        self.path().hash(&mut hasher);
        body.hash(&mut hasher);
        hasher.finish()
    }
}

/// Resolve a codec-routed `op` and its erased input into a concrete
/// [`TypedCall`]. The codec guarantees `op` is one of the 33 and the input
/// decoded into the matching request type, so the downcast is total — a
/// mismatch is a codec bug, surfaced as `malformed_frame`, never a panic.
#[must_use]
pub fn resolve_call(op: &str, input: &dyn std::any::Any) -> Option<TypedCall> {
    macro_rules! arm {
        ($path:literal, $ty:ty, $variant:ident) => {
            if op == <$ty as Operation>::PATH {
                return input
                    .downcast_ref::<$ty>()
                    .cloned()
                    .map(TypedCall::$variant);
            }
        };
    }
    arm!("subject.ensure", SubjectEnsure, SubjectEnsure);
    arm!("daemon.stop", DaemonStop, DaemonStop);
    arm!("room.create", RoomCreate, RoomCreate);
    arm!("room.list", RoomList, RoomList);
    arm!("room.activate", RoomActivate, RoomActivate);
    arm!("room.deactivate", RoomDeactivate, RoomDeactivate);
    arm!("room.leave", RoomLeave, RoomLeave);
    arm!("room.timeline", RoomTimeline, RoomTimeline);
    arm!("room.members", RoomMembers, RoomMembers);
    arm!("room.archive", RoomArchive, RoomArchive);
    arm!("room.peers", RoomPeers, RoomPeers);
    arm!("member.remove", MemberRemove, MemberRemove);
    arm!("invite.mint", InviteMint, InviteMint);
    arm!("invite.list", InviteList, InviteList);
    arm!("invite.revoke", InviteRevoke, InviteRevoke);
    arm!("invite.redeem", InviteRedeem, InviteRedeem);
    arm!("message.send", MessageSend, MessageSend);
    arm!("status.post", StatusPost, StatusPost);
    arm!("status.history", StatusHistory, StatusHistory);
    arm!("fleet.list", FleetList, FleetList);
    arm!("file.share", FileShare, FileShare);
    arm!("file.list", FileList, FileList);
    arm!("file.fetch", FileFetch, FileFetch);
    arm!("file.read", FileRead, FileRead);
    arm!("transfer.cancel", TransferCancel, TransferCancel);
    arm!("pipe.publish", PipePublish, PipePublish);
    arm!("pipe.list", PipeList, PipeList);
    arm!("pipe.connect", PipeConnect, PipeConnect);
    arm!("pipe.release", PipeRelease, PipeRelease);
    arm!("pipe.revoke", PipeRevoke, PipeRevoke);
    arm!("stream.subscribe", StreamSubscribe, StreamSubscribe);
    arm!("stream.unsubscribe", StreamUnsubscribe, StreamUnsubscribe);
    arm!("stream.resync", StreamResync, StreamResync);
    let _ = op;
    None
}

/// A typed reply: the output paired with the call's operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TypedReply {
    /// subject.ensure
    SubjectEnsure(SubjectEnsureOut),
    /// daemon.stop
    DaemonStop(DaemonStopOut),
    /// room.create
    RoomCreate(RoomCreateOut),
    /// room.list
    RoomList(RoomListOut),
    /// room.activate
    RoomActivate(RoomActivateOut),
    /// room.deactivate
    RoomDeactivate(RoomDeactivateOut),
    /// room.leave
    RoomLeave(RoomLeaveOut),
    /// room.timeline
    RoomTimeline(RoomTimelineOut),
    /// room.members
    RoomMembers(RoomMembersOut),
    /// room.archive
    RoomArchive(RoomArchiveOut),
    /// room.peers
    RoomPeers(RoomPeersOut),
    /// member.remove
    MemberRemove(MemberRemoveOut),
    /// invite.mint
    InviteMint(InviteMintOut),
    /// invite.list
    InviteList(InviteListOut),
    /// invite.revoke
    InviteRevoke(InviteRevokeOut),
    /// invite.redeem
    InviteRedeem(InviteRedeemOut),
    /// message.send
    MessageSend(MessageSendOut),
    /// status.post
    StatusPost(StatusPostOut),
    /// status.history
    StatusHistory(StatusHistoryOut),
    /// fleet.list
    FleetList(FleetListOut),
    /// file.share
    FileShare(FileShareOut),
    /// file.list
    FileList(FileListOut),
    /// file.fetch
    FileFetch(FileFetchOut),
    /// file.read
    FileRead(FileReadOut),
    /// transfer.cancel
    TransferCancel(TransferCancelOut),
    /// pipe.publish
    PipePublish(PipePublishOut),
    /// pipe.list
    PipeList(PipeListOut),
    /// pipe.connect
    PipeConnect(PipeConnectOut),
    /// pipe.release
    PipeRelease(PipeReleaseOut),
    /// pipe.revoke
    PipeRevoke(PipeRevokeOut),
}

/// Map a core error onto the typed v2 taxonomy **with no room context**.
///
/// Every room-scoped code needs a `room_id`, and the record's error schema
/// fixes that field: a client branches on it. So there is no roomless mapping
/// that can produce one honestly, and this function does not invent `""`.
///
/// The room-scoped kinds are unreachable here by construction — every operation
/// that can raise one names a room and uses [`core_to_api_room`] — and if one
/// arrives anyway it is an index this daemon could not read, which is the
/// fieldless `room_index_unreadable`, not a refusal about a room nobody named.
fn core_to_api(err: CoreError) -> ApiError {
    match err.kind {
        ErrorKind::IdentityMissing => ApiError::SubjectAbsent,
        ErrorKind::BadTicket => ApiError::CapabilityInvalid,
        ErrorKind::IdentityExists | ErrorKind::InvalidParams => ApiError::InvalidArgument {
            field: "in".to_string(),
            reason: InvalidReason::Format,
        },
        ErrorKind::RoomUnknown
        | ErrorKind::NotAMember
        | ErrorKind::FileUnauthorized
        | ErrorKind::RoomNotOpen
        | ErrorKind::TicketExpired
        | ErrorKind::FileUnavailable
        | ErrorKind::HashMismatch
        | ErrorKind::PipeDenied
        | ErrorKind::PeerUnreachable
        | ErrorKind::Internal => ApiError::RoomIndexUnreadable,
    }
}

/// Map a core error onto the typed v2 taxonomy for an operation that named a
/// room. Every room-scoped code carries **that** identifier — the one the
/// caller supplied one frame earlier, which discloses nothing back to it and is
/// what the record's `room_not_available` echo requires.
///
/// Codes whose typed fields name something other than a room (`file_id`,
/// `pipe_id`, a digest pair) are **not** produced here: the operation that owns
/// them holds those values and builds the error itself, because an error whose
/// typed field is an empty string is a schema satisfied and a fact invented.
fn core_to_api_room(err: CoreError, room_id: &RoomId) -> ApiError {
    match err.kind {
        ErrorKind::IdentityMissing => ApiError::SubjectAbsent,
        ErrorKind::RoomUnknown | ErrorKind::NotAMember | ErrorKind::FileUnauthorized => {
            ApiError::RoomNotAvailable {
                room_id: room_id.clone(),
            }
        }
        ErrorKind::RoomNotOpen => ApiError::RoomNotLive {
            room_id: room_id.clone(),
        },
        ErrorKind::PipeDenied => ApiError::PolicyRefused {
            room_id: room_id.clone(),
        },
        ErrorKind::BadTicket => ApiError::CapabilityInvalid,
        ErrorKind::IdentityExists | ErrorKind::InvalidParams => ApiError::InvalidArgument {
            field: "in".to_string(),
            reason: InvalidReason::Format,
        },
        // A store or fold failure the operation did not classify. It is a read
        // this daemon could not complete, not a malformed request.
        ErrorKind::TicketExpired
        | ErrorKind::FileUnavailable
        | ErrorKind::HashMismatch
        | ErrorKind::PeerUnreachable
        | ErrorKind::Internal => ApiError::RoomIndexUnreadable,
    }
}

/// The redemption-side refusal a failed `invite.redeem` earns.
///
/// `invite.redeem` is the only operation a non-member can reach, so **every**
/// failure here is one of the four redemption-side codes. A room-scoped code
/// would name a room to a caller who is not in it — the probe the non-oracle
/// property exists to prevent — and `capability_invalid` is deliberately
/// fieldless so a forged capability, one for a room that does not exist, and one
/// naming a different identity are indistinguishable.
fn redemption_error(err: CoreError, expired_at: Option<Timestamp>) -> ApiError {
    match (err.kind, expired_at) {
        // The instant comes from the capability the caller itself presented.
        (ErrorKind::TicketExpired, Some(expired_at)) => ApiError::CapabilityExpired { expired_at },
        _ => ApiError::CapabilityInvalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ------------------------------------------------------------------
    // The pipeline's scope tables
    //
    // Each stage runs for a set the record fixes. These assert the set, not a
    // sample of it: a stage that quietly grew or lost an operation is the
    // "rule with a carve-out" failure the record warns about, and only an
    // exhaustive table catches it.
    // ------------------------------------------------------------------

    fn rid() -> RoomId {
        RoomId::new(format!("blake3:{}", "ab".repeat(32)))
    }

    fn page() -> Page {
        Page {
            cursor: Cursor::Start,
            direction: Direction::Forward,
            limit: 32,
        }
    }

    fn subject() -> SubjectId {
        SubjectId::new(
            iroh_rooms::identity::SigningKey::generate()
                .identity_key()
                .to_string(),
        )
    }

    /// One well-formed call per operation, so a scope table can be asserted
    /// over all 33 rather than over a sample.
    fn every_call() -> Vec<TypedCall> {
        vec![
            TypedCall::SubjectEnsure(SubjectEnsure {}),
            TypedCall::DaemonStop(DaemonStop {}),
            TypedCall::RoomCreate(RoomCreate { name: "R".into() }),
            TypedCall::RoomList(RoomList {}),
            TypedCall::RoomActivate(RoomActivate { room_id: rid() }),
            TypedCall::RoomDeactivate(RoomDeactivate { room_id: rid() }),
            TypedCall::RoomLeave(RoomLeave { room_id: rid() }),
            TypedCall::RoomTimeline(RoomTimeline {
                room_id: rid(),
                page: page(),
            }),
            TypedCall::RoomMembers(RoomMembers { room_id: rid() }),
            TypedCall::RoomArchive(RoomArchive {
                room_id: rid(),
                page: page(),
            }),
            TypedCall::RoomPeers(RoomPeers { room_id: rid() }),
            TypedCall::MemberRemove(MemberRemove {
                room_id: rid(),
                subject_id: subject(),
            }),
            TypedCall::InviteMint(InviteMint {
                room_id: rid(),
                subject_id: subject(),
                role: Role::Member,
                expires_at: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
            }),
            TypedCall::InviteList(InviteList {
                room_id: rid(),
                page: page(),
            }),
            TypedCall::InviteRevoke(InviteRevoke {
                room_id: rid(),
                invite_id: InviteId::new("i"),
            }),
            TypedCall::InviteRedeem(InviteRedeem {
                capability: "cap".into(),
            }),
            TypedCall::MessageSend(MessageSend {
                room_id: rid(),
                body: "hi".into(),
            }),
            TypedCall::StatusPost(StatusPost {
                room_id: rid(),
                label: StatusLabel::Working,
                progress: Progress::Absent,
            }),
            TypedCall::StatusHistory(StatusHistory {
                room_id: rid(),
                subject_id: subject(),
                page: page(),
            }),
            TypedCall::FleetList(FleetList {}),
            TypedCall::FileShare(FileShare {
                room_id: rid(),
                name: "f".into(),
                declared_bytes: 1,
                declared_content_type: "text/plain".into(),
            }),
            TypedCall::FileList(FileList {
                room_id: rid(),
                page: page(),
            }),
            TypedCall::FileFetch(FileFetch {
                room_id: rid(),
                file_id: FileId::new("file_00"),
            }),
            TypedCall::FileRead(FileRead {
                room_id: rid(),
                file_id: FileId::new("file_00"),
            }),
            TypedCall::TransferCancel(TransferCancel {
                transfer_op_id: OpId::new("t"),
            }),
            TypedCall::PipePublish(PipePublish {
                room_id: rid(),
                target: Target {
                    host: "127.0.0.1".into(),
                    port: 9,
                },
                audience: Audience::Room,
            }),
            TypedCall::PipeList(PipeList {
                room_id: rid(),
                page: page(),
            }),
            TypedCall::PipeConnect(PipeConnect {
                room_id: rid(),
                pipe_id: PipeId::new("ab".repeat(16)),
            }),
            TypedCall::PipeRelease(PipeRelease {
                connection_id: "127.0.0.1:1".into(),
            }),
            TypedCall::PipeRevoke(PipeRevoke {
                room_id: rid(),
                pipe_id: PipeId::new("ab".repeat(16)),
            }),
            TypedCall::StreamSubscribe(StreamSubscribe {
                room_id: rid(),
                from: Cursor::Start,
            }),
            TypedCall::StreamUnsubscribe(StreamUnsubscribe { room_id: rid() }),
            TypedCall::StreamResync(StreamResync {
                room_id: rid(),
                from_pos: 0,
            }),
        ]
    }

    #[test]
    fn the_call_table_covers_all_thirty_three_operations() {
        let calls = every_call();
        let mut paths: Vec<&str> = calls.iter().map(TypedCall::path).collect();
        paths.sort_unstable();
        paths.dedup();
        assert_eq!(paths.len(), 33, "one call per approved operation");
    }

    /// Validation-order step 4/5/6 scope. Every operation whose `in` carries a
    /// `room_id` resolves a [`RoomContext`]; the eight that do not are named
    /// exactly.
    #[test]
    fn room_bearing_scope_is_decidable_from_the_schema() {
        let roomless: BTreeSet<&str> = every_call()
            .iter()
            .filter(|c| request_room(c).is_none())
            .map(TypedCall::path)
            .collect();
        assert_eq!(
            roomless,
            BTreeSet::from([
                "subject.ensure",
                "daemon.stop",
                "room.create",
                "room.list",
                "invite.redeem",
                "fleet.list",
                "transfer.cancel",
                "pipe.release",
            ]),
            "the roomless set is exactly the operations whose `in` carries no room_id"
        );
    }

    /// Validation-order step 5 exemption. `room.archive` is the only
    /// room-bearing operation defined over a former membership; `room.list` is
    /// the record's other exemption but carries no `room_id`, so it never
    /// reaches the stage at all.
    #[test]
    fn standing_exemption_is_exactly_room_archive() {
        let exempt: BTreeSet<&str> = every_call()
            .iter()
            .filter(|c| standing_exempt(c))
            .map(TypedCall::path)
            .collect();
        assert_eq!(exempt, BTreeSet::from(["room.archive"]));
        assert!(
            request_room(&TypedCall::RoomList(RoomList {})).is_none(),
            "room.list carries no room_id, so its exemption is structural"
        );
    }

    /// Validation-order step 6 scope: exactly the four operations the record
    /// lists, and no others. `pipe.revoke` in particular is publisher-scoped,
    /// not authority-gated.
    #[test]
    fn authority_gate_is_exactly_the_four_authority_operations() {
        let gated: BTreeSet<&str> = every_call()
            .iter()
            .filter(|c| requires_authority(c))
            .map(TypedCall::path)
            .collect();
        assert_eq!(
            gated,
            BTreeSet::from([
                "member.remove",
                "invite.mint",
                "invite.revoke",
                "invite.list"
            ])
        );
        assert!(
            !requires_authority(&TypedCall::PipeRevoke(PipeRevoke {
                room_id: rid(),
                pipe_id: PipeId::new("ab".repeat(16)),
            })),
            "pipe.revoke answers pipe_not_publisher, never insufficient_standing"
        );
    }

    /// The six paging operations, and only those, carry a [`Page`] the
    /// structural stage bounds.
    #[test]
    fn paging_scope_is_exactly_the_six_paging_operations() {
        let paged: BTreeSet<&str> = every_call()
            .iter()
            .filter(|c| request_page(c).is_some())
            .map(TypedCall::path)
            .collect();
        assert_eq!(
            paged,
            BTreeSet::from([
                "room.timeline",
                "room.archive",
                "status.history",
                "invite.list",
                "file.list",
                "pipe.list"
            ])
        );
    }

    // ------------------------------------------------------------------
    // Validation-order regressions
    // ------------------------------------------------------------------

    /// A paging call with the given limit, for each of the six.
    fn paging_calls(limit: u64) -> Vec<TypedCall> {
        let page = Page {
            cursor: Cursor::Start,
            direction: Direction::Forward,
            limit,
        };
        vec![
            TypedCall::RoomTimeline(RoomTimeline {
                room_id: rid(),
                page: page.clone(),
            }),
            TypedCall::RoomArchive(RoomArchive {
                room_id: rid(),
                page: page.clone(),
            }),
            TypedCall::StatusHistory(StatusHistory {
                room_id: rid(),
                subject_id: subject(),
                page: page.clone(),
            }),
            TypedCall::InviteList(InviteList {
                room_id: rid(),
                page: page.clone(),
            }),
            TypedCall::FileList(FileList {
                room_id: rid(),
                page: page.clone(),
            }),
            TypedCall::PipeList(PipeList {
                room_id: rid(),
                page,
            }),
        ]
    }

    /// **Step 1 outranks step 2.** All six paging operations refuse an
    /// out-of-range `limit` as `invalid_argument { field: "in.limit", bound }`
    /// on a daemon with **no subject at all** — so the bound check demonstrably
    /// precedes the subject precondition, and with it every room, standing,
    /// role, and storage access.
    ///
    /// The record puts structure first and states why: step 1 discloses only
    /// value formats and served limits, both of which `hello` already published
    /// to this very connection, so it reveals no daemon state and costs the
    /// non-oracle property nothing.
    #[tokio::test]
    async fn structural_bounds_outrank_subject_absence_for_all_six_paging_operations() {
        let dir = tempdir().unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        // No identity is created: every later stage would refuse.
        for limit in [0, limits().timeline_page_max + 1] {
            for call in paging_calls(limit) {
                let path = call.path();
                let err = dispatch(&sup, call)
                    .await
                    .expect_err("an out-of-range limit is refused");
                assert_eq!(
                    err,
                    ApiError::InvalidArgument {
                        field: "in.limit".into(),
                        reason: InvalidReason::Bound {
                            min: 1,
                            max: limits().timeline_page_max,
                        },
                    },
                    "{path} with limit {limit}"
                );
            }
        }
    }

    /// A `limit` inside the bound passes step 1 and then meets step 2, proving
    /// the previous test measured the bound rather than a blanket refusal.
    #[tokio::test]
    async fn an_in_range_limit_reaches_the_subject_stage() {
        let dir = tempdir().unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        for call in paging_calls(32) {
            let path = call.path();
            let err = dispatch(&sup, call).await.expect_err("no subject exists");
            assert_eq!(err, ApiError::SubjectAbsent, "{path}");
        }
    }

    /// **Step 2 outranks step 4.** Every room-bearing operation answers
    /// `subject_absent` on a subject-less daemon, whatever room it names —
    /// unknown, well-formed, or unparseable. Error-code selection is therefore
    /// not a room probe for a caller with no subject.
    #[tokio::test]
    async fn subject_absence_outranks_room_state_and_is_not_a_membership_oracle() {
        let dir = tempdir().unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        for spelling in [
            format!("blake3:{}", "00".repeat(32)),
            format!("blake3:{}", "ff".repeat(32)),
            "not-a-room-id".to_owned(),
        ] {
            for mut call in every_call() {
                if request_room(&call).is_none() {
                    continue;
                }
                // Re-point the call at this room spelling.
                let room = RoomId::new(spelling.clone());
                call = repoint(call, room);
                let path = call.path();
                let err = dispatch(&sup, call).await.expect_err("no subject exists");
                assert_eq!(err, ApiError::SubjectAbsent, "{path} with room {spelling}");
            }
        }
    }

    /// Rebuild a call against a different room id, for the non-oracle sweep.
    fn repoint(call: TypedCall, room_id: RoomId) -> TypedCall {
        match call {
            TypedCall::RoomActivate(mut r) => {
                r.room_id = room_id;
                TypedCall::RoomActivate(r)
            }
            TypedCall::RoomDeactivate(mut r) => {
                r.room_id = room_id;
                TypedCall::RoomDeactivate(r)
            }
            TypedCall::RoomLeave(mut r) => {
                r.room_id = room_id;
                TypedCall::RoomLeave(r)
            }
            TypedCall::RoomTimeline(mut r) => {
                r.room_id = room_id;
                TypedCall::RoomTimeline(r)
            }
            TypedCall::RoomMembers(mut r) => {
                r.room_id = room_id;
                TypedCall::RoomMembers(r)
            }
            TypedCall::RoomArchive(mut r) => {
                r.room_id = room_id;
                TypedCall::RoomArchive(r)
            }
            TypedCall::RoomPeers(mut r) => {
                r.room_id = room_id;
                TypedCall::RoomPeers(r)
            }
            TypedCall::MemberRemove(mut r) => {
                r.room_id = room_id;
                TypedCall::MemberRemove(r)
            }
            TypedCall::InviteMint(mut r) => {
                r.room_id = room_id;
                TypedCall::InviteMint(r)
            }
            TypedCall::InviteList(mut r) => {
                r.room_id = room_id;
                TypedCall::InviteList(r)
            }
            TypedCall::InviteRevoke(mut r) => {
                r.room_id = room_id;
                TypedCall::InviteRevoke(r)
            }
            TypedCall::MessageSend(mut r) => {
                r.room_id = room_id;
                TypedCall::MessageSend(r)
            }
            TypedCall::StatusPost(mut r) => {
                r.room_id = room_id;
                TypedCall::StatusPost(r)
            }
            TypedCall::StatusHistory(mut r) => {
                r.room_id = room_id;
                TypedCall::StatusHistory(r)
            }
            TypedCall::FileShare(mut r) => {
                r.room_id = room_id;
                TypedCall::FileShare(r)
            }
            TypedCall::FileList(mut r) => {
                r.room_id = room_id;
                TypedCall::FileList(r)
            }
            TypedCall::FileFetch(mut r) => {
                r.room_id = room_id;
                TypedCall::FileFetch(r)
            }
            TypedCall::FileRead(mut r) => {
                r.room_id = room_id;
                TypedCall::FileRead(r)
            }
            TypedCall::PipePublish(mut r) => {
                r.room_id = room_id;
                TypedCall::PipePublish(r)
            }
            TypedCall::PipeList(mut r) => {
                r.room_id = room_id;
                TypedCall::PipeList(r)
            }
            TypedCall::PipeConnect(mut r) => {
                r.room_id = room_id;
                TypedCall::PipeConnect(r)
            }
            TypedCall::PipeRevoke(mut r) => {
                r.room_id = room_id;
                TypedCall::PipeRevoke(r)
            }
            TypedCall::StreamSubscribe(mut r) => {
                r.room_id = room_id;
                TypedCall::StreamSubscribe(r)
            }
            TypedCall::StreamUnsubscribe(mut r) => {
                r.room_id = room_id;
                TypedCall::StreamUnsubscribe(r)
            }
            TypedCall::StreamResync(mut r) => {
                r.room_id = room_id;
                TypedCall::StreamResync(r)
            }
            other => other,
        }
    }

    /// **Step 4 is the non-oracle answer, and an unparseable room id is one of
    /// its causes.** With a subject present, an unknown room and a room id this
    /// daemon cannot even parse produce the *same* `room_not_available`
    /// echoing what the caller sent. A distinct `invalid_argument` for the
    /// malformed spelling would be a second, distinguishable answer for "no
    /// such room" — and identifiers are opaque, so there is no published format
    /// to refuse against.
    #[tokio::test]
    async fn an_unparseable_room_id_is_room_not_available_not_invalid_argument() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        for spelling in ["not-a-room-id", &format!("blake3:{}", "cd".repeat(32))] {
            let room_id = RoomId::new(spelling);
            let err = dispatch(
                &sup,
                TypedCall::RoomTimeline(RoomTimeline {
                    room_id: room_id.clone(),
                    page: page(),
                }),
            )
            .await
            .expect_err("neither room is available");
            assert_eq!(
                err,
                ApiError::RoomNotAvailable { room_id },
                "spelling {spelling}"
            );
        }
    }

    /// Build an authority with one room, plus a joined member on its own
    /// daemon. Returns `(authority, member, room_id)`.
    async fn authority_and_member() -> (
        tempfile::TempDir,
        tempfile::TempDir,
        RoomSupervisor,
        RoomSupervisor,
        String,
    ) {
        let owner_dir = tempdir().unwrap();
        crate::identity::create(owner_dir.path()).unwrap();
        let owner = RoomSupervisor::new(owner_dir.path().to_path_buf(), true).unwrap();
        let room_id = owner.create_room("Pipeline").unwrap();
        let opened = owner.open_room(&room_id, &[]).await.unwrap();
        let owner_addr = opened["endpoint"]["addr"].as_str().unwrap().to_owned();

        let member_dir = tempdir().unwrap();
        let member_profile = crate::identity::create(member_dir.path()).unwrap();
        let member = RoomSupervisor::new(member_dir.path().to_path_buf(), true).unwrap();
        let ticket = owner
            .create_invite(&room_id, &member_profile.identity_id, "member", None)
            .await
            .unwrap();
        member
            .join_room(&ticket, None, std::slice::from_ref(&owner_addr))
            .await
            .unwrap();
        (owner_dir, member_dir, owner, member, room_id)
    }

    /// **Step 5 outranks step 6.** A former member that *was* the room's
    /// authority-gated caller answers `membership_ended`, never
    /// `insufficient_standing` — the role code is defined only for active
    /// members, so reaching it from an ended membership would report the wrong
    /// reason and leak that the caller once held a role.
    #[tokio::test(flavor = "multi_thread")]
    async fn standing_outranks_role_for_an_authority_gated_operation() {
        let (_od, _md, owner, member, room_id) = authority_and_member().await;
        member.open_room(&room_id, &[]).await.unwrap();
        member.leave_room(&room_id).await.unwrap();

        let err = dispatch(
            &member,
            TypedCall::InviteList(InviteList {
                room_id: RoomId::new(&room_id),
                page: page(),
            }),
        )
        .await
        .expect_err("a former member cannot enumerate invites");
        assert_eq!(
            err,
            ApiError::MembershipEnded {
                room_id: RoomId::new(&room_id),
                standing: Standing::Left,
            },
            "standing precedes role"
        );
        owner.close_room(&room_id).await.unwrap();
    }

    /// **Step 6 outranks step 7.** A plain active member calling an
    /// authority-gated operation gets `insufficient_standing` naming the role
    /// tokens — not the operation's own semantic code, which would disclose
    /// room state to a caller who may not read it.
    #[tokio::test(flavor = "multi_thread")]
    async fn role_outranks_operation_semantics() {
        let (_od, _md, owner, member, room_id) = authority_and_member().await;
        // `member.remove` against a subject that is not a member would answer
        // `member_unknown` at step 7 — but the caller is not an authority, so
        // step 6 refuses first.
        let err = dispatch(
            &member,
            TypedCall::MemberRemove(MemberRemove {
                room_id: RoomId::new(&room_id),
                subject_id: subject(),
            }),
        )
        .await
        .expect_err("a member may not remove");
        assert_eq!(
            err,
            ApiError::InsufficientStanding {
                room_id: RoomId::new(&room_id),
                required: Role::Authority,
                held: Role::Member,
            }
        );
        owner.close_room(&room_id).await.unwrap();
    }

    /// The step-1 structural check on `member.remove`'s `subject_id` runs
    /// before the room, standing, and role stages, so a malformed identity is
    /// never answered with an authorization verdict.
    #[tokio::test]
    async fn a_malformed_member_remove_subject_is_structural_before_authorization() {
        let dir = tempdir().unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let err = dispatch(
            &sup,
            TypedCall::MemberRemove(MemberRemove {
                room_id: rid(),
                subject_id: SubjectId::new("not-an-identity"),
            }),
        )
        .await
        .expect_err("a malformed subject is refused structurally");
        assert_eq!(
            err,
            ApiError::InvalidArgument {
                field: "in.subject_id".into(),
                reason: InvalidReason::Format,
            }
        );
    }

    /// `room.archive` is exempt from the standing stage, and checks the
    /// converse itself: an active membership is `room_still_active`, never the
    /// generic `invalid_argument` an earlier revision returned.
    #[tokio::test]
    async fn room_archive_on_an_active_membership_is_room_still_active() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Still Active").unwrap();
        let err = dispatch(
            &sup,
            TypedCall::RoomArchive(RoomArchive {
                room_id: RoomId::new(&room_id),
                page: page(),
            }),
        )
        .await
        .expect_err("the caller still belongs");
        assert_eq!(
            err,
            ApiError::RoomStillActive {
                room_id: RoomId::new(&room_id),
            }
        );
    }

    /// The standing stage refuses a left membership on a room-bearing
    /// operation that is not exempt, and `room.archive` succeeds on the same
    /// room — the exemption is real in both directions.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_left_membership_is_refused_everywhere_except_the_archive() {
        let (_od, _md, owner, member, room_id) = authority_and_member().await;
        member.open_room(&room_id, &[]).await.unwrap();
        member.leave_room(&room_id).await.unwrap();
        let api_room = RoomId::new(&room_id);

        let err = dispatch(
            &member,
            TypedCall::RoomTimeline(RoomTimeline {
                room_id: api_room.clone(),
                page: page(),
            }),
        )
        .await
        .expect_err("a former member reads the archive, not the timeline");
        assert_eq!(
            err,
            ApiError::MembershipEnded {
                room_id: api_room.clone(),
                standing: Standing::Left,
            }
        );

        let reply = dispatch(
            &member,
            TypedCall::RoomArchive(RoomArchive {
                room_id: api_room.clone(),
                page: page(),
            }),
        )
        .await
        .expect("the archive is defined over a former membership");
        let TypedReply::RoomArchive(archive) = reply else {
            panic!("wrong reply");
        };
        assert_eq!(archive.standing, Standing::Left);
        owner.close_room(&room_id).await.unwrap();
    }

    // ------------------------------------------------------------------
    // Typed honesty: no fabricated fact survives
    // ------------------------------------------------------------------

    /// `room.create` refuses a name outside the record's stated bounds with
    /// `room_name_invalid` carrying the closed `reason` variant — a `bound` arm
    /// for length and a `format` arm for whitespace-only.
    #[tokio::test]
    async fn room_create_refuses_an_invalid_name_with_its_own_code() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        for (name, expected) in [
            ("", InvalidReason::Bound { min: 1, max: 128 }),
            ("   ", InvalidReason::Format),
            (&"x".repeat(129), InvalidReason::Bound { min: 1, max: 128 }),
        ] {
            let err = dispatch(
                &sup,
                TypedCall::RoomCreate(RoomCreate { name: name.into() }),
            )
            .await
            .expect_err("the name fails the stated bounds");
            assert_eq!(
                err,
                ApiError::RoomNameInvalid { reason: expected },
                "name {name:?}"
            );
        }
    }

    /// `message.send` answers its own `message_too_large` carrying both counts,
    /// never a generic `invalid_argument`.
    #[tokio::test]
    async fn message_send_over_the_limit_is_message_too_large() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Too Large").unwrap();
        let limit = limits().max_message_body_bytes;
        let err = dispatch(
            &sup,
            TypedCall::MessageSend(MessageSend {
                room_id: RoomId::new(&room_id),
                body: "x".repeat(limit as usize + 1),
            }),
        )
        .await
        .expect_err("the body exceeds the served limit");
        assert_eq!(
            err,
            ApiError::MessageTooLarge {
                declared_bytes: limit + 1,
                limit_bytes: limit,
            }
        );
    }

    /// `invite.mint` refuses a role it may not grant with `role_not_grantable`
    /// naming the requested role.
    #[tokio::test]
    async fn invite_mint_refuses_an_ungrantable_role_by_name() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Grant").unwrap();
        let err = dispatch(
            &sup,
            TypedCall::InviteMint(InviteMint {
                room_id: RoomId::new(&room_id),
                subject_id: subject(),
                role: Role::Authority,
                expires_at: Timestamp::new(
                    time::OffsetDateTime::from_unix_timestamp(
                        (crate::now_ms() / 1000) as i64 + 3600,
                    )
                    .unwrap(),
                ),
            }),
        )
        .await
        .expect_err("authority is not grantable");
        assert_eq!(
            err,
            ApiError::RoleNotGrantable {
                requested: Role::Authority
            }
        );
    }

    /// A mutation reply's `at` is the instant the event's author **signed**,
    /// so it equals the instant the same event carries on the timeline. An
    /// earlier revision served the wall clock at reply time, which is a
    /// different number for the same fact.
    #[tokio::test]
    async fn a_mutation_reply_serves_the_signed_instant_not_the_wall_clock() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Signed Instant").unwrap();
        let api_room = RoomId::new(&room_id);
        // Authoring publishes through a live node in this build (see
        // `LIVENESS_GATED`), so the room is activated first.
        sup.activate_room(&room_id, &[]).await.unwrap();

        let TypedReply::MessageSend(sent) = dispatch(
            &sup,
            TypedCall::MessageSend(MessageSend {
                room_id: api_room.clone(),
                body: "hello".into(),
            }),
        )
        .await
        .expect("message.send") else {
            panic!("wrong reply");
        };
        let TypedReply::RoomTimeline(timeline) = dispatch(
            &sup,
            TypedCall::RoomTimeline(RoomTimeline {
                room_id: api_room,
                page: page(),
            }),
        )
        .await
        .expect("room.timeline") else {
            panic!("wrong reply");
        };
        let committed = timeline
            .events
            .iter()
            .find(|e| e.event_id == sent.event_id)
            .expect("the authored event is on the timeline");
        assert_eq!(
            sent.at, committed.at,
            "the reply and the timeline serve one signed instant"
        );
        assert_eq!(sent.pos, committed.pos, "and one position space");
    }

    /// `room.members` dates every row from a signed join. An earlier revision
    /// fell back to the Unix epoch, which reads as a real instant in 1970.
    #[tokio::test]
    async fn room_members_dates_every_row_from_a_signed_join() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Dated").unwrap();
        let TypedReply::RoomMembers(members) = dispatch(
            &sup,
            TypedCall::RoomMembers(RoomMembers {
                room_id: RoomId::new(&room_id),
            }),
        )
        .await
        .expect("room.members") else {
            panic!("wrong reply");
        };
        assert!(!members.members.is_empty());
        for row in &members.members {
            assert_ne!(
                row.joined_at,
                Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
                "no row is dated to the epoch"
            );
        }
    }

    /// A room row's `last_event` reports the newest **committed** event. An
    /// outstanding invitation is not a committed event and must not win the
    /// recency max — an earlier revision let it, then dropped the kindless
    /// winner, reporting `absent` for a room with a full timeline.
    #[tokio::test]
    async fn a_pending_invitation_does_not_hide_a_rooms_recency() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Recency").unwrap();
        sup.activate_room(&room_id, &[]).await.unwrap();
        dispatch(
            &sup,
            TypedCall::MessageSend(MessageSend {
                room_id: RoomId::new(&room_id),
                body: "the newest committed event".into(),
            }),
        )
        .await
        .expect("message.send");
        // A later invitation is stored but never committed to the timeline.
        sup.create_invite(&room_id, subject().as_str(), "member", Some("1h"))
            .await
            .expect("invite.create");

        let TypedReply::RoomList(list) = dispatch(&sup, TypedCall::RoomList(RoomList {}))
            .await
            .expect("room.list")
        else {
            panic!("wrong reply");
        };
        let row = list
            .rooms
            .iter()
            .find(|r| r.room_id.as_str() == room_id)
            .expect("the room is listed");
        assert!(
            matches!(
                row.last_event,
                LastEvent::Present {
                    kind: EventKind::Message,
                    ..
                }
            ),
            "the newest committed event is the message, not the invitation: {:?}",
            row.last_event
        );
    }

    /// Capabilities are the operations the caller may invoke **right now**.
    /// The three liveness-gated operations appear only on a live room; the
    /// authority-only four only for an authority; `pipe.revoke` is present for
    /// any active member because it is publisher-scoped, not role-scoped; and a
    /// former membership advertises `room.archive` alone.
    #[test]
    fn capabilities_advertise_exactly_what_would_not_be_refused() {
        use CapabilityToken::*;
        let authority_only = [InviteMint, InviteList, InviteRevoke, MemberRemove];

        let offline = room_capabilities(Standing::Active, Role::Member, false);
        let live = room_capabilities(Standing::Active, Role::Member, true);
        for token in LIVENESS_GATED {
            assert!(
                !offline.contains(&token),
                "{token:?} is refused on a non-live room in this build"
            );
            assert!(live.contains(&token), "{token:?} is reachable when live");
        }
        // Naturally idempotent in both directions, so never liveness-gated.
        for token in [RoomActivate, RoomDeactivate] {
            assert!(offline.contains(&token) && live.contains(&token));
        }
        // Reads that work identically whether or not the room is live.
        for token in [RoomTimeline, RoomMembers, FileList, PipeList, FileRead] {
            assert!(
                offline.contains(&token) && live.contains(&token),
                "{token:?} reads committed state and needs no transport"
            );
        }
        for token in authority_only {
            assert!(
                !live.contains(&token),
                "{token:?} is authority-only and must be absent for a member"
            );
        }
        assert!(
            live.contains(&PipeRevoke),
            "pipe.revoke is publisher-scoped, not authority-gated"
        );
        let authority = room_capabilities(Standing::Active, Role::Authority, true);
        for token in authority_only {
            assert!(authority.contains(&token), "{token:?} is an authority's");
        }
        // `room.archive` is advertised only where it would succeed.
        assert!(!live.contains(&RoomArchive));
        for standing in [Standing::Left, Standing::Removed] {
            assert_eq!(
                room_capabilities(standing, Role::Authority, true),
                vec![RoomArchive],
                "a former membership reaches exactly one room-scoped operation"
            );
        }
    }

    /// Every advertised capability is one this daemon would actually serve.
    /// The array is composed from `(standing, role, live)`, and every token in
    /// it is a room-scoped operation — a check that catches a token added to
    /// the list without a corresponding gate.
    #[test]
    fn every_advertised_capability_is_room_scoped() {
        let roomless: BTreeSet<&str> = every_call()
            .iter()
            .filter(|c| request_room(c).is_none())
            .map(TypedCall::path)
            .collect();
        for standing in [Standing::Active, Standing::Left, Standing::Removed] {
            for role in [Role::Member, Role::Authority] {
                for live in [false, true] {
                    for token in room_capabilities(standing, role, live) {
                        let name = serde_json::to_value(token).expect("a token serializes");
                        let name = name.as_str().expect("tokens encode as strings").to_owned();
                        assert!(
                            !roomless.contains(name.as_str()),
                            "{name} is not an answer about one room"
                        );
                    }
                }
            }
        }
    }

    /// Every typed refusal this module can construct carries a **real** value
    /// in every identifier field: the record's error schema fixes those fields
    /// and a client branches on them, so `""` is a schema satisfied and a fact
    /// invented.
    #[test]
    fn no_typed_error_carries_an_empty_identifier() {
        let room = RoomId::new("blake3:room");
        let file = FileId::new("file_00");
        let pipe = PipeId::new("ab".repeat(16));
        let errors = vec![
            ApiError::RoomNotAvailable {
                room_id: room.clone(),
            },
            ApiError::RoomNotLive {
                room_id: room.clone(),
            },
            ApiError::PolicyRefused {
                room_id: room.clone(),
            },
            ApiError::RoomStillActive {
                room_id: room.clone(),
            },
            ApiError::MembershipEnded {
                room_id: room.clone(),
                standing: Standing::Left,
            },
            ApiError::FileUnknown {
                file_id: file.clone(),
            },
            ApiError::FileNotFetched {
                file_id: file.clone(),
            },
            ApiError::PipeUnknown {
                pipe_id: pipe.clone(),
            },
            ApiError::PipeNotPublisher {
                pipe_id: pipe.clone(),
            },
            ApiError::SubscriptionUnknown {
                room_id: room.clone(),
            },
            core_to_api(CoreError::internal("a store read failed")),
            core_to_api_room(CoreError::internal("a store read failed"), &room),
            core_to_api_room(
                CoreError::new(ErrorKind::RoomUnknown, "no such room"),
                &room,
            ),
            redemption_error(CoreError::new(ErrorKind::BadTicket, "forged"), None),
        ];
        for err in errors {
            let value = serde_json::to_value(&err).expect("errors serialize");
            assert_no_empty_ids(&value, &err);
            assert_no_nulls(&value, &format!("{err:?}"));
        }
    }

    fn assert_no_empty_ids(value: &serde_json::Value, err: &ApiError) {
        const ID_KEYS: [&str; 7] = [
            "room_id",
            "subject_id",
            "device_id",
            "file_id",
            "pipe_id",
            "invite_id",
            "connection_id",
        ];
        if let serde_json::Value::Object(map) = value {
            for key in ID_KEYS {
                if let Some(serde_json::Value::String(s)) = map.get(key) {
                    assert!(!s.is_empty(), "{err:?} carries an empty {key}");
                }
            }
        }
    }

    fn assert_no_nulls(value: &serde_json::Value, label: &str) {
        match value {
            serde_json::Value::Null => panic!("{label} encodes a JSON null"),
            serde_json::Value::Object(map) => {
                for v in map.values() {
                    assert_no_nulls(v, label);
                }
            }
            serde_json::Value::Array(items) => {
                for v in items {
                    assert_no_nulls(v, label);
                }
            }
            _ => {}
        }
    }

    /// `invite.redeem` is the only operation a non-member can reach, so every
    /// failure is a fieldless-or-instant redemption-side code. In particular a
    /// capability for a room this daemon does not hold must not surface as
    /// `room_not_available` naming that room.
    #[tokio::test]
    async fn invite_redeem_never_answers_a_room_scoped_code() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        for capability in ["", "not-a-ticket", "roomticket1qqq"] {
            let err = dispatch(
                &sup,
                TypedCall::InviteRedeem(InviteRedeem {
                    capability: capability.into(),
                }),
            )
            .await
            .expect_err("no capability verifies");
            assert!(
                matches!(
                    err,
                    ApiError::CapabilityInvalid
                        | ApiError::CapabilityExpired { .. }
                        | ApiError::CapabilityRevoked { .. }
                        | ApiError::CapabilityRedeemed { .. }
                ),
                "capability {capability:?} answered {err:?}"
            );
        }
    }

    /// The `stream.*` refusal this dispatcher produces names the room the
    /// caller asked about rather than an empty id.
    #[tokio::test]
    async fn a_stream_refusal_names_the_room_it_was_asked_about() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Streamed").unwrap();
        let api_room = RoomId::new(&room_id);
        let err = dispatch(
            &sup,
            TypedCall::StreamUnsubscribe(StreamUnsubscribe {
                room_id: api_room.clone(),
            }),
        )
        .await
        .expect_err("no connection-scoped subscription exists here");
        assert_eq!(err, ApiError::SubscriptionUnknown { room_id: api_room });
    }

    // ------------------------------------------------------------------
    // Preserved behavior
    // ------------------------------------------------------------------

    fn first_page(room_id: &str) -> InviteList {
        InviteList {
            room_id: RoomId::new(room_id),
            page: Page {
                cursor: Cursor::Start,
                direction: Direction::Forward,
                limit: 32,
            },
        }
    }

    #[tokio::test]
    async fn invite_list_folds_outstanding_then_redeemed_without_capability_material() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id_str = sup.create_room("Invite Index").unwrap();
        let room_id: IrohRoomId = room_id_str.parse().unwrap();
        let invitee_identity = iroh_rooms::identity::SigningKey::generate();
        let invitee_device = iroh_rooms::identity::SigningKey::generate();
        let capability = sup
            .create_invite(
                &room_id_str,
                &invitee_identity.identity_key().to_string(),
                "member",
                Some("1h"),
            )
            .await
            .unwrap();

        let TypedReply::InviteList(outstanding) =
            dispatch(&sup, TypedCall::InviteList(first_page(&room_id_str)))
                .await
                .unwrap()
        else {
            panic!("wrong reply");
        };
        assert_eq!(outstanding.invites.len(), 1);
        assert_eq!(
            outstanding.invites[0].redeemability,
            Redeemability::Outstanding
        );
        let encoded = serde_json::to_value(&outstanding).unwrap();
        assert!(
            encoded["invites"][0].get("capability").is_none(),
            "the authority index never returns the bearer capability"
        );
        assert!(
            encoded["invites"][0].get("capability_hash").is_none(),
            "the authority index never returns capability verification material"
        );

        let ticket: iroh_rooms::room::RoomInviteTicket = capability.parse().unwrap();
        let mut heads = sup.open_store().unwrap().heads(&room_id).unwrap();
        heads.truncate(iroh_rooms::events::constants::MAX_PREV_EVENTS);
        let binding = iroh_rooms::identity::DeviceBinding::create(
            &room_id,
            &invitee_identity,
            invitee_device.device_key(),
        );
        let joined = iroh_rooms::room::build_member_joined(
            &invitee_identity,
            &invitee_device,
            &room_id,
            &ticket.invite_id,
            &ticket.capability_secret,
            &ticket.role,
            binding,
            None,
            &heads,
            crate::now_ms(),
        );
        let validated = iroh_rooms::events::validate_wire_bytes(
            &joined.to_bytes(),
            &iroh_rooms::events::ValidationContext::for_room(room_id),
        )
        .unwrap();
        sup.open_store().unwrap().insert(&validated).unwrap();

        let TypedReply::InviteList(redeemed) =
            dispatch(&sup, TypedCall::InviteList(first_page(&room_id_str)))
                .await
                .unwrap()
        else {
            panic!("wrong reply");
        };
        assert_eq!(redeemed.invites[0].redeemability, Redeemability::Redeemed);
    }

    #[tokio::test]
    async fn room_deactivate_is_naturally_idempotent_for_an_active_member() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Idempotent Deactivate").unwrap();
        sup.activate_room(&room_id, &[]).await.unwrap();
        let call = || {
            TypedCall::RoomDeactivate(RoomDeactivate {
                room_id: RoomId::new(&room_id),
            })
        };

        let TypedReply::RoomDeactivate(first) = dispatch(&sup, call()).await.unwrap() else {
            panic!("wrong reply");
        };
        let TypedReply::RoomDeactivate(second) = dispatch(&sup, call()).await.unwrap() else {
            panic!("wrong reply");
        };

        assert_eq!(first, second);
        assert!(!first.live);
        assert!(sup.open_rooms().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn room_deactivate_rejects_an_ended_membership_before_idempotency() {
        let (_od, _md, owner, member, room_id) = authority_and_member().await;
        member.open_room(&room_id, &[]).await.unwrap();
        member.leave_room(&room_id).await.unwrap();

        let err = dispatch(
            &member,
            TypedCall::RoomDeactivate(RoomDeactivate {
                room_id: RoomId::new(&room_id),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(
            err,
            ApiError::MembershipEnded {
                room_id: RoomId::new(&room_id),
                standing: Standing::Left,
            }
        );

        owner.close_room(&room_id).await.unwrap();
    }

    #[tokio::test]
    async fn member_remove_and_pipe_release_map_semantic_errors_exactly() {
        let dir = tempdir().unwrap();
        let profile = crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Exact Errors").unwrap();

        let authority = dispatch(
            &sup,
            TypedCall::MemberRemove(MemberRemove {
                room_id: RoomId::new(&room_id),
                subject_id: SubjectId::new(&profile.identity_id),
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            authority,
            ApiError::AuthorityCannotBeRemoved { .. }
        ));

        let unknown = dispatch(
            &sup,
            TypedCall::MemberRemove(MemberRemove {
                room_id: RoomId::new(&room_id),
                subject_id: subject(),
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(unknown, ApiError::MemberUnknown { .. }));

        let connection_id = "127.0.0.1:65535".to_owned();
        assert_eq!(
            dispatch(
                &sup,
                TypedCall::PipeRelease(PipeRelease {
                    connection_id: connection_id.clone(),
                })
            )
            .await
            .unwrap_err(),
            ApiError::ConnectionUnknown { connection_id }
        );
    }

    #[tokio::test]
    async fn pipe_operations_preflight_room_access_before_runtime_or_policy() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        sup.create_room("Known Room").unwrap();
        let unknown_room = RoomId::new(format!("blake3:{}", "de".repeat(32)));

        // A non-loopback target would be `pipe_target_refused` at step 7 — but
        // the room is not available, and step 4 outranks it.
        let publish = dispatch(
            &sup,
            TypedCall::PipePublish(PipePublish {
                room_id: unknown_room.clone(),
                target: Target {
                    host: "203.0.113.10".into(),
                    port: 4444,
                },
                audience: Audience::Room,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(
            publish,
            ApiError::RoomNotAvailable {
                room_id: unknown_room.clone()
            }
        );

        let connect = dispatch(
            &sup,
            TypedCall::PipeConnect(PipeConnect {
                room_id: unknown_room.clone(),
                pipe_id: PipeId::new("ab".repeat(16)),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(
            connect,
            ApiError::RoomNotAvailable {
                room_id: unknown_room
            }
        );
    }

    /// A loopback-policy refusal still names the rejected target verbatim,
    /// once the room stages have passed.
    #[tokio::test]
    async fn pipe_publish_refuses_a_non_loopback_target_verbatim() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Loopback Policy").unwrap();
        let target = Target {
            host: "192.168.1.10".into(),
            port: 4444,
        };
        let err = dispatch(
            &sup,
            TypedCall::PipePublish(PipePublish {
                room_id: RoomId::new(&room_id),
                target: target.clone(),
                audience: Audience::Room,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err, ApiError::PipeTargetRefused { target });
    }

    /// An outstanding invitation is **not** a member. The fold yields an
    /// `Invited` row with no device binding and no join to date, so including
    /// it would either fabricate a `joined_at` or — as an earlier revision did
    /// — fail the entire roster with `membership_unresolved` the moment any
    /// invite is outstanding, taking `stream.subscribe` and `stream.resync`
    /// with it (both authorize through `room.members`).
    #[tokio::test]
    async fn an_outstanding_invitation_is_not_a_member_and_does_not_break_the_roster() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Invited Not Member").unwrap();
        let api_room = RoomId::new(&room_id);
        let invitee = subject();

        async fn roster(sup: &RoomSupervisor, room_id: &RoomId) -> Vec<MemberRow> {
            let call = TypedCall::RoomMembers(RoomMembers {
                room_id: room_id.clone(),
            });
            let TypedReply::RoomMembers(out) = dispatch(sup, call).await.expect("room.members")
            else {
                panic!("wrong reply");
            };
            out.members
        }

        let before = roster(&sup, &api_room).await;
        assert_eq!(before.len(), 1, "the authority is the only member");

        sup.create_invite(&room_id, invitee.as_str(), "member", Some("1h"))
            .await
            .expect("invite.create");

        let after = roster(&sup, &api_room).await;
        assert_eq!(
            after, before,
            "an outstanding invitation changes no roster row"
        );
        assert!(
            after.iter().all(|m| m.subject_id != invitee),
            "the invitee is not a member until it redeems"
        );

        // `room.list`'s member_count is the same set, so the count and the
        // roster can never disagree.
        let TypedReply::RoomList(list) = dispatch(&sup, TypedCall::RoomList(RoomList {}))
            .await
            .expect("room.list")
        else {
            panic!("wrong reply");
        };
        let row = list
            .rooms
            .iter()
            .find(|r| r.room_id == api_room)
            .expect("the room is listed");
        assert_eq!(row.member_count, after.len() as u64);
    }

    /// `invite.mint` serves the expiry the capability was **signed** with, so
    /// the reply and the `invite.list` row for the same invite agree. Echoing
    /// the requested instant promised an expiry the capability does not carry.
    #[tokio::test]
    async fn invite_mint_serves_the_signed_expiry_not_the_requested_one() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Signed Expiry").unwrap();
        let api_room = RoomId::new(&room_id);
        // A requested instant with a fractional second, so a truncating
        // conversion is observable.
        let requested = Timestamp::new(
            time::OffsetDateTime::from_unix_timestamp((crate::now_ms() / 1000) as i64 + 3600)
                .unwrap(),
        );
        let TypedReply::InviteMint(minted) = dispatch(
            &sup,
            TypedCall::InviteMint(InviteMint {
                room_id: api_room.clone(),
                subject_id: subject(),
                role: Role::Member,
                expires_at: requested,
            }),
        )
        .await
        .expect("invite.mint") else {
            panic!("wrong reply");
        };
        let ticket: iroh_rooms::room::RoomInviteTicket =
            minted.capability.trim().parse().expect("the ticket parses");
        let signed = proj::ts(ticket.expires_at.expect("the ticket carries an expiry"))
            .expect("the signed expiry converts");
        assert_eq!(
            minted.expires_at, signed,
            "the reply serves the capability's own signed expiry"
        );

        let TypedReply::InviteList(listed) =
            dispatch(&sup, TypedCall::InviteList(first_page(&room_id)))
                .await
                .expect("invite.list")
        else {
            panic!("wrong reply");
        };
        assert_eq!(
            listed.invites[0].expires_at, minted.expires_at,
            "mint and list agree about one invite's expiry"
        );
    }

    /// A past expiry is refused with a bound whose `min` and `max` are the same
    /// unit — whole seconds since the epoch, the domain the refused `<ts>` sits
    /// in. An earlier revision mixed seconds and milliseconds.
    #[tokio::test]
    async fn invite_mint_expiry_bound_states_one_unit() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Past Expiry").unwrap();
        let err = dispatch(
            &sup,
            TypedCall::InviteMint(InviteMint {
                room_id: RoomId::new(&room_id),
                subject_id: subject(),
                role: Role::Member,
                expires_at: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
            }),
        )
        .await
        .expect_err("a past expiry is refused");
        let ApiError::InvalidArgument {
            field,
            reason: InvalidReason::Bound { min, max },
        } = err
        else {
            panic!("wrong error: {err:?}");
        };
        assert_eq!(field, "in.expires_at");
        let now_secs = crate::now_ms() / 1000;
        assert!(
            min > now_secs - 5 && min < now_secs + 5,
            "min is whole seconds since the epoch, got {min} against {now_secs}"
        );
        assert_eq!(
            max, 253_402_300_799,
            "max is the same unit as min, not milliseconds, and is the largest \
             instant a `<ts>` can represent rather than an invented ceiling"
        );
        // The advertised maximum has to be one the daemon honours: an expiry
        // below it mints, so a client can trust the range it was handed.
        assert!(
            max > crate::now_ms() / 1000,
            "an inclusive maximum in the past would refuse every expiry"
        );
    }

    /// `subject.ensure` answers the one operation error the record gives it
    /// when its store defeats it. An earlier revision passed the failure
    /// through the roomless fallback, which named `room_index_unreadable` — an
    /// index this operation never opens, and a code that says nothing about the
    /// subject that could not be persisted. Both halves of the store are
    /// covered, because either one defeats the operation on its own.
    #[tokio::test]
    async fn subject_ensure_reports_a_defeated_store_as_subject_store_unwritable() {
        // The profile is present but unreadable: neither "no subject yet" nor a
        // subject this daemon can name.
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(crate::identity::IDENTITY_FILE)).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let err = dispatch(&sup, TypedCall::SubjectEnsure(SubjectEnsure {}))
            .await
            .expect_err("an unreadable subject store is refused");
        assert!(
            matches!(err, ApiError::SubjectStoreUnwritable),
            "expected subject_store_unwritable, got {err:?}"
        );

        // The creation cannot complete: the secret half of the store is already
        // taken, so no subject can be written even though none exists to serve.
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(crate::identity::SECRET_FILE)).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let err = dispatch(&sup, TypedCall::SubjectEnsure(SubjectEnsure {}))
            .await
            .expect_err("a subject that cannot be persisted is refused");
        assert!(
            matches!(err, ApiError::SubjectStoreUnwritable),
            "expected subject_store_unwritable, got {err:?}"
        );

        // And nothing was fabricated: a store that refused the write serves no
        // subject afterwards either.
        assert!(
            crate::identity::load_profile(dir.path())
                .ok()
                .flatten()
                .is_none(),
            "a refused subject.ensure must not leave a subject behind"
        );
    }

    /// `transfer.cancel` names the transfer the caller asked about with its own
    /// distinctive code. This build tracks no transfers by `op_id`, so no
    /// `transfer_op_id` names one for this principal — which is what
    /// `transfer_unknown` states, and it is true.
    #[tokio::test]
    async fn transfer_cancel_answers_its_own_code_naming_the_transfer() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let transfer_op_id = OpId::new("op-that-never-started");
        let err = dispatch(
            &sup,
            TypedCall::TransferCancel(TransferCancel {
                transfer_op_id: transfer_op_id.clone(),
            }),
        )
        .await
        .expect_err("no transfer is tracked");
        assert_eq!(err, ApiError::TransferUnknown { transfer_op_id });
    }

    /// `daemon.stop` skips the subject stage. Refusing it would be a refusal
    /// that still acts: the engine sequences teardown before dispatch runs, so
    /// a `subject_absent` reply would be paired with a daemon that exits.
    #[tokio::test]
    async fn daemon_stop_does_not_require_a_subject() {
        let dir = tempdir().unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let reply = dispatch(&sup, TypedCall::DaemonStop(DaemonStop {}))
            .await
            .expect("daemon.stop needs no subject");
        let TypedReply::DaemonStop(out) = reply else {
            panic!("wrong reply");
        };
        assert!(out.stopping);
    }

    /// **Agent-ness is derived from posting, not from a membership row.** The
    /// record: "An agent is a member that has authored at least one
    /// `status.post` event ... it is a classification, not a permission, so it
    /// is not a `role` and appears in no membership row."
    ///
    /// The room's authority holds the upstream `Admin` role, never `Agent`, so
    /// under the old role-based derivation it could never appear in the fleet
    /// no matter how many statuses it posted. It posts one here and must.
    #[tokio::test]
    async fn fleet_derives_agents_from_posted_status_not_from_a_role() {
        let dir = tempdir().unwrap();
        let profile = crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Fleet Derivation").unwrap();
        sup.activate_room(&room_id, &[]).await.unwrap();
        let api_room = RoomId::new(&room_id);

        // Before posting: a member, not an agent.
        let TypedReply::FleetList(before) = dispatch(&sup, TypedCall::FleetList(FleetList {}))
            .await
            .expect("fleet.list")
        else {
            panic!("wrong reply");
        };
        assert!(
            before.agents.is_empty(),
            "a member that has posted nothing is not an agent: {:?}",
            before.agents
        );

        dispatch(
            &sup,
            TypedCall::StatusPost(StatusPost {
                room_id: api_room,
                label: StatusLabel::Working,
                progress: Progress::Reported { percent: 40 },
            }),
        )
        .await
        .expect("status.post");

        let TypedReply::FleetList(after) = dispatch(&sup, TypedCall::FleetList(FleetList {}))
            .await
            .expect("fleet.list")
        else {
            panic!("wrong reply");
        };
        let row = after
            .agents
            .iter()
            .find(|a| a.subject_id.as_str() == profile.identity_id)
            .expect("the poster is now an agent, whatever its role");
        assert!(
            matches!(
                row.latest_status,
                LatestStatus::Present {
                    label: StatusLabel::Working,
                    ..
                }
            ),
            "the fleet row carries the status that made it an agent: {:?}",
            row.latest_status
        );
    }

    /// A `"host:port"` string becomes a `target` by parsing, never by
    /// splitting on the last colon and defaulting a bad port to `0`.
    #[test]
    fn a_local_endpoint_is_parsed_not_split() {
        assert_eq!(
            parse_target("127.0.0.1:4321"),
            Some(Target {
                host: "127.0.0.1".into(),
                port: 4321
            })
        );
        assert_eq!(
            parse_target("[::1]:4321"),
            Some(Target {
                host: "::1".into(),
                port: 4321
            })
        );
        // No `port: 0` is invented for a value that is not an address.
        assert_eq!(parse_target("not-an-address"), None);
        assert_eq!(parse_target("127.0.0.1:not-a-port"), None);
    }
}
