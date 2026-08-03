//! Shared value types from the record's wire-conventions table. Every
//! operation draws from these; a type defined here is never redefined in an
//! operation. Closed vocabularies are exhaustive enums with **no** unknown
//! arm — deserialization of an out-of-vocabulary value fails, because
//! protocol v2 never silently reclassifies an unrecognized wire value.

use crate::ids::{DeviceId, SubjectId};
use crate::types::EventKind;
use serde::{Deserialize, Serialize};

/// `<ts>` — an RFC 3339 UTC instant with a `Z` offset. Signed event
/// timestamps are non-repudiable author-dated facts, never the wall clock.
///
/// The wire form is the RFC 3339 string, not time's component sequence, and
/// the offset is always `Z`. Serde uses `time::serde::rfc3339` so a
/// non-`Z` offset fails deserialization rather than being silently
/// re-zoned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(time::OffsetDateTime);

impl Timestamp {
    /// Wraps an instant.
    pub fn new(t: time::OffsetDateTime) -> Self {
        Self(t)
    }
    /// The wrapped instant.
    pub fn into_inner(self) -> time::OffsetDateTime {
        self.0
    }
}

impl From<time::OffsetDateTime> for Timestamp {
    fn from(t: time::OffsetDateTime) -> Self {
        Self(t)
    }
}

impl serde::Serialize for Timestamp {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        time::serde::rfc3339::serialize(&self.0, s)
    }
}

impl<'de> serde::Deserialize<'de> for Timestamp {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let t = time::serde::rfc3339::deserialize(d)?;
        // The v2 `<ts>` wire form requires a `Z` offset, not a numeric one —
        // an instant carrying `+01:00` is not the wire shape, so it is
        // refused at deserialization rather than silently re-zoned.
        if t.offset() != time::UtcOffset::UTC {
            return Err(serde::de::Error::custom(
                "timestamp must carry a `Z` (UTC) offset",
            ));
        }
        Ok(Self(t))
    }
}

// ---------------------------------------------------------------------------
// Bare enums (closed, no payload on any arm)
// ---------------------------------------------------------------------------

/// `role` — a member's capacity. Closed on exactly two tokens; `agent` is
/// not a role (agent-ness is a derived classification), and `owner` is v1's
/// removed spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The room's authority (today: its creator).
    Authority,
    /// An ordinary member.
    Member,
}

/// `standing` — the only vocabulary for a subject's relationship to a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    /// A current member.
    Active,
    /// Voluntarily departed.
    Left,
    /// Removed by an authority.
    Removed,
}

/// `severity` — derived from the closed label vocabulary, served on
/// projections, **never sent** and never written into signed content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Nominal.
    Ok,
    /// Task failed.
    Failed,
    /// Stopped and needs a person.
    Review,
}

/// `liveness` — an agent's derived liveness, computed at read time, never
/// stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Liveness {
    /// Reachable and not executing.
    OnlineIdle,
    /// Executing.
    Working,
    /// Not reachable.
    Offline,
    /// Evidence too old to vouch for.
    Stale,
}

/// `reachability` — one room's transport answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reachability {
    /// Bringing transports up.
    Connecting,
    /// At least one peer link exists.
    Connected,
    /// Live with no peers.
    Alone,
    /// Not live.
    Offline,
}

/// The reason a per-device link is absent (`link.reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkReason {
    /// No dial was ever attempted.
    NeverDialed,
    /// A dial was attempted and failed.
    DialFailed,
    /// No route to the peer.
    NoRoute,
    /// The link was up and closed.
    Closed,
}

/// The closed agent-status label vocabulary. Any other label is
/// `status_label_unknown` — v2 does not silently reclassify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusLabel {
    /// Announced, not executing.
    Online,
    /// Not executing, ready.
    Idle,
    /// In claim arbitration — deliberately not `working`.
    Claiming,
    /// Executing.
    Working,
    /// Task succeeded.
    Done,
    /// Task failed.
    Failed,
    /// Stopped and needs a person.
    Blocked,
}

impl StatusLabel {
    /// The severity this label derives. A lookup, never an inference, and
    /// served so no client re-derives it.
    pub fn severity(self) -> Severity {
        match self {
            Self::Online | Self::Idle | Self::Claiming | Self::Working | Self::Done => Severity::Ok,
            Self::Failed => Severity::Failed,
            Self::Blocked => Severity::Review,
        }
    }
}

/// `direction` for the six paging operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Newer first-encountered order.
    Forward,
    /// Older first-encountered order.
    Backward,
}

/// `redeemability` — one vocabulary answering "can this be redeemed, and if
/// not why not" in both `invite.mint` and `invite.list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Redeemability {
    /// Redeemable now.
    Outstanding,
    /// Past its absolute expiry.
    Expired,
    /// Withdrawn by the authority.
    Revoked,
    /// Already converted into membership.
    Redeemed,
}

/// `gap.reason` — why a position discontinuity exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapReason {
    /// The consumer fell behind the writer.
    Backpressure,
    /// Events were pruned below the requested position.
    Retention,
    /// Delivery lapsed with the subscription.
    SubscriptionLapse,
}

// ---------------------------------------------------------------------------
// Tagged variants (closed, at least one arm carries a payload)
// ---------------------------------------------------------------------------

/// `link` — one device's transport answer. Every per-device answer uses
/// this type (`room.peers`, `file.list` provider rows, `pipe.list`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Link {
    /// A direct path, since the given instant.
    Direct {
        /// When the link came up.
        since: Timestamp,
    },
    /// A relayed path, since the given instant.
    Relay {
        /// When the link came up.
        since: Timestamp,
    },
    /// Not connected, with the reason stated.
    NotConnected {
        /// Why there is no link.
        reason: LinkReason,
    },
}

/// `cursor` — a paging cursor. `Start` names no position because the caller
/// does not yet know one; it is the caller's explicit choice, not a daemon
/// default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Cursor {
    /// From the beginning.
    Start,
    /// From a concrete position.
    At {
        /// The position.
        pos: u64,
    },
}

/// `truncated` — the one continuation mechanism for a page. `More` carries
/// the cursor to resend; `Complete` means there is nothing further and no
/// cursor exists to misuse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Truncated {
    /// No further events.
    Complete,
    /// More events exist; resend this cursor.
    More {
        /// The continuation cursor.
        cursor: Cursor,
    },
}

/// `progress` on a status post. The `reported` arm's `percent` is bounded
/// to `0..=100` at deserialization: the record fixes the inclusive range
/// and requires anything outside it to be `invalid_argument`, so the bound
/// is structural, not a later check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Progress {
    /// No progress reported.
    Absent,
    /// Progress reported as a percentage in `0..=100`.
    Reported {
        /// The percentage, inclusive `0..=100`.
        percent: u8,
    },
}

impl<'de> Deserialize<'de> for Progress {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
        enum Raw {
            Absent,
            Reported { percent: u8 },
        }
        match Raw::deserialize(d)? {
            Raw::Absent => Ok(Progress::Absent),
            Raw::Reported { percent } if percent <= 100 => Ok(Progress::Reported { percent }),
            Raw::Reported { percent } => Err(serde::de::Error::custom(format!(
                "progress.percent {percent} is outside 0..=100"
            ))),
        }
    }
}

/// `author` on a committed event — a variant because the record removes the
/// fabricated default role. An author the membership fold cannot resolve
/// carries no attribution at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Author {
    /// Attribution resolved to a member.
    Resolved {
        /// The author's subject.
        subject_id: SubjectId,
        /// The author's role at the time of the event.
        role: Role,
        /// The author's standing at the time of the event.
        standing: Standing,
    },
    /// The author could not be resolved; nothing is asserted.
    Unresolved,
}

/// `subject` on `hello` — the local subject, or its stated absence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SubjectState {
    /// A subject exists.
    Present {
        /// Its subject id.
        subject_id: SubjectId,
        /// Its device id.
        device_id: DeviceId,
    },
    /// No subject exists yet.
    Absent,
}

/// `resume` on `hello` — whether this connection continues a prior stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Resume {
    /// A fresh stream.
    Fresh,
    /// A resumed stream, from a position.
    Resumed {
        /// The position the stream resumes from.
        from_pos: u64,
    },
}

/// `gap.to` — the upper end of a position gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GapTo {
    /// The gap is bounded.
    Bounded {
        /// The position the gap ends at.
        pos: u64,
    },
    /// The gap is open — everything past `from_pos` is suspect.
    Open,
}

/// `target` — a pipe's publish target, one object, never two sibling fields.
///
/// `port` is a `u64`, not a `u16`: the record requires every port outside
/// `1..=65535` to produce `pipe_target_refused` carrying the rejected target
/// verbatim, and a narrower type would reclassify that input as a structural
/// decode error before the policy could see it. The bound is enforced at
/// semantic validation (step 7), never by the type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// The host (loopback only for publish).
    pub host: String,
    /// The port, validated to `1..=65535` at semantic validation.
    pub port: u64,
}

/// `audience` — who may connect to a pipe. Required; `room` must be said
/// out loud rather than fallen into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Audience {
    /// Any member of the room.
    Room,
    /// Named subjects only.
    Subjects {
        /// The subjects.
        subject_ids: Vec<SubjectId>,
    },
}

/// `last_event` on a room row — a variant because a room with no events
/// must be able to say so without a null or a sentinel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LastEvent {
    /// The newest event's author-dated instant and kind.
    Present {
        /// When the author dated it.
        at: Timestamp,
        /// Its kind.
        kind: EventKind,
    },
    /// The room has no events.
    Absent,
}

/// `last_seen` on a fleet row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LastSeen {
    /// The agent was last observed at this instant.
    Present {
        /// The instant.
        at: Timestamp,
    },
    /// Never observed.
    Absent,
}

/// `latest_status` on a fleet row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LatestStatus {
    /// The agent's newest posted status.
    Present {
        /// Its label.
        label: StatusLabel,
        /// When it was posted.
        at: Timestamp,
    },
    /// The agent has posted nothing.
    Absent,
}

/// `byte_total` — a transfer's total size. The `Unknown` arm exists because
/// a provider that never declared a size genuinely leaves it unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ByteTotal {
    /// The total is known.
    Known {
        /// The total bytes.
        bytes: u64,
    },
    /// The total is genuinely unknown.
    Unknown,
}

/// `stream_abort_reason` — why an admitted transfer ended without a more
/// specific operation error.
///
/// This JSON-facing vocabulary deliberately excludes the Binary ABORT value
/// `operation_error` and includes locally synthesized `transport_lost`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamAbortReason {
    /// The transfer was cancelled.
    Cancelled,
    /// The source could not produce bytes.
    SourceFailed,
    /// The sink could not accept bytes.
    SinkFailed,
    /// The transport ended before the stream could terminate.
    TransportLost,
    /// A protocol violation ended the stream.
    ProtocolError,
}

/// `outcome` of `transfer.cancel` — closed on the two terminal results, so
/// idempotence is observable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CancelOutcome {
    /// This call cancelled the transfer.
    Cancelled,
    /// The transfer was already cancelled.
    AlreadyCancelled,
}
