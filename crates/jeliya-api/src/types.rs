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
/// kind, content, and attribution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Per-room monotonic position — the same space the push stream uses.
    pub pos: u64,
    /// The event's id.
    pub event_id: EventId,
    /// When the author dated it (non-repudiable, signed).
    pub at: crate::Timestamp,
    /// Its kind.
    pub kind: EventKind,
    /// Its content, fixed by the kind.
    pub content: EventContent,
    /// Its attribution, or the explicit unresolved variant.
    pub author: Author,
}

/// Event content, fixed by [`EventKind`]. Carries no `severity` anywhere —
/// severity is derived and served on projections, never written into signed
/// content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventContent {
    /// `room_created { name }`
    RoomCreated {
        /// The room's name.
        name: String,
    },
    /// `message { body }`
    Message {
        /// The message body.
        body: String,
    },
    /// `agent_status { label, progress }`
    AgentStatus {
        /// The closed-vocabulary label.
        label: crate::StatusLabel,
        /// Its progress variant.
        progress: Progress,
    },
    /// `member_joined { subject_id, role }`
    MemberJoined {
        /// Who joined.
        subject_id: SubjectId,
        /// The role they joined with.
        role: Role,
    },
    /// `member_left { subject_id }`
    MemberLeft {
        /// Who left.
        subject_id: SubjectId,
    },
    /// `member_removed { subject_id, by }`
    MemberRemoved {
        /// Who was removed.
        subject_id: SubjectId,
        /// Who removed them.
        by: SubjectId,
    },
    /// `invite_revoked { invite_id }`
    InviteRevoked {
        /// The revoked invite.
        invite_id: InviteId,
    },
    /// `file_shared { file_id, name, bytes, digest }`
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
    /// `pipe_published { pipe_id, target, audience }`
    PipePublished {
        /// The pipe's id.
        pipe_id: PipeId,
        /// Its publish target.
        target: Target,
        /// Its audience.
        audience: Audience,
    },
    /// `pipe_revoked { pipe_id }`
    PipeRevoked {
        /// The revoked pipe.
        pipe_id: PipeId,
    },
}
