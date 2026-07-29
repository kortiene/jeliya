//! The committed event and its content kinds. `kind` is closed at ten, and
//! each arm fixes its `content` — an event whose kind a client does not
//! recognise is not rendered and not counted, which is why [`EventKind`]
//! has no unknown arm and deserialization of an unrecognized kind fails.

use crate::ids::{EventId, FileId, InviteId, PipeId, SubjectId};
use crate::shared::{Audience, Author, Progress, Role, Target};
use serde::{Deserialize, Serialize};

/// The closed set of committed-event kinds. Each is authored by exactly one
/// operation, and every operation that returns an `event_id` authors one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// `room.create`
    RoomCreated,
    /// `message.send`
    Message,
    /// `status.post`
    AgentStatus,
    /// `invite.redeem`
    MemberJoined,
    /// `room.leave`
    MemberLeft,
    /// `member.remove`
    MemberRemoved,
    /// `invite.revoke`
    InviteRevoked,
    /// `file.share`
    FileShared,
    /// `pipe.publish`
    PipePublished,
    /// `pipe.revoke`
    PipeRevoked,
}

/// A committed room event: its per-room position, id, author-dated instant,
/// attribution, and **one coupled kind-and-content**. `kind` and `content`
/// are a single enum because the record states each of the ten kinds fixes
/// exactly one content shape — an invalid combination (a `message` kind
/// with `room_created` content) is unrepresentable, not merely rejected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Per-room monotonic position — the same space the push stream uses.
    pub pos: u64,
    /// The event's id.
    pub event_id: EventId,
    /// When the author dated it (non-repudiable, signed).
    pub at: crate::Timestamp,
    /// Its attribution, or the explicit unresolved variant.
    pub author: Author,
    /// The kind and its content, coupled: each kind fixes its content.
    #[serde(flatten)]
    pub kind: EventKindContent,
}

/// One coupled kind-and-content. Serde encodes the kind as `kind` and the
/// content inline, so the wire shape is the record's
/// `{ "kind": "message", "content": { "body": "…" } }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "content", rename_all = "snake_case")]
pub enum EventKindContent {
    /// `room.create` authored it.
    RoomCreated {
        /// The room's name.
        name: String,
    },
    /// `message.send` authored it.
    Message {
        /// The message body.
        body: String,
    },
    /// `status.post` authored it. Carries **no `severity`** — severity is
    /// derived and served on projections, never written into signed content.
    AgentStatus {
        /// The closed-vocabulary label.
        label: crate::StatusLabel,
        /// Its progress variant.
        progress: Progress,
    },
    /// `invite.redeem` authored it.
    MemberJoined {
        /// Who joined.
        subject_id: SubjectId,
        /// The role they joined with.
        role: Role,
    },
    /// `room.leave` authored it.
    MemberLeft {
        /// Who left.
        subject_id: SubjectId,
    },
    /// `member.remove` authored it.
    MemberRemoved {
        /// Who was removed.
        subject_id: SubjectId,
        /// Who removed them.
        by: SubjectId,
    },
    /// `invite.revoke` authored it.
    InviteRevoked {
        /// The revoked invite.
        invite_id: InviteId,
    },
    /// `file.share` authored it.
    FileShared {
        /// The file's id.
        file_id: FileId,
        /// Its declared name.
        name: String,
        /// Its byte count.
        bytes: u64,
        /// Its content digest.
        digest: String,
    },
    /// `pipe.publish` authored it.
    PipePublished {
        /// The pipe's id.
        pipe_id: PipeId,
        /// Its publish target.
        target: Target,
        /// Its audience.
        audience: Audience,
    },
    /// `pipe.revoke` authored it.
    PipeRevoked {
        /// The revoked pipe.
        pipe_id: PipeId,
    },
}

impl EventKindContent {
    /// The kind discriminant.
    pub fn kind(&self) -> EventKind {
        match self {
            Self::RoomCreated { .. } => EventKind::RoomCreated,
            Self::Message { .. } => EventKind::Message,
            Self::AgentStatus { .. } => EventKind::AgentStatus,
            Self::MemberJoined { .. } => EventKind::MemberJoined,
            Self::MemberLeft { .. } => EventKind::MemberLeft,
            Self::MemberRemoved { .. } => EventKind::MemberRemoved,
            Self::InviteRevoked { .. } => EventKind::InviteRevoked,
            Self::FileShared { .. } => EventKind::FileShared,
            Self::PipePublished { .. } => EventKind::PipePublished,
            Self::PipeRevoked { .. } => EventKind::PipeRevoked,
        }
    }
}
