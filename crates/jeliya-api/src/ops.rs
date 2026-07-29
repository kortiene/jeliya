//! The 33 approved protocol-v2 operations, each a request type
//! compile-time paired with its output via [`Operation`]. `M` in the
//! record's table is [`Operation::MUTATING`]. Every field of every `in` is
//! required — `Option` appears in no request type here.

use crate::ids::*;
use crate::shared::*;
use crate::types::*;
use crate::{Operation, Timestamp};
use serde::{Deserialize, Serialize};

macro_rules! operation {
    (
        $(#[$meta:meta])*
        $req:ident, $out:ident, $path:literal, $mutating:literal
    ) => {
        impl Operation for $req {
            type Output = $out;
            const PATH: &'static str = $path;
            const MUTATING: bool = $mutating;
        }
    };
}

// ---------------------------------------------------------------------------
// Subject and daemon
// ---------------------------------------------------------------------------

/// `subject.ensure` — establish the local subject exactly once. Naturally
/// idempotent: a second call returns the same subject with `created: false`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectEnsure {}

/// Its public names and no secret, in any form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectEnsureOut {
    /// The subject id.
    pub subject_id: SubjectId,
    /// The device id.
    pub device_id: DeviceId,
    /// `true` when this call created the subject.
    pub created: bool,
}
operation!(SubjectEnsure, SubjectEnsureOut, "subject.ensure", true);

/// `daemon.stop` — terminate deterministically, reply flushed first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStop {}

/// Terminal acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStopOut {
    /// Always `true` — a second call is `shutdown_in_progress`, never a
    /// comfortable repeat `true`.
    pub stopping: bool,
}
operation!(DaemonStop, DaemonStopOut, "daemon.stop", true);

// ---------------------------------------------------------------------------
// Rooms
// ---------------------------------------------------------------------------

/// `room.create` — bring a room into existence with the caller as its
/// authority; works with no network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomCreate {
    /// `1..=128` bytes after trimming, at least one non-whitespace.
    pub name: String,
}

/// The new room, its origin event, and the caller's authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomCreateOut {
    /// The room id.
    pub room_id: RoomId,
    /// Its name.
    pub name: String,
    /// Always `authority` — served rather than assumed.
    pub role: Role,
    /// Always `active`.
    pub standing: Standing,
    /// The `room_created` event id.
    pub event_id: EventId,
    /// The room's position space anchored at its origin.
    pub pos: u64,
    /// When the event was authored.
    pub created_at: Timestamp,
}
operation!(RoomCreate, RoomCreateOut, "room.create", true);

/// `room.list` — what rooms are mine, in what standing, what may I do in
/// each, from local evidence with zero network activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomList {}

/// One room row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomRow {
    /// The room id.
    pub room_id: RoomId,
    /// Its name (a label, never identity).
    pub name: String,
    /// The caller's own standing.
    pub standing: Standing,
    /// Whether this daemon has a live session.
    pub live: bool,
    /// The caller's role.
    pub role: Role,
    /// Signed member count.
    pub member_count: u64,
    /// The newest event, or the stated `absent` arm.
    pub last_event: LastEvent,
    /// Operation-name tokens the caller may invoke right now.
    pub capabilities: Vec<String>,
}

/// The room list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomListOut {
    /// Every locally known room.
    pub rooms: Vec<RoomRow>,
}
operation!(RoomList, RoomListOut, "room.list", false);

/// `room.activate` — make a room live on this device; returns reachability
/// and capabilities, **not history**.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomActivate {
    /// The room.
    pub room_id: RoomId,
}

/// The live result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomActivateOut {
    /// The room.
    pub room_id: RoomId,
    /// Always `true`.
    pub live: bool,
    /// Whole-room reachability.
    pub reachability: Reachability,
    /// Capabilities now available.
    pub capabilities: Vec<String>,
}
operation!(RoomActivate, RoomActivateOut, "room.activate", true);

/// `room.deactivate` — stop live participation without changing membership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomDeactivate {
    /// The room.
    pub room_id: RoomId,
}

/// The deactivated result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomDeactivateOut {
    /// The room.
    pub room_id: RoomId,
    /// Always `false`.
    pub live: bool,
}
operation!(RoomDeactivate, RoomDeactivateOut, "room.deactivate", true);

/// `room.leave` — author a signed departure every member converges on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomLeave {
    /// The room.
    pub room_id: RoomId,
}

/// The departure event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomLeaveOut {
    /// The room.
    pub room_id: RoomId,
    /// The authored `member_left` event.
    pub event_id: EventId,
    /// Its position.
    pub pos: u64,
    /// Always `left`.
    pub standing: Standing,
}
operation!(RoomLeave, RoomLeaveOut, "room.leave", true);

/// Paging fields shared by the six paging operations — cursor, direction,
/// and limit are all required, never defaulted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    /// Where to start.
    pub cursor: Cursor,
    /// Which way to read.
    pub direction: Direction,
    /// Page size, `1..=timeline_page_max` — refused, never clamped.
    pub limit: u64,
}

/// `room.timeline` — read committed history identically whether or not the
/// room is live.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomTimeline {
    /// The room.
    pub room_id: RoomId,
    /// Paging.
    #[serde(flatten)]
    pub page: Page,
}

/// A page of events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomTimelineOut {
    /// The room.
    pub room_id: RoomId,
    /// The events.
    pub events: Vec<Event>,
    /// The one continuation mechanism.
    pub truncated: Truncated,
}
operation!(RoomTimeline, RoomTimelineOut, "room.timeline", false);

/// `room.members` — the authoritative signed answer to who belongs.
/// Carries **no** presence and **no** reachability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomMembers {
    /// The room.
    pub room_id: RoomId,
}

/// One member row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemberRow {
    /// The member's subject.
    pub subject_id: SubjectId,
    /// Their role.
    pub role: Role,
    /// Their standing.
    pub standing: Standing,
    /// When they joined.
    pub joined_at: Timestamp,
}

/// The roster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomMembersOut {
    /// The room.
    pub room_id: RoomId,
    /// The members.
    pub members: Vec<MemberRow>,
}
operation!(RoomMembers, RoomMembersOut, "room.members", false);

/// `room.archive` — open a left or removed room as a local read-only
/// archive; normatively zero network activity and zero durable mutation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomArchive {
    /// The room.
    pub room_id: RoomId,
    /// Paging.
    #[serde(flatten)]
    pub page: Page,
}

/// The archived room.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomArchiveOut {
    /// The room.
    pub room_id: RoomId,
    /// The caller's (ended) standing.
    pub standing: Standing,
    /// The archived events.
    pub events: Vec<Event>,
    /// The one continuation mechanism.
    pub truncated: Truncated,
}
operation!(RoomArchive, RoomArchiveOut, "room.archive", false);

/// `room.peers` — observed transport facts for one live room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomPeers {
    /// The room.
    pub room_id: RoomId,
}

/// One peer's link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerRow {
    /// The peer's subject.
    pub subject_id: SubjectId,
    /// The peer's device.
    pub device_id: DeviceId,
    /// This daemon's link to it, or why not.
    pub link: Link,
}

/// The peers answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomPeersOut {
    /// The room.
    pub room_id: RoomId,
    /// Whole-room reachability.
    pub reachability: Reachability,
    /// The per-device links.
    pub peers: Vec<PeerRow>,
}
operation!(RoomPeers, RoomPeersOut, "room.peers", false);

/// `member.remove` — room authority removes a member, as a signed fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberRemove {
    /// The room.
    pub room_id: RoomId,
    /// Who is removed.
    pub subject_id: SubjectId,
}

/// The removal event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemberRemoveOut {
    /// The room.
    pub room_id: RoomId,
    /// Who was removed.
    pub subject_id: SubjectId,
    /// The authored `member_removed` event.
    pub event_id: EventId,
    /// Its position.
    pub pos: u64,
    /// Always `removed`.
    pub standing: Standing,
}
operation!(MemberRemove, MemberRemoveOut, "member.remove", true);

// ---------------------------------------------------------------------------
// Invitations
// ---------------------------------------------------------------------------

/// `invite.mint` — mint one key-bound capability exactly one named identity
/// can redeem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InviteMint {
    /// The room.
    pub room_id: RoomId,
    /// The identity the capability is bound to.
    pub subject_id: SubjectId,
    /// The role granted — `member` only today; `authority` is
    /// `role_not_grantable`. Kept because the limitation is a product gap,
    /// not a design intent.
    pub role: Role,
    /// Absolute expiry.
    pub expires_at: Timestamp,
}

/// The minted capability — served only here, and only once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InviteMintOut {
    /// The invite id.
    pub invite_id: InviteId,
    /// The room.
    pub room_id: RoomId,
    /// The bound identity.
    pub subject_id: SubjectId,
    /// The granted role.
    pub role: Role,
    /// Absolute expiry.
    pub expires_at: Timestamp,
    /// The capability itself.
    pub capability: String,
    /// Always `outstanding` at mint.
    pub redeemability: Redeemability,
}
operation!(InviteMint, InviteMintOut, "invite.mint", true);

/// `invite.list` — enumerate outstanding and recently expired invites.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InviteList {
    /// The room.
    pub room_id: RoomId,
    /// Paging.
    #[serde(flatten)]
    pub page: Page,
}

/// One invite row. The capability is **never** returned here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InviteRow {
    /// The invite id.
    pub invite_id: InviteId,
    /// The bound identity.
    pub subject_id: SubjectId,
    /// The granted role.
    pub role: Role,
    /// Absolute expiry.
    pub expires_at: Timestamp,
    /// Can this be redeemed, and if not why not.
    pub redeemability: Redeemability,
}

/// The invites page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InviteListOut {
    /// The room.
    pub room_id: RoomId,
    /// The invites.
    pub invites: Vec<InviteRow>,
    /// The one continuation mechanism.
    pub truncated: Truncated,
}
operation!(InviteList, InviteListOut, "invite.list", false);

/// `invite.revoke` — withdraw an outstanding capability before expiry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteRevoke {
    /// The room.
    pub room_id: RoomId,
    /// The invite.
    pub invite_id: InviteId,
}

/// The withdrawal event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InviteRevokeOut {
    /// The room.
    pub room_id: RoomId,
    /// The invite.
    pub invite_id: InviteId,
    /// The authored `invite_revoked` event.
    pub event_id: EventId,
    /// Its position.
    pub pos: u64,
    /// When it was revoked.
    pub revoked_at: Timestamp,
}
operation!(InviteRevoke, InviteRevokeOut, "invite.revoke", true);

/// `invite.redeem` — convert a capability into signed membership. The only
/// operation reachable by a non-member; its authorization object is the
/// key-bound capability itself, never an identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteRedeem {
    /// The capability.
    pub capability: String,
}

/// The new membership.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InviteRedeemOut {
    /// The room joined.
    pub room_id: RoomId,
    /// The new member's subject.
    pub subject_id: SubjectId,
    /// The role joined with.
    pub role: Role,
    /// Always `active`.
    pub standing: Standing,
    /// The authored `member_joined` event.
    pub event_id: EventId,
    /// Its position.
    pub pos: u64,
    /// `true` when this call authored the membership, `false` on replay.
    pub joined: bool,
}
operation!(InviteRedeem, InviteRedeemOut, "invite.redeem", true);

// ---------------------------------------------------------------------------
// Timeline
// ---------------------------------------------------------------------------

/// `message.send` — author a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSend {
    /// The room.
    pub room_id: RoomId,
    /// The body.
    pub body: String,
}

/// The authored event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageSendOut {
    /// The room.
    pub room_id: RoomId,
    /// The event id.
    pub event_id: EventId,
    /// Its position — the same space as the room's push stream.
    pub pos: u64,
    /// When it was authored.
    pub at: Timestamp,
}
operation!(MessageSend, MessageSendOut, "message.send", true);

/// `status.post` — author an agent status. Open to any active member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusPost {
    /// The room.
    pub room_id: RoomId,
    /// The closed-vocabulary label. **No free-text field exists** — a
    /// `message` key is `invalid_argument` / `unrecognised_field`.
    pub label: StatusLabel,
    /// Progress.
    pub progress: Progress,
}

/// The authored status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusPostOut {
    /// The room.
    pub room_id: RoomId,
    /// The event id.
    pub event_id: EventId,
    /// Its position.
    pub pos: u64,
    /// When it was authored.
    pub at: Timestamp,
    /// The derived severity — served, never sent, never re-derived.
    pub severity: Severity,
}
operation!(StatusPost, StatusPostOut, "status.post", true);

/// `status.history` — read status history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusHistory {
    /// The room.
    pub room_id: RoomId,
    /// Whose history.
    pub subject_id: SubjectId,
    /// Paging.
    #[serde(flatten)]
    pub page: Page,
}

/// One entry — **no `pos` and no `event_id`**; positions are read from
/// `room.timeline`, the one position space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusEntry {
    /// When it was posted.
    pub at: Timestamp,
    /// Its label.
    pub label: StatusLabel,
    /// Its derived severity.
    pub severity: Severity,
    /// Its progress.
    pub progress: Progress,
}

/// The history page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusHistoryOut {
    /// The room.
    pub room_id: RoomId,
    /// Whose history.
    pub subject_id: SubjectId,
    /// One entry per real posted event, chronological.
    pub entries: Vec<StatusEntry>,
    /// The one continuation mechanism.
    pub truncated: Truncated,
}
operation!(StatusHistory, StatusHistoryOut, "status.history", false);

/// `fleet.list` — the agent fleet projection. **No tallies.**
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetList {}

/// One fleet row. An agent is a member that has authored at least one
/// `status.post`; agent-ness is derived, not declared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FleetRow {
    /// The agent's subject.
    pub subject_id: SubjectId,
    /// The room.
    pub room_id: RoomId,
    /// Derived liveness, computed at read time.
    pub liveness: Liveness,
    /// The agent's newest status, or `absent`.
    pub latest_status: LatestStatus,
    /// When the agent was last observed, or `absent`.
    pub last_seen: LastSeen,
}

/// The fleet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FleetListOut {
    /// The agents.
    pub agents: Vec<FleetRow>,
}
operation!(FleetList, FleetListOut, "fleet.list", false);

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// `file.share` — share bytes into a room. **No filesystem path appears**:
/// v1's `path` is removed; `PlatformServices` owns paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileShare {
    /// The room.
    pub room_id: RoomId,
    /// The declared name.
    pub name: String,
    /// Declared byte count, checked at `stage_declared` and `stage_stream`.
    pub declared_bytes: u64,
    /// Peer-declared, untrusted content type — never `content_type`.
    pub declared_content_type: String,
}

/// The shared file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileShareOut {
    /// The room.
    pub room_id: RoomId,
    /// The file id.
    pub file_id: FileId,
    /// The authored `file_shared` event.
    pub event_id: EventId,
    /// Its position.
    pub pos: u64,
    /// The accepted byte count.
    pub bytes: u64,
    /// The content digest.
    pub digest: String,
}
operation!(FileShare, FileShareOut, "file.share", true);

/// `file.list` — files shared into a room, provider availability as a
/// protocol fact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileList {
    /// The room.
    pub room_id: RoomId,
    /// Paging.
    #[serde(flatten)]
    pub page: Page,
}

/// One file row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileRow {
    /// The file id.
    pub file_id: FileId,
    /// Its declared name.
    pub name: String,
    /// Its byte count.
    pub bytes: u64,
    /// Its digest — served here so a fetcher can verify.
    pub digest: String,
    /// Peer-declared, untrusted content type.
    pub declared_content_type: String,
    /// Who shared it.
    pub shared_by: SubjectId,
    /// When.
    pub shared_at: Timestamp,
    /// Provider devices carrying evidence (reachability as a fact).
    pub providers: Vec<PeerRow>,
    /// Whether a fetch can be served now.
    pub fetchable: bool,
    /// Whether this daemon holds the bytes.
    pub self_hosted: bool,
}

/// The files page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileListOut {
    /// The room.
    pub room_id: RoomId,
    /// The files.
    pub files: Vec<FileRow>,
    /// The one continuation mechanism.
    pub truncated: Truncated,
}
operation!(FileList, FileListOut, "file.list", false);

/// `file.fetch` — fetch a file's bytes from a provider. **No `save_dir`**:
/// the daemon holds the bytes and `file.read` streams them out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFetch {
    /// The room.
    pub room_id: RoomId,
    /// The file.
    pub file_id: FileId,
}

/// The fetch result. Does **not** report local hold state — that is
/// `file.read` succeeding or a `file.list` row's `self_hosted`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileFetchOut {
    /// The room.
    pub room_id: RoomId,
    /// The file.
    pub file_id: FileId,
    /// The byte count fetched.
    pub bytes: u64,
    /// The verified digest.
    pub digest: String,
    /// The provider the bytes came from.
    pub provider: ProviderRef,
}

/// A provider reference (subject + device).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRef {
    /// The provider's subject.
    pub subject_id: SubjectId,
    /// The provider's device.
    pub device_id: DeviceId,
}
operation!(FileFetch, FileFetchOut, "file.fetch", true);

/// `file.read` — stream locally held bytes out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRead {
    /// The room.
    pub room_id: RoomId,
    /// The file.
    pub file_id: FileId,
}

/// The read header; the bytes follow as a stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileReadOut {
    /// The room.
    pub room_id: RoomId,
    /// The file.
    pub file_id: FileId,
    /// The byte count.
    pub bytes: u64,
    /// Peer-declared, untrusted content type — **a client MUST NOT render
    /// peer-supplied bytes inline on the strength of it.**
    pub declared_content_type: String,
}
operation!(FileRead, FileReadOut, "file.read", false);

/// `transfer.cancel` — cancel a transfer by the `op_id` that started it.
/// The request field is `transfer_op_id`, not `op_id` — one wire name never
/// means two things.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferCancel {
    /// The transfer's op_id.
    pub transfer_op_id: OpId,
}

/// The cancel result — idempotence is observable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransferCancelOut {
    /// The transfer's op_id.
    pub transfer_op_id: OpId,
    /// `cancelled` or `already_cancelled`.
    pub outcome: CancelOutcome,
    /// Bytes transferred before cancellation.
    pub transferred_bytes: u64,
    /// The total, or genuinely unknown.
    pub total: ByteTotal,
}
operation!(TransferCancel, TransferCancelOut, "transfer.cancel", true);

// ---------------------------------------------------------------------------
// Pipes
// ---------------------------------------------------------------------------

/// `pipe.publish` — publish a pipe to a loopback target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipePublish {
    /// The room.
    pub room_id: RoomId,
    /// The loopback target (one object).
    pub target: Target,
    /// Who may connect — required, never defaulted.
    pub audience: Audience,
}

/// The published pipe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipePublishOut {
    /// The room.
    pub room_id: RoomId,
    /// The pipe id.
    pub pipe_id: PipeId,
    /// The target, echoed.
    pub target: Target,
    /// The audience, echoed.
    pub audience: Audience,
    /// The authored `pipe_published` event.
    pub event_id: EventId,
    /// Its position.
    pub pos: u64,
}
operation!(PipePublish, PipePublishOut, "pipe.publish", true);

/// `pipe.list` — pipes in a room, local connection and publisher
/// reachability as two separately named facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipeList {
    /// The room.
    pub room_id: RoomId,
    /// Paging.
    #[serde(flatten)]
    pub page: Page,
}

/// One pipe row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipeRow {
    /// The pipe id.
    pub pipe_id: PipeId,
    /// Who published it.
    pub published_by: SubjectId,
    /// On which device.
    pub device_id: DeviceId,
    /// When.
    pub published_at: Timestamp,
    /// The publisher's reachability (a protocol fact).
    pub link: Link,
    /// Whether this daemon holds a local connection (a runtime fact).
    pub connected: bool,
}

/// The pipes page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipeListOut {
    /// The room.
    pub room_id: RoomId,
    /// The pipes.
    pub pipes: Vec<PipeRow>,
    /// The one continuation mechanism.
    pub truncated: Truncated,
}
operation!(PipeList, PipeListOut, "pipe.list", false);

/// `pipe.connect` — connect to a pipe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipeConnect {
    /// The room.
    pub room_id: RoomId,
    /// The pipe.
    pub pipe_id: PipeId,
}

/// The connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipeConnectOut {
    /// The room.
    pub room_id: RoomId,
    /// The pipe.
    pub pipe_id: PipeId,
    /// The local connection identity.
    pub connection_id: String,
    /// The local endpoint the connection is bound to.
    pub local: Target,
}
operation!(PipeConnect, PipeConnectOut, "pipe.connect", true);

/// `pipe.release` — release a local connection, named by the connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipeRelease {
    /// The connection.
    pub connection_id: String,
}

/// The release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipeReleaseOut {
    /// The connection.
    pub connection_id: String,
    /// Always `true`.
    pub released: bool,
}
operation!(PipeRelease, PipeReleaseOut, "pipe.release", true);

/// `pipe.revoke` — withdraw a published pipe as a signed fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipeRevoke {
    /// The room.
    pub room_id: RoomId,
    /// The pipe.
    pub pipe_id: PipeId,
}

/// The withdrawal event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipeRevokeOut {
    /// The room.
    pub room_id: RoomId,
    /// The pipe.
    pub pipe_id: PipeId,
    /// The authored `pipe_revoked` event.
    pub event_id: EventId,
    /// Its position.
    pub pos: u64,
    /// When it was revoked.
    pub revoked_at: Timestamp,
}
operation!(PipeRevoke, PipeRevokeOut, "pipe.revoke", true);

// ---------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------

/// `stream.subscribe` — subscribe a connection to a room's pushes.
/// Naturally idempotent; scoped to the connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamSubscribe {
    /// The room.
    pub room_id: RoomId,
    /// Where to start — a cursor (`{"state": "start"}` names no position).
    pub from: Cursor,
}

/// The subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamSubscribeOut {
    /// The room.
    pub room_id: RoomId,
    /// The concrete position the cursor resolved to — what the client needs
    /// to detect its first gap.
    pub from_pos: u64,
}
operation!(
    StreamSubscribe,
    StreamSubscribeOut,
    "stream.subscribe",
    false
);

/// `stream.unsubscribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamUnsubscribe {
    /// The room.
    pub room_id: RoomId,
}

/// The unsubscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamUnsubscribeOut {
    /// The room.
    pub room_id: RoomId,
    /// Always `true`.
    pub unsubscribed: bool,
}
operation!(
    StreamUnsubscribe,
    StreamUnsubscribeOut,
    "stream.unsubscribe",
    false
);

/// `stream.resync` — the **authoritative** recovery. The client names the
/// last position it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamResync {
    /// The room.
    pub room_id: RoomId,
    /// The last position held (recovery is defined over positions).
    pub from_pos: u64,
}

/// The resync page — exactly one continuation mechanism.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamResyncOut {
    /// The room.
    pub room_id: RoomId,
    /// The events since `from_pos`.
    pub events: Vec<Event>,
    /// The position of the last event returned, or `from_pos` when empty.
    pub next_pos: u64,
    /// The one continuation mechanism.
    pub truncated: Truncated,
}
operation!(StreamResync, StreamResyncOut, "stream.resync", false);
