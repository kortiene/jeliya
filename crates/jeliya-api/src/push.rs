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
/// The `fields` carry the code's typed payload (for example
/// `insufficient_standing { required, held }`). They are typed where the
/// record fixes a shape and deliberately minimal: an adapter constructs
/// errors through the typed constructors, never by stuffing raw JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    /// The machine-readable code.
    pub code: ErrorCode,
    /// The dotted path into the frame for `invalid_argument`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// The closed reason variant for `invalid_argument`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<InvalidReason>,
    /// The room for room-scoped refusals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_id: Option<RoomId>,
    /// The subject for subject-scoped refusals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<SubjectId>,
    /// Required vs held standing for `insufficient_standing`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Role>,
    /// The role the caller holds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held: Option<Role>,
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

/// The `hello` frame — the daemon's first frame after upgrade, exactly one.
/// Carries no `pid`, no `port`, and no `data_dir`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    /// The protocol generation (always 2).
    pub protocol: u64,
    /// The storage generation.
    pub storage_generation: u64,
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
