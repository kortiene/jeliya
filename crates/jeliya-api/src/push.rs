//! Push frames, the error taxonomy, and lifecycle-visible views.
//!
//! A push carries `t` and never `id`; a reply carries `id` and never `t`.
//! Every error carries a machine-readable `code` plus operation-specific
//! typed fields — the v1 `hint` field is removed (hardcoded English;
//! localization is the client's job).

use crate::ids::*;
use crate::shared::*;
use crate::types::Event;
use serde::{Deserialize, Serialize};

/// An attempted provider, carrying the same per-device link evidence
/// `file.list` serves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptedProvider {
    /// The provider's subject.
    pub subject_id: SubjectId,
    /// The provider's device.
    pub device_id: DeviceId,
    /// The link the attempt was made over, or why it was not connected.
    pub link: Link,
}

// ---------------------------------------------------------------------------
// Pushes — `t` is closed at four
// ---------------------------------------------------------------------------

/// A push frame. `t` is closed at four; a frame type not listed does not
/// exist. Every push carries a per-room monotonic position; across rooms no
/// ordering is defined and a client MUST NOT infer one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Push {
    /// A room event committed.
    Event {
        /// The room.
        room_id: RoomId,
        /// The committed event, inline.
        #[serde(flatten)]
        event: Event,
    },
    /// A position discontinuity, detected or forced.
    Gap {
        /// The room.
        room_id: RoomId,
        /// The position the gap starts after.
        from_pos: u64,
        /// Where the gap ends.
        to: GapTo,
        /// Why it exists.
        reason: GapReason,
    },
    /// A peer's link changed. Depends on U1: `generation` is the connection
    /// generation that makes a stale teardown discardable from the frame
    /// alone.
    Peer {
        /// The room.
        room_id: RoomId,
        /// The peer's subject.
        subject_id: SubjectId,
        /// The peer's device.
        device_id: DeviceId,
        /// The new link state.
        link: Link,
        /// The connection generation.
        generation: u64,
    },
    /// A transfer made progress. Goes **only** to the principal that
    /// started the transfer.
    Transfer {
        /// The transfer's op_id.
        transfer_op_id: OpId,
        /// Bytes transferred so far.
        transferred_bytes: u64,
        /// The total, or genuinely unknown.
        total: ByteTotal,
    },
}

// ---------------------------------------------------------------------------
// The error taxonomy
// ---------------------------------------------------------------------------

/// The closed set of error codes. Every operation has at least one code
/// specific to it; the taxonomy is the whole of what an adapter may raise.
/// Serde rename uses the wire spellings verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    // Handshake and generation gate
    /// Unsupported protocol generation.
    ProtocolUnsupported,
    /// Storage generation mismatch.
    StorageGenerationMismatch,
    /// The credential is absent, wrong, or a spent ticket.
    Unauthenticated,
    /// The daemon is not yet serving, or its subject store cannot be read.
    NotReady,
    /// A frame exceeds `max_frame_bytes`.
    FrameTooLarge,
    /// No activity within `idle_timeout_ms`.
    IdleTimeout,
    /// A submitted pairing code is not currently redeemable.
    PairingCodeInvalid,
    /// A browser session credential is past its TTL or was revoked.
    SessionExpired,
    /// The upgrade's `Origin`/`Host` is not loopback.
    ForbiddenOrigin,

    // Envelope and structure
    /// The frame is not JSON, or decodes to no envelope with a usable `id`.
    MalformedFrame,
    /// `op` names no operation in this generation.
    UnknownOperation,
    /// Validation-order step 1 refused.
    InvalidArgument,
    /// A replayed envelope `op_id` collided with a different payload.
    OpIdConflict,
    /// A served limit was reached by consumption.
    ResourceExhausted,

    // Subject and daemon
    /// The operation needs a local subject and none exists.
    SubjectAbsent,
    /// The subject store cannot be written.
    SubjectStoreUnwritable,
    /// A stop is already in progress.
    ShutdownInProgress,

    // Rooms and membership
    /// No such room, or the caller cannot see it (deliberately one answer).
    RoomNotAvailable,
    /// The room index cannot be read.
    RoomIndexUnreadable,
    /// The name fails the stated bounds.
    RoomNameInvalid,
    /// The caller is the room's only authority.
    SoleAuthorityCannotLeave,
    /// A well-formed cursor names a position the store can no longer serve.
    CursorUnknown,
    /// `room.activate` cannot bring the room live.
    TransportUnavailable,
    /// `member.remove` names an authority.
    AuthorityCannotBeRemoved,
    /// The named subject is not a member of this room.
    MemberUnknown,
    /// The caller's standing is `left` or `removed`.
    MembershipEnded,
    /// The caller's role is below what the operation needs.
    InsufficientStanding,
    /// The membership fold cannot be built.
    MembershipUnresolved,
    /// `room.archive` on a room the caller still belongs to.
    RoomStillActive,
    /// The operation requires a live room.
    RoomNotLive,

    // Invitations
    /// `invite.mint` named an identity already an active member.
    InviteeAlreadyMember,
    /// `invite.mint` named a role this record does not permit minting.
    RoleNotGrantable,
    /// The invite index cannot be read.
    InviteIndexUnreadable,
    /// No such invite for this authority.
    InviteUnknown,
    /// The capability is not a currently valid one.
    CapabilityInvalid,
    /// The capability is past its absolute expiry.
    CapabilityExpired,
    /// The capability was withdrawn.
    CapabilityRevoked,
    /// The capability was already converted into membership.
    CapabilityRedeemed,

    // Timeline
    /// The message body exceeds `max_message_body_bytes`.
    MessageTooLarge,
    /// The label is outside the closed vocabulary.
    StatusLabelUnknown,
    /// `status.history` named a subject with no history.
    StatusSubjectUnknown,
    /// The fleet projection cannot be built.
    FleetProjectionUnavailable,

    // Files and transfers
    /// The streamed bytes disagree with the declared size.
    DeclaredSizeMismatch,
    /// The file exceeds `max_shared_file_bytes`.
    FileTooLarge,
    /// The file index cannot be read.
    FileIndexUnreadable,
    /// No reachable provider for the file.
    ProviderUnreachable,
    /// The file id is not in this room's history.
    FileUnknown,
    /// `file.read` on bytes not held locally.
    FileNotFetched,
    /// Content did not verify. **Never returned for a size refusal.**
    DigestMismatch,
    /// No forward progress within `transfer_stall_ms`.
    TransferStalled,
    /// The size-aware absolute transfer budget expired.
    TransferDeadlineExceeded,
    /// An admitted transfer ended without a more specific operation error.
    StreamAborted,
    /// No such transfer for this principal.
    TransferUnknown,

    // Pipes
    /// The publish target is not allowed (loopback policy).
    PipeTargetRefused,
    /// Publishing is not permitted in this room.
    PolicyRefused,
    /// The pipe index cannot be read.
    PipeIndexUnreadable,
    /// No such pipe visible to this caller.
    PipeUnknown,
    /// The publisher's device cannot be reached.
    PipeUnreachable,
    /// The pipe was revoked.
    PipeRevoked,
    /// The caller is not the pipe's publisher.
    PipeNotPublisher,
    /// No such connection.
    ConnectionUnknown,

    // Stream
    /// The connection holds `max_subscriptions_per_connection`.
    SubscriptionLimitReached,
    /// No such subscription on this connection.
    SubscriptionUnknown,
    /// The named position can no longer be served; discard and re-read.
    ResyncRequired,
}

/// A wire error: a machine-readable `code` plus operation-specific typed
/// fields. **No `hint` and no prose** — localization is the client's job,
/// and no daemon text may be fabricated.
///
/// The error is **code-discriminated**: each variant carries exactly the
/// typed payload the taxonomy row fixes, no more and no fewer, so an
/// adapter cannot construct a protocol-invalid error (a `file_too_large`
/// without `declared_bytes`, `limit_bytes`, and `enforced_at` is
/// unrepresentable). Serde flattens the variant's fields beside `code`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ApiError {
    // Handshake and generation gate
    /// `Host`, or a present `Origin`, is not loopback.
    ForbiddenOrigin,
    /// `v` is absent or names an unsupported generation.
    ProtocolUnsupported {
        /// The generations this daemon supports.
        supported: Vec<u64>,
        /// The client's declared generation, or `absent`.
        client: DeclaredVersion,
    },
    /// `sg` is absent or does not equal the daemon's.
    StorageGenerationMismatch {
        /// The daemon's storage generation.
        daemon: u64,
        /// The client's declared generation, or `absent`.
        client: DeclaredGeneration,
    },
    /// The credential is absent, wrong, or a spent ticket.
    Unauthenticated,
    /// The daemon is not yet serving, or its subject store cannot be read.
    NotReady,
    /// A frame exceeds `max_frame_bytes`; closes `4005` unparsed.
    FrameTooLarge {
        /// The served frame limit.
        limit_bytes: u64,
    },
    /// No activity within `idle_timeout_ms`; closes `4004`.
    IdleTimeout {
        /// The served idle window.
        idle_ms: u64,
    },
    /// A submitted pairing code is not currently redeemable.
    PairingCodeInvalid {
        /// Why not.
        reason: PairingCodeInvalidReason,
    },
    /// A browser session credential is past its TTL or was revoked.
    SessionExpired,

    // Envelope and structure
    /// The frame is not JSON, or decodes to no envelope with a usable `id`.
    MalformedFrame,
    /// `op` names no operation in this generation.
    UnknownOperation {
        /// The unrecognized operation name.
        op: String,
    },
    /// Validation-order step 1 refused.
    InvalidArgument {
        /// Dotted path into the frame.
        field: String,
        /// Why it is invalid.
        reason: InvalidReason,
    },
    /// The `op_id` was seen before with a different request body.
    OpIdConflict {
        /// The conflicting key.
        op_id: OpId,
    },
    /// A served limit was reached by consumption.
    ResourceExhausted {
        /// The resource.
        resource: String,
        /// The limit.
        limit: u64,
    },

    // Subject and daemon
    /// The operation needs a local subject and none exists.
    SubjectAbsent,
    /// `subject.ensure` cannot persist the subject it created.
    SubjectStoreUnwritable,
    /// A `daemon.stop` is already sequenced.
    ShutdownInProgress,

    // Rooms and membership
    /// No such room, or the caller cannot see it (deliberately one answer).
    RoomNotAvailable {
        /// The room.
        room_id: RoomId,
    },
    /// The accepted-room index cannot be read.
    RoomIndexUnreadable,
    /// The name fails the stated bounds.
    RoomNameInvalid {
        /// Why (the `invalid_argument` variant).
        reason: InvalidReason,
    },
    /// The caller is the room's only authority.
    SoleAuthorityCannotLeave {
        /// The room.
        room_id: RoomId,
    },
    /// A well-formed cursor names a position the store can no longer serve.
    CursorUnknown {
        /// The cursor.
        cursor: Cursor,
    },
    /// `room.activate` cannot bring the room live.
    TransportUnavailable {
        /// The room.
        room_id: RoomId,
    },
    /// `member.remove` names an authority.
    AuthorityCannotBeRemoved {
        /// The room.
        room_id: RoomId,
        /// The authority.
        subject_id: SubjectId,
    },
    /// The named subject is not a member of this room.
    MemberUnknown {
        /// The room.
        room_id: RoomId,
        /// The subject.
        subject_id: SubjectId,
    },
    /// The caller's standing is `left` or `removed`.
    MembershipEnded {
        /// The room.
        room_id: RoomId,
        /// The caller's standing.
        standing: Standing,
    },
    /// The caller's role is below what the operation needs.
    InsufficientStanding {
        /// The room.
        room_id: RoomId,
        /// The role required.
        required: Role,
        /// The role held.
        held: Role,
    },
    /// The membership fold cannot resolve a member's standing.
    MembershipUnresolved {
        /// The room.
        room_id: RoomId,
        /// The member.
        subject_id: SubjectId,
    },
    /// `room.archive` on a room the caller still belongs to.
    RoomStillActive {
        /// The room.
        room_id: RoomId,
    },
    /// The operation requires a live room.
    RoomNotLive {
        /// The room.
        room_id: RoomId,
    },

    // Invitations
    /// `invite.mint` named an identity already an active member.
    InviteeAlreadyMember {
        /// The room.
        room_id: RoomId,
        /// The identity.
        subject_id: SubjectId,
    },
    /// `invite.mint` named a role this record does not permit minting.
    RoleNotGrantable {
        /// The requested role.
        requested: Role,
    },
    /// The invite index cannot be read.
    InviteIndexUnreadable,
    /// No such invite for this authority.
    InviteUnknown {
        /// The invite.
        invite_id: InviteId,
    },
    /// The capability does not verify.
    CapabilityInvalid,
    /// The capability's absolute expiry has passed.
    CapabilityExpired {
        /// When it expired.
        expired_at: Timestamp,
    },
    /// The capability was withdrawn.
    CapabilityRevoked {
        /// When it was revoked.
        revoked_at: Timestamp,
    },
    /// The capability was already converted into membership.
    CapabilityRedeemed {
        /// When it was redeemed.
        redeemed_at: Timestamp,
    },

    // Timeline
    /// The body exceeds `max_message_body_bytes`.
    MessageTooLarge {
        /// The declared size.
        declared_bytes: u64,
        /// The served limit.
        limit_bytes: u64,
    },
    /// The label is outside the closed vocabulary.
    StatusLabelUnknown {
        /// The unrecognized label.
        label: String,
    },
    /// `status.history` named a subject with no history.
    StatusSubjectUnknown {
        /// The room.
        room_id: RoomId,
        /// The subject.
        subject_id: SubjectId,
    },
    /// The fleet projection cannot be built.
    FleetProjectionUnavailable,

    // Files and transfers
    /// `file.share`'s streamed bytes did not match its declared size.
    DeclaredSizeMismatch {
        /// The declared size.
        declared_bytes: u64,
        /// The observed size.
        observed_bytes: u64,
    },
    /// The file exceeds `max_shared_file_bytes`.
    FileTooLarge {
        /// The declared size.
        declared_bytes: u64,
        /// The served limit.
        limit_bytes: u64,
        /// Which enforcement point fired.
        enforced_at: EnforcedAt,
    },
    /// The room's file index cannot be read.
    FileIndexUnreadable,
    /// No such file in this room.
    FileUnknown {
        /// The file.
        file_id: FileId,
    },
    /// `file.read` named a file whose bytes are not held locally.
    FileNotFetched {
        /// The file.
        file_id: FileId,
    },
    /// No provider holding the file could be reached.
    ProviderUnreachable {
        /// The file.
        file_id: FileId,
        /// The **attempted** provider rows, each `{subject_id, device_id,
        /// link}` — the same per-device link evidence `file.list` carries,
        /// so a client can say *why* each attempt failed, not only that the
        /// fetch did.
        providers: Vec<AttemptedProvider>,
    },
    /// Content did not verify. **Never returned for a size refusal.**
    DigestMismatch {
        /// The expected digest.
        expected: String,
        /// The observed digest.
        observed: String,
    },
    /// No forward progress within the stall window.
    TransferStalled {
        /// Bytes transferred before the stall.
        transferred_bytes: u64,
        /// The total, or genuinely unknown.
        total: ByteTotal,
    },
    /// The size-aware absolute transfer budget expired.
    TransferDeadlineExceeded {
        /// Bytes transferred before expiry.
        transferred_bytes: u64,
        /// The total, or genuinely unknown.
        total: ByteTotal,
        /// The admitted transfer's absolute budget in milliseconds.
        budget_ms: u64,
    },
    /// An admitted transfer ended without a more specific operation error.
    StreamAborted {
        /// Bytes accepted before the abort.
        transferred_bytes: u64,
        /// The total, or genuinely unknown.
        total: ByteTotal,
        /// Why the stream ended.
        reason: StreamAbortReason,
    },
    /// No such in-flight transfer for this principal.
    TransferUnknown {
        /// The transfer's op_id.
        transfer_op_id: OpId,
    },

    // Pipes
    /// The publish target is not allowed (loopback policy).
    PipeTargetRefused {
        /// The rejected target, verbatim.
        target: Target,
    },
    /// Publishing is not permitted in this room.
    PolicyRefused {
        /// The room.
        room_id: RoomId,
    },
    /// The room's pipe index cannot be read.
    PipeIndexUnreadable,
    /// No such pipe, or the caller is outside its audience.
    PipeUnknown {
        /// The pipe.
        pipe_id: PipeId,
    },
    /// The publisher's device could not be reached.
    PipeUnreachable {
        /// The pipe.
        pipe_id: PipeId,
        /// The last known link.
        link: Link,
    },
    /// The pipe was withdrawn.
    PipeRevoked {
        /// The pipe.
        pipe_id: PipeId,
        /// When it was revoked.
        revoked_at: Timestamp,
    },
    /// `pipe.revoke` named a pipe this subject did not publish.
    PipeNotPublisher {
        /// The pipe.
        pipe_id: PipeId,
    },
    /// No such local connection.
    ConnectionUnknown {
        /// The connection.
        connection_id: String,
    },

    // Stream
    /// The connection holds `max_subscriptions_per_connection`.
    SubscriptionLimitReached {
        /// The served limit.
        limit: u64,
    },
    /// No such subscription on this connection.
    SubscriptionUnknown {
        /// The room.
        room_id: RoomId,
    },
    /// The named position can no longer be served; discard and re-read.
    ResyncRequired {
        /// The room.
        room_id: RoomId,
        /// The position to re-read from.
        from_pos: u64,
    },
}

/// The client's declared protocol generation, or its stated absence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DeclaredVersion {
    /// A declared generation.
    Declared {
        /// The generation.
        v: u64,
    },
    /// Nothing declared.
    Absent,
}

/// The client's declared storage generation, or its stated absence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DeclaredGeneration {
    /// A declared generation.
    Declared {
        /// The generation.
        sg: u64,
    },
    /// Nothing declared.
    Absent,
}

/// Why a pairing code is not redeemable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PairingCodeInvalidReason {
    /// No such code is outstanding.
    Unknown,
    /// The code is past its TTL.
    Expired,
    /// The code was already used.
    Spent,
    /// The code was voided by too many failed attempts.
    Voided,
}

/// Which size-enforcement point fired. The record names **five daemon-side**
/// points of the six in the shared-file size policy; the sixth is the
/// client preflight, which by definition never reaches the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcedAt {
    /// The declared size was refused before any byte was read.
    StageDeclared,
    /// The streamed total exceeded the declaration after the fact.
    StageStream,
    /// An in-process authoring check rejected the share.
    Authoring,
    /// A fetch preflight refused the signed size before contacting a
    /// provider.
    FetchPreflight,
    /// A fetch stream was aborted when the bytes exceeded the declaration.
    FetchStream,
}

/// The closed `invalid_argument.reason` variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum InvalidReason {
    /// A required key is absent.
    Missing,
    /// A key the operation does not define.
    UnrecognisedField,
    /// Wrong JSON type.
    Type {
        /// What was expected.
        expected: String,
    },
    /// Right type, unparseable as its domain.
    Format,
    /// A numeric or length bound was violated.
    Bound {
        /// Inclusive minimum.
        min: u64,
        /// Inclusive maximum.
        max: u64,
    },
}

// ---------------------------------------------------------------------------
// Lifecycle-visible views
// ---------------------------------------------------------------------------

/// The shared discovery version object. The ready line, the portfile, and
/// the Layer 0 health endpoint carry an **identical** `{protocol,
/// min_protocol, storage_generation, limits}` object, defined once here —
/// two version axes on one artifact, or three producers each defining the
/// shape, is the drift this single definition exists to prevent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionInfo {
    /// The protocol generation (always 2).
    pub protocol: u64,
    /// The minimum supported protocol generation (always 2 — one
    /// generation at a time).
    pub min_protocol: u64,
    /// The storage generation.
    pub storage_generation: u64,
    /// The served limits.
    pub limits: Limits,
}

/// The `hello` frame — the daemon's first frame after upgrade, exactly one,
/// carrying `t: "hello"` as its discriminator. Carries no `pid`, no `port`,
/// and no `data_dir`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename = "hello")]
pub struct Hello {
    /// The protocol generation (always 2).
    pub protocol: u64,
    /// The storage generation.
    pub storage_generation: u64,
    /// The daemon incarnation identity: a per-process nonce, fresh at each
    /// daemon start and identical for every connection of one process. A
    /// client compares it across reconnects to fence replay of keyed
    /// mutations — the dedup ledger is in-memory, so a changed incarnation
    /// means a restarted daemon with an empty ledger (orthogonal to
    /// `storage_generation`, which is persistent).
    pub incarnation: Incarnation,
    /// The served limits.
    pub limits: Limits,
    /// The local subject, or its stated absence.
    pub subject: SubjectState,
    /// Whether this connection continues a prior stream.
    pub resume: Resume,
}

/// The served limits object — read from the wire, never compiled in.
/// Every field is required: a missing limit forces a client back to a
/// hardcoded constant, which is the defect served limits exist to end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// The shared-file maximum (104,857,600 bytes per the size policy).
    pub max_shared_file_bytes: u64,
    /// The message body maximum.
    pub max_message_body_bytes: u64,
    /// The frame maximum.
    pub max_frame_bytes: u64,
    /// Maximum in-flight requests per connection.
    pub max_inflight_requests: u64,
    /// Room subscriptions one connection may hold. Exceeding it is
    /// `subscription_limit_reached`, never a silent drop.
    pub max_subscriptions_per_connection: u64,
    /// Maximum connections.
    pub max_connections: u64,
    /// Maximum concurrent transfers.
    pub max_concurrent_transfers: u64,
    /// Maximum transfer bytes in flight.
    pub max_transfer_bytes_inflight: u64,
    /// Per-provider connection allowance.
    pub transfer_connect_allowance_ms: u64,
    /// The floor the transfer deadline is computed from.
    pub transfer_floor_bits_per_second: u64,
    /// Zero-forward-progress window before `transfer_stalled`.
    pub transfer_stall_ms: u64,
    /// Largest timeline page (governs all six paging operations).
    pub timeline_page_max: u64,
    /// Inactivity after which the daemon closes with `4004`.
    pub idle_timeout_ms: u64,
    /// Lifetime a pairing code is granted when issued (policy value).
    pub pairing_code_ttl_ms: u64,
    /// Failed submissions against one outstanding code before it is voided.
    pub pairing_code_max_attempts: u64,
    /// Lifetime of a browser session credential.
    pub browser_session_ttl_ms: u64,
}
