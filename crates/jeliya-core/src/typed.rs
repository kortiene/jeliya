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
//! # Paging and positions
//!
//! The v2 record fixes one continuation mechanism: every paging operation
//! takes a required [`Page`] (`cursor`, `direction`, `limit`) and answers a
//! [`Truncated`]. Positions are the store's per-room Lamport clock (`pos`),
//! anchored at `0` for the genesis. The Iroh store's `room_tail` returns the
//! most-recent `limit` events in ascending `(lamport, event_id)` order, which
//! is exactly the v2 position space; cursor/direction paging is applied over
//! that space here.

use std::collections::{BTreeMap, BTreeSet};

use jeliya_api::*;
use serde::{Deserialize, Serialize};

use iroh_rooms::events::{Content, EventType, SignedEvent};
use iroh_rooms::room::RoomId as IrohRoomId;

use crate::error::{CoreError, CoreResult, ErrorKind};
use crate::materializer::file_handle;
use crate::projection as proj;
use crate::supervisor::RoomSupervisor;

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
        let limit = usize::try_from(page.limit).unwrap_or(usize::MAX);
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

/// The typed projection facade. Cheap to construct; borrows the supervisor.
pub struct TypedSupervisor<'a> {
    sup: &'a RoomSupervisor,
}

impl<'a> TypedSupervisor<'a> {
    /// Wrap a supervisor.
    #[must_use]
    pub fn new(sup: &'a RoomSupervisor) -> Self {
        Self { sup }
    }

    /// The underlying supervisor (for host surfaces that bypass dispatch).
    #[must_use]
    pub fn supervisor(&self) -> &'a RoomSupervisor {
        self.sup
    }

    fn parse_room(room_id: &RoomId) -> CoreResult<IrohRoomId> {
        room_id.as_str().trim().parse().map_err(|e| {
            CoreError::invalid(format!("invalid room_id (expected blake3:<hex>): {e}"))
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

    // ------------------------------------------------------------------
    // Subject and daemon
    // ------------------------------------------------------------------

    /// `subject.ensure` — establish the local subject exactly once; a second
    /// call returns the same subject with `created: false` (naturally
    /// idempotent, never an `identity_exists` refusal).
    pub fn subject_ensure(&self) -> CoreResult<SubjectEnsureOut> {
        if let Some(profile) = crate::identity::load_profile(self.sup.data_dir())? {
            return Ok(SubjectEnsureOut {
                subject_id: SubjectId::new(profile.identity_id),
                device_id: DeviceId::new(profile.device_id),
                created: false,
            });
        }
        let profile = crate::identity::create(self.sup.data_dir())?;
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
    pub fn room_create(&self, req: &RoomCreate) -> CoreResult<RoomCreateOut> {
        let name = req.name.trim();
        if name.is_empty() || name.len() > 128 {
            return Err(CoreError::new(
                ErrorKind::InvalidParams,
                "room name must be 1..=128 bytes with at least one non-whitespace",
            ));
        }
        let room_id_str = self.sup.create_room(name)?;
        let room_id: IrohRoomId = room_id_str
            .parse()
            .map_err(|e| CoreError::internal(format!("fresh room id does not parse: {e}")))?;
        // The genesis is the room's origin event at pos 0, authored now.
        let store = self.sup.open_store()?;
        let rows = store
            .room_tail(&room_id, 1)
            .map_err(|e| CoreError::internal(format!("could not read the genesis: {e}")))?;
        let (event_id, created_at) = rows
            .first()
            .and_then(|se| {
                SignedEvent::decode(&se.wire.signed)
                    .ok()
                    .map(|ev| (se.event_id, ev.created_at))
            })
            .ok_or_else(|| CoreError::internal("the genesis did not persist"))?;
        Ok(RoomCreateOut {
            room_id: RoomId::new(room_id_str),
            name: name.to_string(),
            role: Role::Authority,
            standing: Standing::Active,
            event_id: proj::event_id(&event_id),
            pos: 0,
            created_at: proj_ts(created_at),
        })
    }

    /// `room.list` — every room this identity holds, in what standing, with
    /// recency and capabilities, from local evidence with zero network
    /// activity.
    pub async fn room_list(&self) -> Result<RoomListOut, ApiError> {
        if !self.sup.db_path().exists() {
            return Ok(RoomListOut { rooms: Vec::new() });
        }
        let self_key = match self.sup.local_identity_key() {
            Ok(key) => key,
            Err(e) if e.kind == ErrorKind::IdentityMissing => {
                return Ok(RoomListOut { rooms: Vec::new() })
            }
            Err(e) => return Err(core_to_api(e)),
        };
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
            let self_member = snapshot.member(&self_key);
            let role = snapshot
                .role(&self_key)
                .map(proj::role)
                .unwrap_or(Role::Member);
            let store = self.sup.open_store().map_err(core_to_api)?;
            let (removed_ids, left_ids) = crate::supervisor::departure_sets(&store, &room_id)
                .map_err(core_to_api)?;
            let standing = self_member.map(|m| {
                proj::standing(
                    removed_ids.contains(&m.identity)
                        || matches!(m.status, iroh_rooms::room::Status::Removed),
                    left_ids.contains(&m.identity),
                )
            });
            // Recency: the newest committed event's author-dated instant and
            // kind, max by timestamp over the COMPLETE history (never the wall
            // clock, never a bounded window — a clock-ahead peer's older row
            // with the greatest signed instant must not fall out and move the
            // projection backward).
            let recency = store
                .room_tail(&room_id, u32::MAX)
                .map_err(|_| ApiError::RoomIndexUnreadable)?
                .iter()
                .filter_map(|se| {
                    proj::stored_event_recency(se).map(|(ts, kind)| (ts, se.event_id, kind))
                })
                .max_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
                .map(|(ts, _, kind)| (ts, kind));
            let last_event = proj::last_event(recency);
            let live = self.sup.is_open(&room_id);
            let standing = standing.unwrap_or(Standing::Active);
            let member_count = snapshot.members().count() as u64;
            let capabilities = room_capabilities(standing, role, live);
            rooms.push(RoomRow {
                room_id: proj::room_id(&room_id),
                name: name.unwrap_or_default(),
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
    /// and capabilities, **not history**.
    pub async fn room_activate(&self, req: &RoomActivate) -> CoreResult<RoomActivateOut> {
        let room_id = Self::parse_room(&req.room_id)?;
        // Activate is also a read boundary: authorize before spawning a node.
        self.sup.readable_snapshot(&room_id).await?;
        self.sup.open_room(req.room_id.as_ref(), &[]).await?;
        let live = self.sup.is_open(&room_id);
        let snapshot = self.sup.snapshot_for(&room_id).await?;
        let self_key = self.sup.local_identity_key()?;
        let role = snapshot
            .role(&self_key)
            .map(proj::role)
            .unwrap_or(Role::Member);
        let standing = self_key_standing(self.sup, &room_id, &snapshot, &self_key)?;
        let reachability = self.reachability(&room_id).await;
        Ok(RoomActivateOut {
            room_id: req.room_id.clone(),
            live,
            reachability,
            capabilities: room_capabilities(standing, role, live),
        })
    }

    /// `room.deactivate` — stop live participation without changing membership.
    pub async fn room_deactivate(&self, req: &RoomDeactivate) -> CoreResult<RoomDeactivateOut> {
        self.sup.close_room(req.room_id.as_ref()).await?;
        Ok(RoomDeactivateOut {
            room_id: req.room_id.clone(),
            live: false,
        })
    }

    /// `room.leave` — author a signed departure every member converges on.
    pub async fn room_leave(&self, req: &RoomLeave) -> CoreResult<RoomLeaveOut> {
        let room_id = Self::parse_room(&req.room_id)?;
        let event_id_hex = self.sup.leave_room(req.room_id.as_ref()).await?;
        let pos = self.latest_pos(&room_id)?;
        Ok(RoomLeaveOut {
            room_id: req.room_id.clone(),
            event_id: EventId::new(event_id_hex),
            pos,
            standing: Standing::Left,
        })
    }

    /// `room.timeline` — read committed history identically whether or not
    /// the room is live. A left or removed caller is `membership_ended` — only
    /// `room.archive` and `room.list` are defined over a former membership.
    pub async fn room_timeline(&self, req: &RoomTimeline) -> Result<RoomTimelineOut, ApiError> {
        let room_id = Self::parse_room(&req.room_id).map_err(core_to_api)?;
        let snapshot = self
            .sup
            .readable_snapshot(&room_id)
            .await
            .map_err(|e| core_to_api_room(e, &req.room_id))?;
        self.require_active_standing(&req.room_id, &room_id, &snapshot)?;
        let events = self
            .committed_events(&room_id, &snapshot)
            .map_err(|e| core_to_api_room(e, &req.room_id))?;
        let (page, truncated) = page_events(events, Window::resolve(&req.page)?);
        Ok(RoomTimelineOut {
            room_id: req.room_id.clone(),
            events: page,
            truncated,
        })
    }

    /// Validation-order step 5: the caller's standing must be `active`. A
    /// `left` or `removed` standing is `membership_ended` carrying the ended
    /// standing, for the operations defined over a live membership
    /// (everything except `room.archive` and `room.list`).
    fn require_active_standing(
        &self,
        api_room_id: &RoomId,
        room_id: &IrohRoomId,
        snapshot: &iroh_rooms::room::MembershipSnapshot,
    ) -> Result<(), ApiError> {
        let self_key = self.sup.local_identity_key().map_err(core_to_api)?;
        let store = self.sup.open_store().map_err(core_to_api)?;
        let (removed_ids, left_ids) = crate::supervisor::departure_sets(&store, room_id)
            .map_err(core_to_api)?;
        let standing = snapshot.member(&self_key).map_or(Standing::Active, |m| {
            proj::standing(
                removed_ids.contains(&m.identity)
                    || matches!(m.status, iroh_rooms::room::Status::Removed),
                left_ids.contains(&m.identity),
            )
        });
        if standing == Standing::Active {
            return Ok(());
        }
        Err(ApiError::MembershipEnded {
            room_id: api_room_id.clone(),
            standing,
        })
    }

    /// `room.members` — the authoritative signed answer to who belongs. No
    /// presence, no reachability.
    pub async fn room_members(&self, req: &RoomMembers) -> CoreResult<RoomMembersOut> {
        let room_id = Self::parse_room(&req.room_id)?;
        let snapshot = self.sup.readable_snapshot(&room_id).await?;
        let store = self.sup.open_store()?;
        let (removed_ids, left_ids) = crate::supervisor::departure_sets(&store, &room_id)?;
        let members = snapshot
            .members()
            .map(|m| MemberRow {
                subject_id: SubjectId::new(m.identity.to_string()),
                role: proj::role(m.role),
                standing: proj::standing(
                    removed_ids.contains(&m.identity)
                        || matches!(m.status, iroh_rooms::room::Status::Removed),
                    left_ids.contains(&m.identity),
                ),
                // The fold does not date joins; joined_at is the join event's
                // author-dated instant when discoverable, else the genesis.
                joined_at: self
                    .joined_at(&room_id, &m.identity)
                    .unwrap_or_else(proj_epoch),
            })
            .collect();
        Ok(RoomMembersOut {
            room_id: req.room_id.clone(),
            members,
        })
    }

    /// `room.archive` — open a left or removed room as a local read-only
    /// archive; normatively zero network activity.
    pub async fn room_archive(&self, req: &RoomArchive) -> CoreResult<RoomArchiveOut> {
        let room_id = Self::parse_room(&req.room_id)?;
        let snapshot = self.sup.readable_snapshot(&room_id).await?;
        let self_key = self.sup.local_identity_key()?;
        let standing = self_key_standing(self.sup, &room_id, &snapshot, &self_key)?;
        if standing == Standing::Active {
            return Err(CoreError::new(
                ErrorKind::InvalidParams,
                "room.archive on a room the caller still belongs to",
            ));
        }
        let events = self.committed_events(&room_id, &snapshot)?;
        let (page, truncated) = page_events(events, Window::resolve(&req.page).map_err(api_to_core)?);
        Ok(RoomArchiveOut {
            room_id: req.room_id.clone(),
            standing,
            events: page,
            truncated,
        })
    }

    /// `room.peers` — observed transport facts for one live room.
    pub async fn room_peers(&self, req: &RoomPeers) -> CoreResult<RoomPeersOut> {
        let room_id = Self::parse_room(&req.room_id)?;
        self.sup.readable_snapshot(&room_id).await?;
        let session = self.sup.session(&room_id)?;
        let peers = self.peer_rows(&session.node).await;
        let reachability = reachability_from_peers(&peers, self.sup.is_open(&room_id));
        Ok(RoomPeersOut {
            room_id: req.room_id.clone(),
            reachability,
            peers,
        })
    }

    /// `member.remove` — room authority removes a member, as a signed fact.
    /// Not yet backed by the runtime (the MVP supervisor exposes no removal
    /// flow), so it is refused honestly rather than fabricated.
    pub async fn member_remove(&self, req: &MemberRemove) -> CoreResult<MemberRemoveOut> {
        let _ = req;
        Err(CoreError::new(
            ErrorKind::InvalidParams,
            "member.remove is not implemented in this build",
        ))
    }

    // ------------------------------------------------------------------
    // Invitations
    // ------------------------------------------------------------------

    /// `invite.mint` — mint one key-bound capability exactly one named
    /// identity can redeem.
    pub async fn invite_mint(&self, req: &InviteMint) -> CoreResult<InviteMintOut> {
        if req.role != Role::Member {
            return Err(CoreError::new(
                ErrorKind::InvalidParams,
                "invite.mint may only grant the member role today",
            ));
        }
        // v2 `expires_at` is an absolute instant; the supervisor takes a
        // relative spec. Convert absolute -> seconds-from-now. A past or
        // already-expiring expiry is refused rather than minting a capability
        // that is born expired yet labelled `outstanding` — the reply's
        // redeemability must agree with the capability's signed expiry.
        let expires_ms = req.expires_at.into_inner().unix_timestamp().max(0) as u64 * 1000;
        let now = crate::now_ms();
        if expires_ms <= now {
            return Err(CoreError::new(
                ErrorKind::InvalidParams,
                "invite.mint expiry is not in the future",
            ));
        }
        let spec = format!("{}s", (expires_ms - now) / 1000);
        let ticket = self
            .sup
            .create_invite(
                req.room_id.as_ref(),
                req.subject_id.as_str(),
                "member",
                Some(&spec),
            )
            .await?;
        // The ticket is the capability string; its id is derivable from the
        // ticket itself for the reply.
        let parsed: iroh_rooms::room::RoomInviteTicket = ticket.trim().parse().map_err(|e| {
            CoreError::internal(format!("freshly minted ticket does not parse: {e}"))
        })?;
        Ok(InviteMintOut {
            invite_id: proj::invite_id(&parsed.invite_id),
            room_id: req.room_id.clone(),
            subject_id: req.subject_id.clone(),
            role: req.role,
            expires_at: req.expires_at,
            capability: ticket,
            redeemability: Redeemability::Outstanding,
        })
    }

    /// `invite.redeem` — convert a capability into signed membership.
    pub async fn invite_redeem(&self, req: &InviteRedeem) -> CoreResult<InviteRedeemOut> {
        let room_id_str = self.sup.join_room(&req.capability, None, &[]).await?;
        let room_id: IrohRoomId = room_id_str
            .parse()
            .map_err(|e| CoreError::internal(format!("joined room id does not parse: {e}")))?;
        let snapshot = self.sup.snapshot_for(&room_id).await?;
        let self_key = self.sup.local_identity_key()?;
        let role = snapshot
            .role(&self_key)
            .map(proj::role)
            .unwrap_or(Role::Member);
        // The member_joined event is the newest committed event by this
        // subject; find its id and position.
        let (event_id, pos) = self
            .latest_by_subject(&room_id, &self_key)
            .unwrap_or_else(|| (EventId::new(""), 0));
        Ok(InviteRedeemOut {
            room_id: RoomId::new(room_id_str),
            subject_id: SubjectId::new(self_key.to_string()),
            role,
            standing: Standing::Active,
            event_id,
            pos,
            joined: true,
        })
    }

    /// `invite.list` — enumerate outstanding and recently expired invites.
    /// The MVP runtime does not maintain a served invite index, so this is
    /// refused honestly rather than fabricated empty.
    pub async fn invite_list(&self, req: &InviteList) -> CoreResult<InviteListOut> {
        let _ = req;
        Err(CoreError::new(
            ErrorKind::InvalidParams,
            "invite.list is not implemented in this build",
        ))
    }

    /// `invite.revoke` — withdraw an outstanding capability before expiry.
    /// Not yet backed by the runtime.
    pub async fn invite_revoke(&self, req: &InviteRevoke) -> CoreResult<InviteRevokeOut> {
        let _ = req;
        Err(CoreError::new(
            ErrorKind::InvalidParams,
            "invite.revoke is not implemented in this build",
        ))
    }

    // ------------------------------------------------------------------
    // Timeline
    // ------------------------------------------------------------------

    /// `message.send` — author a message.
    pub async fn message_send(&self, req: &MessageSend) -> CoreResult<MessageSendOut> {
        let body_len = req.body.len() as u64;
        if req.body.is_empty() || body_len > limits().max_message_body_bytes {
            return Err(CoreError::new(
                ErrorKind::InvalidParams,
                format!(
                    "message body must be 1..={} bytes",
                    limits().max_message_body_bytes
                ),
            ));
        }
        let room_id = Self::parse_room(&req.room_id)?;
        let event_id_hex = self
            .sup
            .send_message(req.room_id.as_ref(), &req.body)
            .await?;
        let pos = self.latest_pos(&room_id)?;
        Ok(MessageSendOut {
            room_id: req.room_id.clone(),
            event_id: EventId::new(event_id_hex),
            pos,
            at: proj_ts(crate::now_ms()),
        })
    }

    /// `status.post` — author an agent status (any active member).
    pub async fn status_post(&self, req: &StatusPost) -> CoreResult<StatusPostOut> {
        let room_id = Self::parse_room(&req.room_id)?;
        let label = status_label_wire(req.label);
        let progress_pct = match req.progress {
            Progress::Reported { percent } => Some(u64::from(percent)),
            Progress::Absent => None,
        };
        let event_id_hex = self
            .sup
            .post_status(req.room_id.as_ref(), label, None, progress_pct, &[])
            .await?;
        let pos = self.latest_pos(&room_id)?;
        Ok(StatusPostOut {
            room_id: req.room_id.clone(),
            event_id: EventId::new(event_id_hex),
            pos,
            at: proj_ts(crate::now_ms()),
            severity: req.label.severity(),
        })
    }

    /// `status.history` — read one subject's status history, one entry per
    /// real posted event, chronological.
    pub async fn status_history(&self, req: &StatusHistory) -> CoreResult<StatusHistoryOut> {
        let room_id = Self::parse_room(&req.room_id)?;
        let identity = Self::parse_subject(&req.subject_id)?;
        self.sup.readable_snapshot(&room_id).await?;
        let store = self.sup.open_store()?;
        let rows = store
            .room_tail(&room_id, u32::MAX)
            .map_err(|e| CoreError::internal(format!("could not read the timeline: {e}")))?;
        let mut entries = Vec::new();
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
            let Content::AgentStatus(c) = ev.content else {
                continue;
            };
            let Ok(label) = status_label_parse(&c.status) else {
                continue; // out-of-vocabulary labels are not reclassified
            };
            entries.push(StatusEntry {
                at: proj_ts(ev.created_at),
                label,
                severity: label.severity(),
                progress: proj_progress(c.progress_pct),
            });
        }
        // Paging over the chronological entries.
        let window = Window::resolve(&req.page).map_err(api_to_core)?;
        let (page, truncated) = page_indexed(entries, window);
        Ok(StatusHistoryOut {
            room_id: req.room_id.clone(),
            subject_id: req.subject_id.clone(),
            entries: page,
            truncated,
        })
    }

    /// `fleet.list` — the agent fleet projection, no tallies.
    pub async fn fleet_list(&self) -> CoreResult<FleetListOut> {
        let now = crate::now_ms();
        let self_id = self.sup.local_identity_key()?;
        let known: BTreeSet<String> = crate::localstate::load(self.sup.data_dir())?
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
            let agent_ids: BTreeSet<iroh_rooms::identity::IdentityKey> = snapshot
                .members()
                .filter(|m| m.role == iroh_rooms::room::Role::Agent)
                .map(|m| m.identity)
                .collect();
            if agent_ids.is_empty() {
                continue;
            }
            let rows = {
                let store = self.sup.open_store()?;
                store
                    .room_tail(&room_id, u32::MAX)
                    .map_err(|e| CoreError::internal(format!("could not read the timeline: {e}")))?
            };
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

    /// `file.share` — share bytes into a room. The v2 surface declares the
    /// name/size/type; the daemon stages the bytes itself.
    pub async fn file_share(&self, req: &FileShare) -> CoreResult<FileShareOut> {
        // The v1 staging endpoint supplies bytes by path; the v2 WS op is the
        // typed declaration. Until the byte-stream staging is wired into the
        // codec, this is refused honestly rather than fabricating an event.
        let _ = req;
        Err(CoreError::new(
            ErrorKind::InvalidParams,
            "file.share byte staging is not wired to the typed surface in this build",
        ))
    }

    /// `file.list` — files shared into a room, provider availability as a
    /// protocol fact.
    pub async fn file_list(&self, req: &FileList) -> CoreResult<FileListOut> {
        let room_id = Self::parse_room(&req.room_id)?;
        self.sup.readable_snapshot(&room_id).await?;
        let store = self.sup.open_store()?;
        let events = store
            .by_type(&room_id, EventType::FileShared)
            .map_err(|e| CoreError::internal(format!("could not read file.shared events: {e}")))?;
        let session = self.sup.session_opt(&room_id);
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
            let fetchable = session.as_deref().is_some_and(|s| {
                providers.iter().any(|p| {
                    crate::supervisor::endpoint_id_of(*p).is_ok_and(|id| {
                        s.node.peer_state(id)
                            == Some(iroh_rooms::experimental::session::PeerConnState::Connected)
                    })
                })
            });
            let file_id = file_handle(&f.file_id);
            let self_hosted =
                crate::localstate::fetched_file(self.sup.data_dir(), &room_id_str, &file_id)
                    .is_some();
            let provider_rows = providers
                .iter()
                .map(|p| PeerRow {
                    subject_id: SubjectId::new(ev.sender_id.to_string()),
                    device_id: DeviceId::new(p.to_string()),
                    link: Link::NotConnected {
                        reason: LinkReason::NoRoute,
                    },
                })
                .collect();
            files.push(FileRow {
                file_id: FileId::new(file_id),
                name: f.name.clone(),
                bytes: f.size_bytes,
                digest: f.blob_hash.to_string(),
                declared_content_type: f.mime_type.clone(),
                shared_by: SubjectId::new(ev.sender_id.to_string()),
                shared_at: proj_ts(ev.created_at),
                providers: provider_rows,
                fetchable,
                self_hosted,
            });
        }
        // Paging over the file rows (position = index within the file index).
        let window = Window::resolve(&req.page).map_err(api_to_core)?;
        let (page, truncated) = page_indexed(files, window);
        Ok(FileListOut {
            room_id: req.room_id.clone(),
            files: page,
            truncated,
        })
    }

    /// `file.fetch` — fetch a file's bytes from a provider; the daemon holds
    /// the bytes and `file.read` streams them out.
    pub async fn file_fetch(&self, req: &FileFetch) -> CoreResult<FileFetchOut> {
        let room_id = Self::parse_room(&req.room_id)?;
        let result = self
            .sup
            .fetch_file(req.room_id.as_ref(), req.file_id.as_str(), None)
            .await?;
        let bytes = result
            .get("bytes")
            .and_then(|b| b.as_u64())
            .ok_or_else(|| CoreError::internal("fetch result carried no byte count"))?;
        // The verified digest comes from the shared file's declared hash.
        let file_id = Self::parse_file(&req.file_id)?;
        let store = self.sup.open_store()?;
        let events = store
            .by_type(&room_id, EventType::FileShared)
            .map_err(|e| CoreError::internal(format!("could not read file.shared events: {e}")))?;
        let digest = events
            .iter()
            .filter_map(|se| SignedEvent::decode(&se.wire.signed).ok())
            .find_map(|ev| match ev.content {
                Content::FileShared(f) if f.file_id == file_id => Some(f.blob_hash.to_string()),
                _ => None,
            })
            .unwrap_or_default();
        let _ = room_id;
        Ok(FileFetchOut {
            room_id: req.room_id.clone(),
            file_id: req.file_id.clone(),
            bytes,
            digest,
            provider: ProviderRef {
                subject_id: SubjectId::new(""),
                device_id: DeviceId::new(""),
            },
        })
    }

    /// `file.read` — stream locally held bytes out (the header; bytes follow).
    pub async fn file_read(&self, req: &FileRead) -> CoreResult<FileReadOut> {
        let local = self
            .sup
            .local_file(req.room_id.as_ref(), req.file_id.as_str())
            .await?;
        Ok(FileReadOut {
            room_id: req.room_id.clone(),
            file_id: req.file_id.clone(),
            bytes: local.bytes,
            declared_content_type: local.mime,
        })
    }

    /// `transfer.cancel` — cancel a transfer by the op_id that started it.
    /// Transfers are not yet tracked by op_id in this build.
    pub async fn transfer_cancel(&self, req: &TransferCancel) -> CoreResult<TransferCancelOut> {
        let _ = req;
        Err(CoreError::new(
            ErrorKind::InvalidParams,
            "transfer.cancel is not implemented in this build",
        ))
    }

    // ------------------------------------------------------------------
    // Pipes
    // ------------------------------------------------------------------

    /// `pipe.publish` — publish a pipe to a loopback target. `audience: room`
    /// authorizes every active member; `audience: subjects` authorizes the
    /// named list. A non-loopback target or an out-of-range port is
    /// `pipe_target_refused` carrying the rejected target verbatim.
    pub async fn pipe_publish(&self, req: &PipePublish) -> Result<PipePublishOut, ApiError> {
        let room_id = Self::parse_room(&req.room_id).map_err(core_to_api)?;
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
        let snapshot = self
            .sup
            .readable_snapshot(&room_id)
            .await
            .map_err(core_to_api)?;
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

        let target_hint = format!("{}:{}", req.target.host, req.target.port);
        let (pipe_id, event_id) = self
            .sup
            .pipe_expose_multi(&room_id, target_addr, &target_hint, &allowed)
            .await
            .map_err(core_to_api)?;
        let pos = self.latest_pos(&room_id).map_err(core_to_api)?;
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
    pub async fn pipe_list(&self, req: &PipeList) -> CoreResult<PipeListOut> {
        let room_id = Self::parse_room(&req.room_id)?;
        self.sup.readable_snapshot(&room_id).await?;
        let store = self.sup.open_store()?;
        let session = self.sup.session_opt(&room_id);
        let closed = closed_pipe_ids(&store, &room_id)?;
        let opened = store
            .by_type(&room_id, EventType::PipeOpened)
            .map_err(|e| CoreError::internal(format!("could not read pipe.opened events: {e}")))?;
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
            let connected = session.as_deref().is_some_and(|s| {
                crate::supervisor::endpoint_id_of(p.owner_endpoint).is_ok_and(|id| {
                    s.node.peer_state(id)
                        == Some(iroh_rooms::experimental::session::PeerConnState::Connected)
                })
            });
            let link = if connected {
                Link::Direct {
                    since: proj_epoch(),
                }
            } else {
                Link::NotConnected {
                    reason: LinkReason::NoRoute,
                }
            };
            pipes.push((
                se.lamport.unwrap(),
                PipeRow {
                    pipe_id: proj::pipe_id(&p.pipe_id),
                    published_by: SubjectId::new(p.owner_id.to_string()),
                    device_id: DeviceId::new(p.owner_endpoint.to_string()),
                    published_at: proj_ts(ev.created_at),
                    link,
                    connected,
                },
            ));
        }
        pipes.sort_by_key(|(pos, _)| *pos);
        let all: Vec<PipeRow> = pipes.into_iter().map(|(_, p)| p).collect();
        let window = Window::resolve(&req.page).map_err(api_to_core)?;
        let (page, truncated) = page_indexed(all, window);
        Ok(PipeListOut {
            room_id: req.room_id.clone(),
            pipes: page,
            truncated,
        })
    }

    /// `pipe.connect` — connect to a pipe.
    pub async fn pipe_connect(&self, req: &PipeConnect) -> CoreResult<PipeConnectOut> {
        let local_addr = self
            .sup
            .pipe_connect(req.room_id.as_ref(), req.pipe_id.as_str())
            .await?;
        let (host, port) = split_host_port(&local_addr);
        Ok(PipeConnectOut {
            room_id: req.room_id.clone(),
            pipe_id: req.pipe_id.clone(),
            connection_id: local_addr.clone(),
            local: Target { host, port },
        })
    }

    /// `pipe.release` — release a local connection, named by the connection.
    pub async fn pipe_release(&self, req: &PipeRelease) -> CoreResult<PipeReleaseOut> {
        // The runtime names connections by their local addr today; releasing
        // by connection_id requires a connection index the MVP does not keep.
        let _ = req;
        Err(CoreError::new(
            ErrorKind::InvalidParams,
            "pipe.release by connection_id is not implemented in this build",
        ))
    }

    /// `pipe.revoke` — withdraw a published pipe as a signed fact.
    pub async fn pipe_revoke(&self, req: &PipeRevoke) -> CoreResult<PipeRevokeOut> {
        let room_id = Self::parse_room(&req.room_id)?;
        let result = self
            .sup
            .pipe_close(req.room_id.as_ref(), req.pipe_id.as_str())
            .await?;
        let event_id = result
            .get("event_id")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string();
        let pos = self.latest_pos(&room_id)?;
        Ok(PipeRevokeOut {
            room_id: req.room_id.clone(),
            pipe_id: req.pipe_id.clone(),
            event_id: EventId::new(event_id),
            pos,
            revoked_at: proj_ts(crate::now_ms()),
        })
    }

    // ------------------------------------------------------------------
    // Shared helpers
    // ------------------------------------------------------------------

    /// All committed timeline events for a room, ascending by position.
    fn committed_events(
        &self,
        room_id: &IrohRoomId,
        snapshot: &iroh_rooms::room::MembershipSnapshot,
    ) -> CoreResult<Vec<Event>> {
        let store = self.sup.open_store()?;
        let rows = store
            .room_tail(room_id, u32::MAX)
            .map_err(|e| CoreError::internal(format!("could not read the timeline: {e}")))?;
        Ok(rows
            .iter()
            .filter_map(|se| proj::materialize(se, snapshot))
            .collect())
    }

    /// The newest committed position in a room (0 when empty).
    fn latest_pos(&self, room_id: &IrohRoomId) -> CoreResult<u64> {
        let store = self.sup.open_store()?;
        let rows = store
            .room_tail(room_id, 1)
            .map_err(|e| CoreError::internal(format!("could not read the timeline: {e}")))?;
        Ok(rows.first().and_then(|se| se.lamport).unwrap_or(0))
    }

    /// The newest committed event authored by a subject, with its position.
    fn latest_by_subject(
        &self,
        room_id: &IrohRoomId,
        subject: &iroh_rooms::identity::IdentityKey,
    ) -> Option<(EventId, u64)> {
        let store = self.sup.open_store().ok()?;
        let rows = store.room_tail(room_id, u32::MAX).ok()?;
        rows.iter()
            .filter(|se| se.lamport.is_some())
            .filter_map(|se| {
                let ev = SignedEvent::decode(&se.wire.signed).ok()?;
                if &ev.sender_id == subject {
                    Some((proj::event_id(&se.event_id), se.lamport.unwrap()))
                } else {
                    None
                }
            })
            .next_back()
    }

    /// The author-dated instant a subject joined, when discoverable.
    fn joined_at(
        &self,
        room_id: &IrohRoomId,
        subject: &iroh_rooms::identity::IdentityKey,
    ) -> Option<Timestamp> {
        let store = self.sup.open_store().ok()?;
        let rows = store.by_type(room_id, EventType::MemberJoined).ok()?;
        for se in rows {
            let ev = SignedEvent::decode(&se.wire.signed).ok()?;
            if let Content::MemberJoined(c) = &ev.content {
                if &c.device_binding.identity_key == subject {
                    return Some(proj_ts(ev.created_at));
                }
            }
        }
        // The authority's join is the genesis.
        let genesis = store.room_tail(room_id, 1).ok()?;
        let first = genesis.first()?;
        let ev = SignedEvent::decode(&first.wire.signed).ok()?;
        if &ev.sender_id == subject {
            return Some(proj_ts(ev.created_at));
        }
        None
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
            .map(|(device, entry)| {
                let connected = matches!(
                    entry.state,
                    iroh_rooms::experimental::session::PeerConnState::Connected
                );
                let link = if connected {
                    match paths.get(&device).copied() {
                        Some("direct") | Some("mixed") => Link::Direct {
                            since: proj_epoch(),
                        },
                        Some("relay") => Link::Relay {
                            since: proj_epoch(),
                        },
                        _ => Link::NotConnected {
                            reason: LinkReason::NoRoute,
                        },
                    }
                } else {
                    Link::NotConnected {
                        reason: LinkReason::NoRoute,
                    }
                };
                PeerRow {
                    subject_id: entry
                        .identity
                        .as_ref()
                        .map(|id| SubjectId::new(id.to_string()))
                        .unwrap_or_else(|| SubjectId::new("")),
                    device_id: DeviceId::new(device.to_string()),
                    link,
                }
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

/// The capabilities a caller holds in a room, derived from standing/role/live.
fn room_capabilities(standing: Standing, role: Role, live: bool) -> Vec<CapabilityToken> {
    use CapabilityToken::*;
    let mut caps = vec![RoomTimeline, RoomMembers, RoomList, RoomArchive];
    if standing == Standing::Active {
        caps.extend([RoomLeave, InviteList, FileList, PipeList, StatusHistory]);
        if live {
            caps.extend([
                RoomDeactivate,
                RoomPeers,
                MessageSend,
                StatusPost,
                FileShare,
                FileFetch,
                FileRead,
                PipePublish,
                PipeConnect,
                StreamSubscribe,
                StreamUnsubscribe,
                StreamResync,
            ]);
        } else {
            caps.push(RoomActivate);
        }
        if role == Role::Authority {
            caps.extend([InviteMint, InviteRevoke, MemberRemove, PipeRevoke]);
        }
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

/// The caller's own standing in a room.
fn self_key_standing(
    sup: &RoomSupervisor,
    room_id: &IrohRoomId,
    snapshot: &iroh_rooms::room::MembershipSnapshot,
    self_key: &iroh_rooms::identity::IdentityKey,
) -> CoreResult<Standing> {
    let store = sup.open_store()?;
    let (removed_ids, left_ids) = crate::supervisor::departure_sets(&store, room_id)?;
    Ok(snapshot.member(self_key).map_or(Standing::Active, |m| {
        proj::standing(
            removed_ids.contains(&m.identity)
                || matches!(m.status, iroh_rooms::room::Status::Removed),
            left_ids.contains(&m.identity),
        )
    }))
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

/// Map an on-wire progress percent to the v2 variant.
fn proj_progress(pct: Option<u64>) -> Progress {
    match pct {
        Some(p) => Progress::Reported {
            percent: u8::try_from(p.min(100)).unwrap_or(100),
        },
        None => Progress::Absent,
    }
}

/// The wire `<ts>` for an author-dated ms instant.
fn proj_ts(created_at_ms: u64) -> Timestamp {
    let secs = i64::try_from(created_at_ms / 1000).unwrap_or(i64::MAX);
    Timestamp::new(
        time::OffsetDateTime::from_unix_timestamp(secs).unwrap_or(time::OffsetDateTime::UNIX_EPOCH),
    )
}

/// The Unix epoch as a `<ts>` (an honest "unknown instant", never the wall
/// clock).
fn proj_epoch() -> Timestamp {
    Timestamp::new(time::OffsetDateTime::UNIX_EPOCH)
}

/// Whether a socket address is loopback (IPv4 127.0.0.0/8 or IPv6 ::1).
fn is_loopback_addr(addr: &std::net::SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// Split a `"host:port"` loopback address into a `Target`.
fn split_host_port(addr: &str) -> (String, u64) {
    match addr.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().unwrap_or(0)),
        None => (addr.to_string(), 0),
    }
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
// The typed dispatch table (the engine's v2 surface)
// ---------------------------------------------------------------------------

/// One typed operation in, its typed output out, or a typed error. This is
/// the engine's v2 dispatch seam: the codec hands a decoded [`Call`] here and
/// gets a `Result<Output, ApiError>` to encode.
///
/// The dispatch is **total by construction**: the codec's router already
/// refused any `op` not in the 33, so the downcast to the concrete input type
/// cannot fail.
pub async fn dispatch(sup: &RoomSupervisor, call: TypedCall) -> Result<TypedReply, ApiError> {
    let t = TypedSupervisor::new(sup);
    // Validation-order step 2 (subject precondition): every operation except
    // `subject.ensure` — which exists to create the subject — requires a local
    // subject before any later stage (dedup, room index, standing, role,
    // semantics) runs. `subject_absent` outranks `room_not_available` and is
    // never a membership oracle.
    if !matches!(call, TypedCall::SubjectEnsure(_)) && !t.subject_present()? {
        return Err(ApiError::SubjectAbsent);
    }
    match call {
        TypedCall::SubjectEnsure(_) => t
            .subject_ensure()
            .map(TypedReply::SubjectEnsure)
            .map_err(core_to_api),
        TypedCall::RoomCreate(r) => t
            .room_create(&r)
            .map(TypedReply::RoomCreate)
            .map_err(core_to_api),
        TypedCall::RoomList(_) => t.room_list().await.map(TypedReply::RoomList),
        TypedCall::RoomActivate(r) => t
            .room_activate(&r)
            .await
            .map(TypedReply::RoomActivate)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::RoomDeactivate(r) => t
            .room_deactivate(&r)
            .await
            .map(TypedReply::RoomDeactivate)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::RoomLeave(r) => t
            .room_leave(&r)
            .await
            .map(TypedReply::RoomLeave)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::RoomTimeline(r) => t.room_timeline(&r).await.map(TypedReply::RoomTimeline),
        TypedCall::RoomMembers(r) => t
            .room_members(&r)
            .await
            .map(TypedReply::RoomMembers)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::RoomArchive(r) => t
            .room_archive(&r)
            .await
            .map(TypedReply::RoomArchive)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::RoomPeers(r) => t
            .room_peers(&r)
            .await
            .map(TypedReply::RoomPeers)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::MemberRemove(r) => t
            .member_remove(&r)
            .await
            .map(TypedReply::MemberRemove)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::InviteMint(r) => t
            .invite_mint(&r)
            .await
            .map(TypedReply::InviteMint)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::InviteList(r) => t
            .invite_list(&r)
            .await
            .map(TypedReply::InviteList)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::InviteRevoke(r) => t
            .invite_revoke(&r)
            .await
            .map(TypedReply::InviteRevoke)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::InviteRedeem(r) => t
            .invite_redeem(&r)
            .await
            .map(TypedReply::InviteRedeem)
            .map_err(core_to_api),
        TypedCall::MessageSend(r) => t
            .message_send(&r)
            .await
            .map(TypedReply::MessageSend)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::StatusPost(r) => t
            .status_post(&r)
            .await
            .map(TypedReply::StatusPost)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::StatusHistory(r) => t
            .status_history(&r)
            .await
            .map(TypedReply::StatusHistory)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::FleetList(_) => t
            .fleet_list()
            .await
            .map(TypedReply::FleetList)
            .map_err(core_to_api),
        TypedCall::FileShare(r) => t
            .file_share(&r)
            .await
            .map(TypedReply::FileShare)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::FileList(r) => t
            .file_list(&r)
            .await
            .map(TypedReply::FileList)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::FileFetch(r) => t
            .file_fetch(&r)
            .await
            .map(TypedReply::FileFetch)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::FileRead(r) => t
            .file_read(&r)
            .await
            .map(TypedReply::FileRead)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::TransferCancel(r) => t
            .transfer_cancel(&r)
            .await
            .map(TypedReply::TransferCancel)
            .map_err(core_to_api),
        TypedCall::PipePublish(r) => t.pipe_publish(&r).await.map(TypedReply::PipePublish),
        TypedCall::PipeList(r) => t
            .pipe_list(&r)
            .await
            .map(TypedReply::PipeList)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::PipeConnect(r) => t
            .pipe_connect(&r)
            .await
            .map(TypedReply::PipeConnect)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        TypedCall::PipeRelease(r) => t
            .pipe_release(&r)
            .await
            .map(TypedReply::PipeRelease)
            .map_err(core_to_api),
        TypedCall::PipeRevoke(r) => t
            .pipe_revoke(&r)
            .await
            .map(TypedReply::PipeRevoke)
            .map_err(|e| core_to_api_room(e, &r.room_id)),
        // Stream operations are connection-scoped; the engine handles them
        // against the connection's subscription set, not the supervisor.
        TypedCall::StreamSubscribe(_)
        | TypedCall::StreamUnsubscribe(_)
        | TypedCall::StreamResync(_) => Err(ApiError::SubscriptionUnknown {
            room_id: RoomId::new(""),
        }),
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

/// Map a typed [`ApiError`] back onto a core error, for the typed-read paths
/// that still surface `CoreResult` internally.
fn api_to_core(err: ApiError) -> CoreError {
    match err {
        ApiError::InvalidArgument { .. } => {
            CoreError::new(ErrorKind::InvalidParams, "invalid paging argument")
        }
        ApiError::RoomIndexUnreadable => {
            CoreError::new(ErrorKind::Internal, "room index unreadable")
        }
        _ => CoreError::internal("typed projection error"),
    }
}

/// Map a core error onto the typed v2 taxonomy at the engine boundary. A
/// room-scoped error carries the identifier the operation named, never an
/// empty placeholder — the typed error schema fixes the field and a client
/// branches on it.
fn core_to_api(err: CoreError) -> ApiError {
    core_to_api_ctx(err, None)
}

/// [`core_to_api`] with the room the operation named, for room-scoped errors.
fn core_to_api_room(err: CoreError, room_id: &RoomId) -> ApiError {
    core_to_api_ctx(err, Some(room_id.clone()))
}

fn core_to_api_ctx(err: CoreError, room_id: Option<RoomId>) -> ApiError {
    let rid = || room_id.clone().unwrap_or_else(|| RoomId::new(""));
    match err.kind {
        ErrorKind::IdentityMissing => ApiError::SubjectAbsent,
        ErrorKind::RoomUnknown | ErrorKind::NotAMember | ErrorKind::FileUnauthorized => {
            ApiError::RoomNotAvailable { room_id: rid() }
        }
        ErrorKind::RoomNotOpen => ApiError::RoomNotLive { room_id: rid() },
        ErrorKind::BadTicket => ApiError::CapabilityInvalid,
        ErrorKind::TicketExpired => ApiError::CapabilityExpired {
            expired_at: proj_epoch(),
        },
        ErrorKind::FileUnavailable => ApiError::ProviderUnreachable {
            file_id: FileId::new(""),
            providers: Vec::new(),
        },
        ErrorKind::HashMismatch => ApiError::DigestMismatch {
            expected: String::new(),
            observed: String::new(),
        },
        ErrorKind::PipeDenied => ApiError::PolicyRefused { room_id: rid() },
        ErrorKind::PeerUnreachable => ApiError::PipeUnreachable {
            pipe_id: PipeId::new(""),
            link: Link::NotConnected {
                reason: LinkReason::NoRoute,
            },
        },
        ErrorKind::IdentityExists | ErrorKind::InvalidParams | ErrorKind::Internal => {
            ApiError::InvalidArgument {
                field: "in".to_string(),
                reason: InvalidReason::Format,
            }
        }
    }
}
