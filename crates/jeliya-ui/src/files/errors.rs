//! Typed, redacting classification of Files/Pipes flow failures (spec §6).
//!
//! Every daemon failure is a typed `ApiError`; this module maps the
//! **discriminant** (plus the structured integers the taxonomy fixes) to a
//! closed [`FlowFailure`], **never** an English substring match (retiring the
//! v1 `share limit` / `HashMismatch` substring hacks). A [`FlowFailure`] carries
//! only bytes/limits — never a file's bytes, a name, a path, a full digest, or a
//! token (spec D7) — so it is safe to localize and safe to log.
//!
//! Cancellation is kept **out** of [`FlowFailure`]: a cancelled flow settles as
//! [`Settled::Cancelled`], never as a failure the user caused and never as `Ok`
//! (spec D5). The three "did not happen" platform reasons — `Unavailable`,
//! `Denied`, `Cancelled` — stay distinct, mirroring the platform boundary.

use jeliya_api::ApiError;
use jeliya_client::CallError;
use jeliya_platform::{CapabilityError, FailureKind};

/// A closed, redacting classification of why a flow did not succeed. The
/// payloads are byte counts only — safe to render, per §K1/D7.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FlowFailure {
    /// Over the served `max_shared_file_bytes` (spec D3). Carries the served
    /// integers so the surface can *explain* the limit (never a baked number).
    TooLarge {
        /// The offending size.
        size: u64,
        /// The served ceiling.
        limit: u64,
    },
    /// A share's streamed bytes disagreed with its declared size — a *size*
    /// disagreement, never rendered as corruption (spec D3).
    SizeMismatch {
        /// The declared size.
        declared: u64,
        /// The observed size.
        observed: u64,
    },
    /// The picked file was empty; nothing to share.
    Empty,
    /// No provider holding the file could be reached (spec D2/#94). The attempted
    /// providers' evidence is rendered from the `file.list` model, not carried in
    /// the error string.
    NoReachableProvider,
    /// Content did not verify — a genuine integrity failure. **Never** used for a
    /// size refusal (spec D3).
    DigestMismatch,
    /// No such file in this room.
    FileUnknown,
    /// The file's bytes are not held locally — fetch first.
    NotFetched,
    /// No forward progress within the stall window.
    Stalled,
    /// The size-aware absolute transfer budget expired.
    DeadlineExceeded,
    /// An admitted transfer ended without a more specific operation error.
    Aborted,
    /// The publish target is not allowed (loopback policy).
    PipeTargetRefused,
    /// Publishing is not permitted in this room (distinct from a bad target).
    PolicyRefused,
    /// The publisher's device could not be reached — the **distinctive** offline
    /// -publisher failure (#94), distinct from [`FlowFailure::PipeUnknown`].
    PipeUnreachable,
    /// No such pipe, or the caller is outside its audience (deliberately one
    /// answer — not an oracle).
    PipeUnknown,
    /// The pipe was withdrawn.
    PipeRevoked,
    /// `pipe.revoke` named a pipe this subject did not publish.
    PipeNotPublisher,
    /// No such local connection.
    ConnectionUnknown,
    /// The operation requires a live room — activate first.
    RoomNotLive,
    /// The room's file/pipe index cannot be read.
    IndexUnreadable,
    /// The capability is not available in this browser (structural).
    Unavailable,
    /// Permission was refused by the user or platform.
    Denied,
    /// The source/destination could not be read or written locally.
    Unreadable,
    /// A typed daemon refusal with no more specific mapping.
    Wire,
    /// The round trip could not complete (queue full / timeout / disconnect /
    /// local encode/decode) — not a daemon verdict.
    Transport,
}

/// A flow's non-success outcome: a typed failure, or an honest cancellation
/// (which is *never* a failure — spec D5).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FlowStop {
    /// A typed failure to render.
    Failed(FlowFailure),
    /// The action did nothing; the prior state is untouched.
    Cancelled,
}

/// The settled outcome of a flow: exactly one of success, a typed failure, or a
/// cancellation. `Cancelled` is never `Ok` and never a `Failed` (spec D5).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Settled<T> {
    /// The flow succeeded with its paired payload.
    Ok(T),
    /// The flow failed with a typed, redacting reason.
    Failed(FlowFailure),
    /// The flow was cancelled — the prior state is untouched.
    Cancelled,
}

impl<T> Settled<T> {
    /// Lift a non-success stop into a settled outcome.
    pub fn from_stop(stop: FlowStop) -> Self {
        match stop {
            FlowStop::Failed(failure) => Settled::Failed(failure),
            FlowStop::Cancelled => Settled::Cancelled,
        }
    }

    /// Whether this outcome is a cancellation (the safety-critical branch: keep
    /// the prior state, render nothing as an error).
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Settled::Cancelled)
    }
}

impl FlowStop {
    /// Classify a client [`CallError`]: a delivery-classified cancellation is
    /// [`FlowStop::Cancelled`]; every other outcome is a typed
    /// [`FlowFailure`]. Reads the wire discriminant, never a substring (§6).
    pub fn from_call(error: &CallError) -> Self {
        match error {
            CallError::Cancelled { .. } => FlowStop::Cancelled,
            other => FlowStop::Failed(classify_call(other)),
        }
    }

    /// Classify a platform [`CapabilityError`]: `Cancelled` is
    /// [`FlowStop::Cancelled`]; everything else is a typed [`FlowFailure`].
    pub fn from_capability(error: CapabilityError) -> Self {
        if error.is_cancelled() {
            FlowStop::Cancelled
        } else {
            FlowStop::Failed(classify_capability(error))
        }
    }
}

/// Map a client [`CallError`] (already known not to be a cancellation) to a
/// [`FlowFailure`]. A wire verdict maps by discriminant; a transport outcome is
/// [`FlowFailure::Transport`].
fn classify_call(error: &CallError) -> FlowFailure {
    match error.as_wire() {
        Some(api) => classify_wire(api),
        None => FlowFailure::Transport,
    }
}

/// Map a typed daemon [`ApiError`] to a [`FlowFailure`] by discriminant only.
fn classify_wire(api: &ApiError) -> FlowFailure {
    match api {
        ApiError::FileTooLarge {
            declared_bytes,
            limit_bytes,
            ..
        } => FlowFailure::TooLarge {
            size: *declared_bytes,
            limit: *limit_bytes,
        },
        ApiError::DeclaredSizeMismatch {
            declared_bytes,
            observed_bytes,
        } => FlowFailure::SizeMismatch {
            declared: *declared_bytes,
            observed: *observed_bytes,
        },
        ApiError::ProviderUnreachable { .. } => FlowFailure::NoReachableProvider,
        ApiError::DigestMismatch { .. } => FlowFailure::DigestMismatch,
        ApiError::FileUnknown { .. } => FlowFailure::FileUnknown,
        ApiError::FileNotFetched { .. } => FlowFailure::NotFetched,
        ApiError::TransferStalled { .. } => FlowFailure::Stalled,
        ApiError::TransferDeadlineExceeded { .. } => FlowFailure::DeadlineExceeded,
        ApiError::StreamAborted { .. } => FlowFailure::Aborted,
        ApiError::TransferUnknown { .. } => FlowFailure::Transport,
        ApiError::FileIndexUnreadable | ApiError::PipeIndexUnreadable => {
            FlowFailure::IndexUnreadable
        }
        ApiError::PipeTargetRefused { .. } => FlowFailure::PipeTargetRefused,
        ApiError::PolicyRefused { .. } => FlowFailure::PolicyRefused,
        ApiError::PipeUnreachable { .. } => FlowFailure::PipeUnreachable,
        ApiError::PipeUnknown { .. } => FlowFailure::PipeUnknown,
        ApiError::PipeRevoked { .. } => FlowFailure::PipeRevoked,
        ApiError::PipeNotPublisher { .. } => FlowFailure::PipeNotPublisher,
        ApiError::ConnectionUnknown { .. } => FlowFailure::ConnectionUnknown,
        ApiError::RoomNotLive { .. } => FlowFailure::RoomNotLive,
        _ => FlowFailure::Wire,
    }
}

/// Map a platform [`CapabilityError`] (already known not to be a cancellation)
/// to a [`FlowFailure`].
fn classify_capability(error: CapabilityError) -> FlowFailure {
    match error {
        CapabilityError::Unavailable => FlowFailure::Unavailable,
        CapabilityError::Denied => FlowFailure::Denied,
        // Handled by the caller before reaching here, but map defensively.
        CapabilityError::Cancelled => FlowFailure::Transport,
        CapabilityError::Failed(kind) => match kind {
            FailureKind::FileTooLarge { size, limit } => FlowFailure::TooLarge { size, limit },
            FailureKind::FileEmpty => FlowFailure::Empty,
            FailureKind::Unreadable | FailureKind::Io => FlowFailure::Unreadable,
            FailureKind::WriteNotDurable => FlowFailure::Transport,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::{
        ByteTotal, EnforcedAt, FileId, OpId, PipeId, RoomId, StreamAbortReason, Target, Timestamp,
    };
    use jeliya_client::Execution;

    #[test]
    fn a_wire_size_refusal_carries_the_served_integers_and_is_not_digest() {
        let stop = FlowStop::from_call(&CallError::Wire(ApiError::FileTooLarge {
            declared_bytes: 200,
            limit_bytes: 100,
            enforced_at: EnforcedAt::StageDeclared,
        }));
        assert_eq!(
            stop,
            FlowStop::Failed(FlowFailure::TooLarge {
                size: 200,
                limit: 100
            })
        );
        // A digest mismatch is a *different* discriminant — never conflated with
        // a size refusal (spec D3).
        let digest = FlowStop::from_call(&CallError::Wire(ApiError::DigestMismatch {
            expected: "a".into(),
            observed: "b".into(),
        }));
        assert_eq!(digest, FlowStop::Failed(FlowFailure::DigestMismatch));
    }

    #[test]
    fn cancellation_is_never_a_failure() {
        let stop = FlowStop::from_call(&CallError::Cancelled {
            execution: Execution::Unknown,
        });
        assert_eq!(stop, FlowStop::Cancelled);
        assert!(Settled::<()>::from_stop(stop).is_cancelled());
        assert!(FlowStop::from_capability(CapabilityError::Cancelled) == FlowStop::Cancelled);
    }

    #[test]
    fn a_transport_failure_is_not_a_daemon_verdict() {
        assert_eq!(
            FlowStop::from_call(&CallError::Timeout),
            FlowStop::Failed(FlowFailure::Transport)
        );
        assert_eq!(
            FlowStop::from_call(&CallError::Disconnected {
                execution: Execution::Unknown
            }),
            FlowStop::Failed(FlowFailure::Transport)
        );
    }

    #[test]
    fn pipe_unreachable_is_distinct_from_pipe_unknown() {
        let unreachable = FlowStop::from_call(&CallError::Wire(ApiError::PipeUnreachable {
            pipe_id: jeliya_api::PipeId::new("blake3:pipe"),
            link: jeliya_api::Link::NotConnected {
                reason: jeliya_api::LinkReason::NoRoute,
            },
        }));
        let unknown = FlowStop::from_call(&CallError::Wire(ApiError::PipeUnknown {
            pipe_id: jeliya_api::PipeId::new("blake3:pipe"),
        }));
        assert_eq!(unreachable, FlowStop::Failed(FlowFailure::PipeUnreachable));
        assert_eq!(unknown, FlowStop::Failed(FlowFailure::PipeUnknown));
        assert_ne!(unreachable, unknown);
    }

    #[test]
    fn platform_failures_map_to_typed_kinds() {
        assert_eq!(
            FlowStop::from_capability(CapabilityError::Unavailable),
            FlowStop::Failed(FlowFailure::Unavailable)
        );
        assert_eq!(
            FlowStop::from_capability(CapabilityError::Failed(FailureKind::FileEmpty)),
            FlowStop::Failed(FlowFailure::Empty)
        );
        // The client preflight's local too-large maps identically to the daemon's.
        assert_eq!(
            FlowStop::from_capability(CapabilityError::Failed(FailureKind::FileTooLarge {
                size: 5,
                limit: 4
            })),
            FlowStop::Failed(FlowFailure::TooLarge { size: 5, limit: 4 })
        );
        let _ = FileId::new("x");
    }

    #[test]
    fn size_mismatch_is_distinct_from_too_large_and_carries_bytes() {
        // DeclaredSizeMismatch is a stream-integrity disagreement; it MUST NOT
        // be conflated with FileTooLarge (a size policy refusal). The carried
        // integers are declared vs observed, not size vs limit (spec D3).
        let stop = FlowStop::from_call(&CallError::Wire(ApiError::DeclaredSizeMismatch {
            declared_bytes: 100,
            observed_bytes: 120,
        }));
        assert_eq!(
            stop,
            FlowStop::Failed(FlowFailure::SizeMismatch {
                declared: 100,
                observed: 120,
            })
        );
        assert_ne!(
            stop,
            FlowStop::Failed(FlowFailure::TooLarge {
                size: 100,
                limit: 120,
            })
        );
    }

    #[test]
    fn provider_unreachable_maps_to_no_reachable_provider() {
        // ProviderUnreachable carries provider evidence; the FlowFailure
        // carries NONE of it — evidence is read from the file.list model, not
        // the error payload (spec D2 / AC "honest unavailable-provider failure").
        let stop = FlowStop::from_call(&CallError::Wire(ApiError::ProviderUnreachable {
            file_id: FileId::new("blake3:file"),
            providers: vec![],
        }));
        assert_eq!(stop, FlowStop::Failed(FlowFailure::NoReachableProvider));
        assert_ne!(stop, FlowStop::Failed(FlowFailure::DigestMismatch));
    }

    #[test]
    fn both_index_errors_collapse_to_index_unreadable() {
        let file_idx = FlowStop::from_call(&CallError::Wire(ApiError::FileIndexUnreadable));
        let pipe_idx = FlowStop::from_call(&CallError::Wire(ApiError::PipeIndexUnreadable));
        assert_eq!(file_idx, FlowStop::Failed(FlowFailure::IndexUnreadable));
        assert_eq!(pipe_idx, FlowStop::Failed(FlowFailure::IndexUnreadable));
        assert_eq!(file_idx, pipe_idx);
    }

    #[test]
    fn remaining_file_transfer_wire_errors_map_to_their_discriminants() {
        assert_eq!(
            FlowStop::from_call(&CallError::Wire(ApiError::FileUnknown {
                file_id: FileId::new("blake3:f"),
            })),
            FlowStop::Failed(FlowFailure::FileUnknown),
        );
        assert_eq!(
            FlowStop::from_call(&CallError::Wire(ApiError::FileNotFetched {
                file_id: FileId::new("blake3:f"),
            })),
            FlowStop::Failed(FlowFailure::NotFetched),
        );
        assert_eq!(
            FlowStop::from_call(&CallError::Wire(ApiError::TransferStalled {
                transferred_bytes: 0,
                total: ByteTotal::Unknown,
            })),
            FlowStop::Failed(FlowFailure::Stalled),
        );
        assert_eq!(
            FlowStop::from_call(&CallError::Wire(ApiError::TransferDeadlineExceeded {
                transferred_bytes: 0,
                total: ByteTotal::Unknown,
                budget_ms: 5000,
            })),
            FlowStop::Failed(FlowFailure::DeadlineExceeded),
        );
        assert_eq!(
            FlowStop::from_call(&CallError::Wire(ApiError::StreamAborted {
                transferred_bytes: 0,
                total: ByteTotal::Unknown,
                reason: StreamAbortReason::Cancelled,
            })),
            FlowStop::Failed(FlowFailure::Aborted),
        );
        // TransferUnknown maps to Transport (the match arm is explicit, but it
        // is a "no such in-flight transfer" — not a more specific daemon verdict).
        assert_eq!(
            FlowStop::from_call(&CallError::Wire(ApiError::TransferUnknown {
                transfer_op_id: OpId::new("file.fetch:r:f"),
            })),
            FlowStop::Failed(FlowFailure::Transport),
        );
    }

    #[test]
    fn remaining_pipe_wire_errors_map_to_their_discriminants() {
        assert_eq!(
            FlowStop::from_call(&CallError::Wire(ApiError::PipeRevoked {
                pipe_id: PipeId::new("blake3:pipe"),
                revoked_at: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
            })),
            FlowStop::Failed(FlowFailure::PipeRevoked),
        );
        assert_eq!(
            FlowStop::from_call(&CallError::Wire(ApiError::PipeNotPublisher {
                pipe_id: PipeId::new("blake3:pipe"),
            })),
            FlowStop::Failed(FlowFailure::PipeNotPublisher),
        );
        assert_eq!(
            FlowStop::from_call(&CallError::Wire(ApiError::ConnectionUnknown {
                connection_id: "conn-1".into(),
            })),
            FlowStop::Failed(FlowFailure::ConnectionUnknown),
        );
        assert_eq!(
            FlowStop::from_call(&CallError::Wire(ApiError::RoomNotLive {
                room_id: RoomId::new("r"),
            })),
            FlowStop::Failed(FlowFailure::RoomNotLive),
        );
        assert_eq!(
            FlowStop::from_call(&CallError::Wire(ApiError::PolicyRefused {
                room_id: RoomId::new("r"),
            })),
            FlowStop::Failed(FlowFailure::PolicyRefused),
        );
        assert_eq!(
            FlowStop::from_call(&CallError::Wire(ApiError::PipeTargetRefused {
                target: Target {
                    host: "127.0.0.1".into(),
                    port: 8080,
                },
            })),
            FlowStop::Failed(FlowFailure::PipeTargetRefused),
        );
    }

    #[test]
    fn an_unknown_wire_error_maps_to_the_wire_catch_all() {
        // A wire error not in the explicit arms maps to FlowFailure::Wire,
        // never to a typed failure (§6: discriminant-only match, no substring).
        assert_eq!(
            FlowStop::from_call(&CallError::Wire(ApiError::SubscriptionLimitReached {
                limit: 10,
            })),
            FlowStop::Failed(FlowFailure::Wire),
        );
    }

    #[test]
    fn queue_full_is_not_a_daemon_verdict() {
        // QueueFull is client-side backpressure; as_wire() returns None so it
        // lands in FlowFailure::Transport (§6).
        assert_eq!(
            FlowStop::from_call(&CallError::QueueFull {
                resource: "queue_depth",
                limit: 64,
            }),
            FlowStop::Failed(FlowFailure::Transport),
        );
    }

    #[test]
    fn denied_capability_error_is_a_typed_failure() {
        assert_eq!(
            FlowStop::from_capability(CapabilityError::Denied),
            FlowStop::Failed(FlowFailure::Denied),
        );
    }

    #[test]
    fn settled_is_cancelled_only_for_the_cancelled_variant() {
        assert!(Settled::<u32>::Cancelled.is_cancelled());
        assert!(!Settled::Ok(0u32).is_cancelled());
        assert!(!Settled::<u32>::Failed(FlowFailure::Wire).is_cancelled());
    }
}
