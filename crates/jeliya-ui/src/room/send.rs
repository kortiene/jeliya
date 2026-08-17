//! The evidence-backed send state machine (#179 §6, D3/D4).
//!
//! A send is exactly what the seam proves and nothing more (D3): a
//! [`SendEntry`] moves `Pending` → `Syncing` (the daemon authored the event)
//! → **dropped** when the reconciler's converged timeline surfaces the
//! committed row; or `Failed`, sub-classified by
//! [`CallError::execution`](jeliya_client::CallError::execution) into
//! never-sent ([`Execution::DefinitelyNot`]) versus may-have-executed
//! ([`Execution::Unknown`]). A `DecodeReply` failure ([`Execution::Definitely`])
//! is treated as `Syncing` — the daemon ran it, so a committed row will arrive.
//!
//! Retry is idempotent by construction (D4): each entry mints one stable
//! [`OpId`] and reuses it on every retry, so `message.send` under the same
//! `op_id` returns the daemon ledger's original `event_id` and performs no
//! second effect on the same connection. The client **never** auto-replays a
//! may-have-executed send; only the user's Retry re-issues.
//!
//! This module is pure (`jeliya-api` + `jeliya-client` seam types + std): no
//! Dioxus, no `web-sys`, no `cfg`. It holds the decisions; the composer
//! component ([`crate::components::composer`]) performs the dispatch.

use std::sync::atomic::{AtomicU64, Ordering};

use jeliya_api::{EventId, MessageSendOut, OpId, Timestamp};
use jeliya_client::{CallError, Execution};

/// A local, per-room-session identifier for one send attempt. Distinct from the
/// daemon's [`EventId`]: it names the *local* entry before (and independently
/// of) any committed row, so a retry can address the same pending entry.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct SendId(pub u64);

/// A monotonic source of [`SendId`]s, per room-session. Pure and deterministic
/// (no clock, no RNG): the nth minted id is `SendId(n)`, so a test can assert
/// exact ids and the derived [`OpId`] is reproducible.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SendIds {
    next: u64,
}

impl SendIds {
    /// A fresh source starting at `SendId(0)`.
    pub fn new() -> Self {
        Self { next: 0 }
    }

    /// Mint the next id.
    pub fn mint(&mut self) -> SendId {
        let id = SendId(self.next);
        self.next = self.next.saturating_add(1);
        id
    }
}

/// The Unix-epoch [`Timestamp`], the wasm-safe fallback ordering instant for a
/// local send when the room has no signed events yet (a renderer-agnostic
/// component has no wall clock — D6, §6). `UNIX_EPOCH` is a `const`, so this
/// works on the browser without the `time` clock feature; the value is only an
/// ordering key (pending rows render no timestamp), never shown as a real time.
pub fn epoch_timestamp() -> Timestamp {
    Timestamp::new(time::OffsetDateTime::UNIX_EPOCH)
}

/// The process-local sequence backing [`next_send_namespace`]. No clock, no
/// RNG: deterministic within a run (tests assert structure, never absolute
/// values).
static SEND_NAMESPACE_SEQ: AtomicU64 = AtomicU64::new(0);

/// A session-unique op-id namespace for ONE pending-send set (§6/D4). The
/// daemon's dedup ledger is keyed by `(principal, op_id)` — never by room or
/// component lifetime — so a per-pane counter alone restarts at `dx-send-0`
/// on every mount and collides with earlier sends, producing `op_id_conflict`
/// instead of authoring the message. Each fresh set mints a unique namespace;
/// within a set the per-entry counter stays stable across retries.
pub fn next_send_namespace() -> String {
    format!(
        "dx-send-{}",
        SEND_NAMESPACE_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// The stable envelope `op_id` a send (and every retry of it) carries. Derived
/// from the send set's session-unique `namespace` plus the local [`SendId`],
/// so it is stable across retries of the same entry (D4) and unique per entry
/// across pane lifetimes. The value is a dedup key, never a user-visible
/// identifier.
pub fn op_id_for(namespace: &str, client_id: SendId) -> OpId {
    OpId::new(format!("{namespace}-{}", client_id.0))
}

/// A local send's lifecycle phase — exactly what the seam has proven (D3).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SendPhase {
    /// The `message.send` future is in flight; nothing is proven yet.
    Pending,
    /// The daemon authored the event. `event_id` is `Some` once the
    /// `MessageSendOut` returned; `None` for a `DecodeReply` failure
    /// ([`Execution::Definitely`]) where the daemon ran it but the local reply
    /// was unreadable, so the id is unknown but a committed row will still
    /// arrive (reconciled by absence, §10). Never a delivery receipt — the
    /// honest "syncing…" state, awaiting the committed row.
    Syncing {
        /// The authored event id, when the reply was readable.
        event_id: Option<EventId>,
    },
    /// The call failed. `execution` distinguishes never-sent
    /// ([`Execution::DefinitelyNot`], clean retry) from may-have-executed
    /// ([`Execution::Unknown`], retry offered but the ambiguity stated and never
    /// auto-taken). `code` is the daemon's protocol code discriminant when the
    /// failure was a typed [`CallError::Wire`] refusal — the discriminant only,
    /// never a payload-bearing identifier (§11).
    Failed {
        /// Whether the failed send may have executed.
        execution: Execution,
        /// The wire protocol code, for a typed daemon refusal.
        code: Option<String>,
    },
}

/// One local send not yet reconciled against a committed row. Always shown in
/// the timeline (never filtered), so Retry stays reachable (§5.1).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SendEntry {
    /// The local, per-room-session id.
    pub client_id: SendId,
    /// The stable dedup key reused on every retry (D4).
    pub op_id: OpId,
    /// The composed body.
    pub body: String,
    /// The author-dated instant this send was composed at (local; not a signed
    /// timestamp until the committed row arrives).
    pub at: Timestamp,
    /// The current lifecycle phase.
    pub phase: SendPhase,
}

impl SendEntry {
    /// A fresh send in [`SendPhase::Pending`], with its stable op_id derived
    /// from the send set's `namespace` and `client_id`.
    pub fn new(namespace: &str, client_id: SendId, body: String, at: Timestamp) -> Self {
        Self {
            client_id,
            op_id: op_id_for(namespace, client_id),
            body,
            at,
            phase: SendPhase::Pending,
        }
    }

    /// The reconciler surfaced this send's committed row: an entry in
    /// [`SendPhase::Syncing`] with a known `event_id` is dropped iff `timeline`
    /// contains that id.
    pub fn reconciled_by(&self, timeline_has: &impl Fn(&EventId) -> bool) -> bool {
        matches!(
            &self.phase,
            SendPhase::Syncing { event_id: Some(id) } if timeline_has(id)
        )
    }
}

/// The daemon authored the event: transition a `Pending` send to
/// [`SendPhase::Syncing`] with the returned `event_id` (D3, step 2). This is the
/// only place a known `event_id` enters a send's phase.
pub fn phase_for_reply(out: &MessageSendOut) -> SendPhase {
    SendPhase::Syncing {
        event_id: Some(out.event_id.clone()),
    }
}

/// Classify a failed [`CallError`] into the honest phase (D3, step 3):
///
/// - [`Execution::Definitely`] (a `DecodeReply` local failure) → `Syncing` with
///   an unknown `event_id`: the daemon ran it, so a committed row will arrive
///   and the entry reconciles by absence (§10 fallback). It is **not** a
///   failure the user must retry.
/// - [`Execution::DefinitelyNot`] → `Failed{DefinitelyNot}` ("not sent", clean
///   retry).
/// - [`Execution::Unknown`] → `Failed{Unknown}` ("may not have sent", retry
///   offered with the ambiguity stated, never auto-taken).
///
/// A typed [`CallError::Wire`] refusal carries its protocol `code` discriminant
/// (never a payload, §11) so the composer can translate it.
pub fn classify_error(error: &CallError) -> SendPhase {
    match error.execution() {
        Execution::Definitely => SendPhase::Syncing { event_id: None },
        execution @ (Execution::DefinitelyNot | Execution::Unknown) => SendPhase::Failed {
            execution,
            code: wire_code(error),
        },
    }
}

/// The protocol `code` discriminant of a typed daemon refusal, or `None` for a
/// transport/local failure. Derived from the seam's [`CallError::as_wire`] so
/// the code is the enum variant name — no string parsing, no payload exposure
/// (§K15).
fn wire_code(error: &CallError) -> Option<String> {
    error.as_wire().map(|api| api.wire_code().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::{ApiError, OpId, RoomId};

    fn ts(secs: i64) -> Timestamp {
        Timestamp::new(time::OffsetDateTime::from_unix_timestamp(secs).expect("valid instant"))
    }

    fn send_out(event_id: &str) -> MessageSendOut {
        MessageSendOut {
            room_id: RoomId::new("r"),
            event_id: EventId::new(event_id),
            pos: 7,
            at: ts(100),
        }
    }

    #[test]
    fn send_ids_are_monotonic_and_op_id_is_stable() {
        let mut ids = SendIds::new();
        let a = ids.mint();
        let b = ids.mint();
        assert_eq!(a, SendId(0));
        assert_eq!(b, SendId(1));
        // The op_id is derived from the namespace + client id, so it is stable
        // across retries of the SAME entry and distinct between entries (D4).
        assert_eq!(op_id_for("dx-send-t", a), OpId::new("dx-send-t-0"));
        assert_eq!(op_id_for("dx-send-t", a), op_id_for("dx-send-t", a));
        assert_ne!(op_id_for("dx-send-t", a), op_id_for("dx-send-t", b));
    }

    #[test]
    fn namespaces_make_op_ids_unique_across_send_sets() {
        // Two sets (two pane mounts) must never mint the same op_id: the
        // daemon dedup ledger is keyed by (principal, op_id), so a collision
        // answers op_id_conflict instead of authoring the message.
        let ns_a = next_send_namespace();
        let ns_b = next_send_namespace();
        assert_ne!(ns_a, ns_b);
        assert_ne!(
            op_id_for(&ns_a, SendId(0)),
            op_id_for(&ns_b, SendId(0)),
            "fresh mounts must not collide on dx-send-…-0"
        );
    }

    #[test]
    fn a_reply_moves_pending_to_syncing_with_the_event_id() {
        let mut ids = SendIds::new();
        let entry = SendEntry::new("dx-send-t", ids.mint(), "hi".into(), ts(1));
        assert_eq!(entry.phase, SendPhase::Pending);
        let phase = phase_for_reply(&send_out("event-1"));
        assert_eq!(
            phase,
            SendPhase::Syncing {
                event_id: Some(EventId::new("event-1"))
            }
        );
    }

    #[test]
    fn classify_maps_execution_to_the_honest_phase() {
        // DefinitelyNot: a typed refusal, a full queue, an encode failure.
        assert_eq!(
            classify_error(&CallError::QueueFull {
                resource: "outbound",
                limit: 8,
            }),
            SendPhase::Failed {
                execution: Execution::DefinitelyNot,
                code: None,
            }
        );
        // A typed wire refusal carries its code discriminant, not its payload.
        let wire = classify_error(&CallError::Wire(ApiError::OpIdConflict {
            op_id: OpId::new("secret-key"),
        }));
        match wire {
            SendPhase::Failed {
                execution: Execution::DefinitelyNot,
                code: Some(code),
            } => {
                assert_eq!(code, "op_id_conflict");
                assert!(!code.contains("secret-key"));
            }
            other => panic!("expected DefinitelyNot wire failure, got {other:?}"),
        }
        // Unknown: a timeout, a disconnect after bytes went out.
        assert_eq!(
            classify_error(&CallError::Timeout),
            SendPhase::Failed {
                execution: Execution::Unknown,
                code: None,
            }
        );
        assert_eq!(
            classify_error(&CallError::Disconnected {
                execution: Execution::Unknown,
            }),
            SendPhase::Failed {
                execution: Execution::Unknown,
                code: None,
            }
        );
        // Definitely (DecodeReply): the daemon ran it — treated as Syncing, not
        // a failure the user must retry.
        assert_eq!(
            classify_error(&CallError::Local(jeliya_client::LocalError::DecodeReply)),
            SendPhase::Syncing { event_id: None }
        );
    }

    #[test]
    fn a_never_sent_disconnect_is_a_clean_failure() {
        assert_eq!(
            classify_error(&CallError::Disconnected {
                execution: Execution::DefinitelyNot,
            }),
            SendPhase::Failed {
                execution: Execution::DefinitelyNot,
                code: None,
            }
        );
    }

    #[test]
    fn a_syncing_entry_reconciles_only_on_its_own_event_id() {
        let mut ids = SendIds::new();
        let mut entry = SendEntry::new("dx-send-t", ids.mint(), "hi".into(), ts(1));
        entry.phase = phase_for_reply(&send_out("event-1"));
        let has_other = |id: &EventId| id.as_str() == "event-2";
        let has_own = |id: &EventId| id.as_str() == "event-1";
        assert!(!entry.reconciled_by(&has_other));
        assert!(entry.reconciled_by(&has_own));

        // A still-Pending entry never reconciles (no event_id yet).
        let pending = SendEntry::new("dx-send-t", ids.mint(), "hi".into(), ts(1));
        assert!(!pending.reconciled_by(&has_own));

        // A Syncing entry with an UNKNOWN event_id (DecodeReply) reconciles by
        // absence, never by a timeline id match.
        let mut decoded = SendEntry::new("dx-send-t", ids.mint(), "hi".into(), ts(1));
        decoded.phase = SendPhase::Syncing { event_id: None };
        assert!(!decoded.reconciled_by(&has_own));
    }

    #[test]
    fn epoch_timestamp_is_unix_epoch_and_precedes_any_real_signed_timestamp() {
        // `epoch_timestamp()` returns `UNIX_EPOCH` as a wasm-safe ordering key.
        // Any event authored in the room will have a later `at`; the epoch value
        // must therefore sort before it so pending rows without a real clock
        // do not displace committed rows.
        let epoch = epoch_timestamp();
        let later = ts(1);
        assert!(
            epoch < later,
            "UNIX_EPOCH sorts before any positive-second timestamp"
        );
        // The epoch is the same every call — a const, not an RNG or clock.
        assert_eq!(epoch, epoch_timestamp());
    }
}
