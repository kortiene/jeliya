//! The converged, per-room view the UI renders, and the reconciler-owned
//! fan-out that delivers it (#169 §R2, §R6, Open Q2).
//!
//! [`RoomView`] is the reconciler's product: a gap-free, deduplicated,
//! position-ordered timeline plus the wholesale-replaced membership and presence
//! snapshot, stamped with the epoch it was reconciled under. [`RoomUpdate`] is
//! the item a consumer observes — a [`RoomUpdate::Resyncing`] notice at the
//! start of every reconciliation (so *every gap reason is observable*, AC-1) and
//! a [`RoomUpdate::Converged`] view when it completes.
//!
//! Delivery is a **reconciler-owned fan-out** (not [`crate::ClientEvent`]), so
//! the seam's event stream stays the raw wire model and the reconciled view is a
//! distinct, opt-in subscription (Open Q2). Converged views coalesce per room —
//! each view is a full authoritative snapshot, so a stalled consumer keeps only
//! the latest per room rather than an unbounded backlog. Public `Debug` output is
//! metadata-only: event bodies, file names, and digests never pass through the
//! wire model's payload-bearing derived formatter (§R16).

use std::collections::VecDeque;
use std::fmt;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker};

use futures::Stream;
use jeliya_api::{Event, MemberRow, PeerRow, Reachability, RoomId};

use crate::reconcile::buffer::estimated_event_bytes;
use crate::reconcile::reason::ResyncRequired;

/// The converged, per-room view the room UI (#178+) renders.
///
/// The timeline is **gap-free and position-ordered** (dense `pos`, deduplicated
/// by `event_id`), and the membership/presence snapshots are **replaced
/// wholesale** from authoritative reads — never merged, so a missed removal
/// converges (§R8). `generation` is the reconciler-local epoch this view was
/// reconciled under (§R4), so a consumer can discard a view older than one it
/// already holds.
#[derive(Clone, PartialEq)]
pub struct RoomView {
    /// The room this view is for.
    pub room_id: RoomId,
    /// The reconciler-local epoch this view was reconciled under.
    pub generation: u64,
    /// The gap-free, deduplicated, position-ordered timeline.
    pub timeline: Vec<Event>,
    /// The authoritative membership roster (replaced wholesale).
    pub members: Vec<MemberRow>,
    /// The authoritative per-device presence snapshot (replaced wholesale).
    pub peers: Vec<PeerRow>,
    /// The whole-room reachability from the last authoritative `room.peers`.
    pub reachability: Reachability,
}

impl fmt::Debug for RoomView {
    /// Render only bounded aggregate metadata. In particular, do not delegate
    /// to `jeliya_api::Event`'s derived formatter or print opaque event ids:
    /// either may contain sensitive attacker-controlled text (§R16).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let timeline_range = self
            .timeline
            .first()
            .zip(self.timeline.last())
            .map(|(first, last)| (first.pos, last.pos));
        f.debug_struct("RoomView")
            .field("room_id", &self.room_id)
            .field("generation", &self.generation)
            .field("timeline_len", &self.timeline.len())
            .field("timeline_range", &timeline_range)
            .field("members_len", &self.members.len())
            .field("peers_len", &self.peers.len())
            .field("reachability", &self.reachability)
            .finish()
    }
}

/// One item a [`RoomUpdate`] consumer observes.
#[derive(Clone, PartialEq)]
pub enum RoomUpdate {
    /// A reconciliation started for a room; carries the epoch and the observable
    /// cause. Emitted before the baseline read settles, so the cause is visible
    /// before the outcome (AC-1).
    Resyncing {
        /// The room being reconciled.
        room_id: RoomId,
        /// The epoch and cause of the re-baseline.
        resync: ResyncRequired,
    },
    /// A reconciliation converged; carries the authoritative view.
    Converged(RoomView),
    /// Updates were lost either in this subscription mailbox or in a bounded
    /// reconciliation buffer. The marker is delivered at the loss boundary; a
    /// `Converged` immediately after it is the authority that recovered that
    /// loss, otherwise the consumer waits for a subsequent convergence.
    Lagged {
        /// The room, when the dropped updates could be attributed to one room.
        room_id: Option<RoomId>,
        /// The number of updates discarded at this boundary.
        dropped: u64,
    },
}

impl fmt::Debug for RoomUpdate {
    /// Delegate only to the redacted [`RoomView`] formatter and safe control
    /// metadata; never recursively format wire payloads.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RoomUpdate::Resyncing { room_id, resync } => f
                .debug_struct("Resyncing")
                .field("room_id", room_id)
                .field("resync", resync)
                .finish(),
            RoomUpdate::Converged(view) => f.debug_tuple("Converged").field(view).finish(),
            RoomUpdate::Lagged { room_id, dropped } => f
                .debug_struct("Lagged")
                .field("room_id", room_id)
                .field("dropped", dropped)
                .finish(),
        }
    }
}

impl RoomUpdate {
    /// The room this update concerns, when it can be attributed to one room.
    fn room_id(&self) -> Option<&RoomId> {
        match self {
            RoomUpdate::Resyncing { room_id, .. } => Some(room_id),
            RoomUpdate::Converged(view) => Some(&view.room_id),
            RoomUpdate::Lagged { room_id, .. } => room_id.as_ref(),
        }
    }
}

/// The default number of pending [`RoomUpdate`]s one subscription holds before
/// the oldest un-coalesced notice is dropped. Converged views coalesce per room
/// and so do not count against growth; this bounds the `Resyncing` notices a
/// stalled consumer can accumulate.
const DEFAULT_UPDATE_CAPACITY: usize = 256;

/// One queued update and its exact contribution to the ordinary byte budget.
struct QueuedUpdate {
    update: Arc<RoomUpdate>,
    /// Loss markers and the one oversized recovery allowance are zero-charged.
    charged_bytes: u64,
}

/// One subscriber's private mailbox on the [`RoomUpdateBus`].
struct SubscriberState {
    /// Delivered-but-unread updates, in broadcast order. A `Converged` view
    /// replaces any earlier unread `Converged` for the same room (latest wins).
    queue: VecDeque<QueuedUpdate>,
    /// The depth before the oldest notice is dropped.
    capacity: usize,
    /// Estimated bytes retained by ordinary updates (loss markers and one
    /// shared oversized latest-authority allowance sit outside this budget).
    bytes: u64,
    /// Maximum estimated ordinary payload bytes retained by this mailbox.
    bytes_capacity: u64,
    /// The waker to notify when an update (or close) arrives.
    waker: Option<Waker>,
    /// Set once the bus closes; the stream ends after the queue drains.
    closed: bool,
}

/// The reconciler-owned [`RoomUpdate`] fan-out. Every active
/// [`RoomUpdateSubscription`] observes every broadcast update, mirroring the
/// seam's [`crate::EventSubscription`] contract, so two components subscribing
/// cannot starve each other.
pub(crate) struct RoomUpdateBus {
    subscribers: Mutex<Vec<Weak<Mutex<SubscriberState>>>>,
    closed: Mutex<bool>,
    bytes_capacity: u64,
}

impl Drop for RoomUpdateBus {
    fn drop(&mut self) {
        // A subscription may outlive a never-polled reconciler. Last-owner drop
        // must still wake it and make the stream terminal.
        self.close();
    }
}

impl RoomUpdateBus {
    /// A fresh bus with no subscribers.
    pub(crate) fn new() -> Self {
        Self::with_byte_capacity(2 * 1024 * 1024)
    }

    /// A fresh bus whose every subscriber has the supplied byte ceiling.
    pub(crate) fn with_byte_capacity(bytes_capacity: u64) -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
            closed: Mutex::new(false),
            bytes_capacity,
        }
    }

    /// Register an independent subscription. A subscription created after the
    /// bus has closed is born closed and yields `None` immediately.
    pub(crate) fn subscribe(&self) -> RoomUpdateSubscription {
        let mut subscribers = self.subscribers.lock().expect("view bus poisoned");
        subscribers.retain(|weak| weak.strong_count() > 0);
        let closed = *self.closed.lock().expect("view bus poisoned");
        let state = Arc::new(Mutex::new(SubscriberState {
            queue: VecDeque::new(),
            capacity: DEFAULT_UPDATE_CAPACITY,
            bytes: 0,
            bytes_capacity: self.bytes_capacity,
            waker: None,
            closed,
        }));
        if !closed {
            subscribers.push(Arc::downgrade(&state));
        }
        RoomUpdateSubscription { state }
    }

    /// Deliver `update` to every live subscription in registration order. A
    /// `Converged` view coalesces into any earlier unread `Converged` for the
    /// same room (each view is a full snapshot, so the latest supersedes). When
    /// the queue is otherwise at capacity the oldest notice is dropped rather
    /// than grow without bound.
    pub(crate) fn broadcast(&self, update: RoomUpdate) {
        // One immutable payload is shared by every subscriber mailbox. Slow
        // consumers retain bounded Arc references, not one deep timeline clone
        // per subscriber.
        let update = Arc::new(update);
        let mut wakers = Vec::new();
        let mut subscribers = self.subscribers.lock().expect("view bus poisoned");
        subscribers.retain(|weak| weak.strong_count() > 0);
        for weak in subscribers.iter() {
            let Some(state) = weak.upgrade() else {
                continue;
            };
            let mut state = state.lock().expect("view subscriber poisoned");
            if state.closed {
                continue;
            }
            let coalesced = matches!(update.as_ref(), RoomUpdate::Converged(_))
                && coalesce_converged(&mut state, &update);
            if !coalesced {
                enqueue_update(&mut state, Arc::clone(&update));
            }
            if let Some(waker) = state.waker.take() {
                wakers.push(waker);
            }
        }
        drop(subscribers);
        for waker in wakers {
            waker.wake();
        }
    }

    /// Close every subscription. Already-queued updates remain readable; once a
    /// mailbox drains, its stream yields `None`.
    pub(crate) fn close(&self) {
        let mut wakers = Vec::new();
        {
            let mut subscribers = self.subscribers.lock().expect("view bus poisoned");
            *self.closed.lock().expect("view bus poisoned") = true;
            for weak in subscribers.iter() {
                let Some(state) = weak.upgrade() else {
                    continue;
                };
                let mut state = state.lock().expect("view subscriber poisoned");
                state.closed = true;
                if let Some(waker) = state.waker.take() {
                    wakers.push(waker);
                }
            }
            subscribers.clear();
        }
        for waker in wakers {
            waker.wake();
        }
    }
}

/// Replace the latest unread `Converged` for the same room with `update`,
/// returning `true` if one was found and replaced. A `Resyncing` or loss marker
/// is a causal barrier: a converged outcome after it must remain after that
/// marker, never moving in front of the reason that caused it.
fn coalesce_converged(state: &mut SubscriberState, update: &Arc<RoomUpdate>) -> bool {
    let new_bytes = estimated_update_bytes(update.as_ref());
    for existing in state.queue.iter_mut().rev() {
        let existing_update = existing.update.as_ref();
        // An unattributed loss can have removed a cause for any room, so no
        // converged view may cross it while coalescing.
        if matches!(existing_update, RoomUpdate::Lagged { room_id: None, .. }) {
            return false;
        }
        if existing_update.room_id() != update.room_id() {
            continue;
        }
        match existing_update {
            RoomUpdate::Converged(_) => {
                let replaced_total = state
                    .bytes
                    .saturating_sub(existing.charged_bytes)
                    .saturating_add(new_bytes);
                if new_bytes > state.bytes_capacity || replaced_total > state.bytes_capacity {
                    return false;
                }
                *existing = QueuedUpdate {
                    update: Arc::clone(update),
                    charged_bytes: new_bytes,
                };
                state.bytes = replaced_total;
                return true;
            }
            // Preserve the start-before-outcome contract for this room.
            RoomUpdate::Resyncing { .. } | RoomUpdate::Lagged { .. } => return false,
        }
    }
    false
}

/// Queue one update, surfacing count or byte loss as an in-order marker. Loss
/// markers use one fixed control allowance beyond the payload budget; repeated
/// trailing loss is accumulated rather than creating an unbounded side queue.
fn enqueue_update(state: &mut SubscriberState, update: Arc<RoomUpdate>) {
    let update_bytes = estimated_update_bytes(update.as_ref());
    if state.capacity == 0 {
        record_trailing_loss(state, update.room_id().cloned(), 1);
        return;
    }
    if update_bytes > state.bytes_capacity {
        // A valid authoritative view may exceed the ordinary mailbox budget.
        // Keep exactly one such shared Arc as a recovery allowance: otherwise
        // the sole convergence could be replaced by Lagged with no later view
        // guaranteed. Clear older work so repeated giant views cannot stack.
        let mut dropped = 0_u64;
        let mut attribution: Option<Option<RoomId>> = None;
        while let Some(old) = state.queue.pop_front() {
            dropped = dropped.saturating_add(match old.update.as_ref() {
                RoomUpdate::Lagged { dropped, .. } => *dropped,
                _ => 1,
            });
            let next = old.update.room_id().cloned();
            attribution = Some(match attribution {
                None => next,
                Some(current) => merge_room_id(current, next),
            });
        }
        state.bytes = 0;
        if dropped > 0 {
            record_front_loss(state, attribution.flatten(), dropped);
        }
        // The payload allocation is shared across all subscribers; only this
        // one pointer is exempt from ordinary byte accounting.
        state.queue.push_back(QueuedUpdate {
            update,
            charged_bytes: 0,
        });
        return;
    }

    let mut dropped = 0_u64;
    // Outer None means no evicted item has been observed yet; inner None means
    // the observed loss is already unattributed/mixed.
    let mut attribution: Option<Option<RoomId>> = None;
    while !state.queue.is_empty()
        && (state.queue.len() >= state.capacity
            || state.bytes.saturating_add(update_bytes) > state.bytes_capacity)
    {
        let old = state.queue.pop_front().expect("queue was nonempty");
        state.bytes = state.bytes.saturating_sub(old.charged_bytes);
        dropped = dropped.saturating_add(match old.update.as_ref() {
            RoomUpdate::Lagged { dropped, .. } => *dropped,
            _ => 1,
        });
        let next = old.update.room_id().cloned();
        attribution = Some(match attribution {
            None => next,
            Some(current) => merge_room_id(current, next),
        });
    }
    if state.bytes.saturating_add(update_bytes) > state.bytes_capacity {
        // Defensive only: `update_bytes <= bytes_capacity`, so removing all
        // ordinary payloads normally makes it fit. Any rejected new item is a
        // trailing loss, distinct from evictions at the front boundary.
        if dropped > 0 {
            record_front_loss(state, attribution.flatten(), dropped);
        }
        record_trailing_loss(state, update.room_id().cloned(), 1);
        return;
    }
    if dropped > 0 {
        // Evicted updates preceded every item still retained. Put the marker at
        // the front so consumers learn of loss before observing later state.
        record_front_loss(state, attribution.flatten(), dropped);
    }
    state.bytes = state.bytes.saturating_add(update_bytes);
    state.queue.push_back(QueuedUpdate {
        update,
        charged_bytes: update_bytes,
    });
}

fn record_front_loss(state: &mut SubscriberState, room_id: Option<RoomId>, dropped: u64) {
    if let Some(first) = state.queue.front_mut() {
        if matches!(first.update.as_ref(), RoomUpdate::Lagged { .. }) {
            if let RoomUpdate::Lagged {
                room_id: marker_room,
                dropped: marker_dropped,
            } = Arc::make_mut(&mut first.update)
            {
                *marker_room = merge_room_id(marker_room.take(), room_id);
                *marker_dropped = marker_dropped.saturating_add(dropped);
                return;
            }
        }
    }
    state.queue.push_front(QueuedUpdate {
        update: Arc::new(RoomUpdate::Lagged { room_id, dropped }),
        charged_bytes: 0,
    });
}

fn record_trailing_loss(state: &mut SubscriberState, room_id: Option<RoomId>, dropped: u64) {
    if let Some(last) = state.queue.back_mut() {
        if matches!(last.update.as_ref(), RoomUpdate::Lagged { .. }) {
            if let RoomUpdate::Lagged {
                room_id: marker_room,
                dropped: marker_dropped,
            } = Arc::make_mut(&mut last.update)
            {
                *marker_room = merge_room_id(marker_room.take(), room_id);
                *marker_dropped = marker_dropped.saturating_add(dropped);
                return;
            }
        }
    }
    state.queue.push_back(QueuedUpdate {
        update: Arc::new(RoomUpdate::Lagged { room_id, dropped }),
        charged_bytes: 0,
    });
}

fn estimated_update_bytes(update: &RoomUpdate) -> u64 {
    const FIXED: u64 = 64;
    match update {
        RoomUpdate::Resyncing { room_id, .. } => {
            FIXED.saturating_add(room_id.as_str().len() as u64)
        }
        RoomUpdate::Converged(view) => {
            let timeline = view
                .timeline
                .iter()
                .map(estimated_event_bytes)
                .fold(0_u64, u64::saturating_add);
            let members = view
                .members
                .iter()
                .map(|row| 64_u64.saturating_add(row.subject_id.as_str().len() as u64))
                .fold(0_u64, u64::saturating_add);
            let peers = view
                .peers
                .iter()
                .map(|row| {
                    64_u64
                        .saturating_add(row.subject_id.as_str().len() as u64)
                        .saturating_add(row.device_id.as_str().len() as u64)
                })
                .fold(0_u64, u64::saturating_add);
            FIXED
                .saturating_add(view.room_id.as_str().len() as u64)
                .saturating_add(timeline)
                .saturating_add(members)
                .saturating_add(peers)
        }
        // A loss marker is the fixed control allowance. Identifier length is
        // independently bounded at room activation.
        RoomUpdate::Lagged { .. } => 0,
    }
}

fn merge_room_id(current: Option<RoomId>, next: Option<RoomId>) -> Option<RoomId> {
    match (current, next) {
        (Some(current), Some(next)) if current == next => Some(current),
        (None, _) | (_, None) => None,
        (Some(_), Some(_)) => None,
    }
}

/// An independent, live view of the reconciler's [`RoomUpdate`] stream.
///
/// Implements [`futures::Stream`]. Each subscription observes every update; a
/// `Converged` view supersedes an earlier unread one for the same room. The
/// stream yields `None` once the bus has closed and the mailbox is drained.
pub struct RoomUpdateSubscription {
    state: Arc<Mutex<SubscriberState>>,
}

impl Stream for RoomUpdateSubscription {
    type Item = RoomUpdate;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut state = self.state.lock().expect("view subscriber poisoned");
        if let Some(queued) = state.queue.pop_front() {
            state.bytes = state.bytes.saturating_sub(queued.charged_bytes);
            return Poll::Ready(Some(queued.update.as_ref().clone()));
        }
        if state.closed {
            return Poll::Ready(None);
        }
        state.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reconcile::reason::ResyncReason;

    fn view(room: &str, generation: u64) -> RoomView {
        RoomView {
            room_id: RoomId::new(room),
            generation,
            timeline: Vec::new(),
            members: Vec::new(),
            peers: Vec::new(),
            reachability: Reachability::Offline,
        }
    }

    #[test]
    fn converged_views_coalesce_per_room() {
        let bus = RoomUpdateBus::new();
        let sub = bus.subscribe();
        bus.broadcast(RoomUpdate::Converged(view("r", 1)));
        bus.broadcast(RoomUpdate::Converged(view("r", 2)));
        let state = sub.state.lock().expect("subscriber");
        assert_eq!(state.queue.len(), 1, "latest converged view wins");
        assert!(matches!(
            state.queue.front().map(|queued| queued.update.as_ref()),
            Some(RoomUpdate::Converged(v)) if v.generation == 2
        ));
    }

    #[test]
    fn resyncing_barrier_stays_before_following_converged_view() {
        let bus = RoomUpdateBus::new();
        let sub = bus.subscribe();
        let room = RoomId::new("r");
        bus.broadcast(RoomUpdate::Converged(view("r", 1)));
        bus.broadcast(RoomUpdate::Resyncing {
            room_id: room.clone(),
            resync: ResyncRequired {
                generation: 2,
                reason: ResyncReason::Reconnect,
            },
        });
        bus.broadcast(RoomUpdate::Converged(view("r", 2)));
        bus.broadcast(RoomUpdate::Converged(view("r", 3)));

        let state = sub.state.lock().expect("subscriber");
        assert!(
            matches!(state.queue.front().map(|queued| queued.update.as_ref()), Some(RoomUpdate::Converged(v)) if v.generation == 1)
        );
        assert!(
            matches!(state.queue.get(1).map(|queued| queued.update.as_ref()), Some(RoomUpdate::Resyncing { resync, .. }) if resync.generation == 2)
        );
        assert!(
            matches!(state.queue.get(2).map(|queued| queued.update.as_ref()), Some(RoomUpdate::Converged(v)) if v.generation == 3)
        );
    }

    #[test]
    fn sole_oversized_convergence_remains_recoverable() {
        let bus = RoomUpdateBus::with_byte_capacity(1);
        let sub = bus.subscribe();
        bus.broadcast(RoomUpdate::Converged(view("r", 1)));
        let state = sub.state.lock().expect("subscriber");
        assert_eq!(state.bytes, 0);
        assert_eq!(state.queue.len(), 1);
        assert!(matches!(
            state.queue.front().map(|queued| queued.update.as_ref()),
            Some(RoomUpdate::Converged(view)) if view.generation == 1
        ));
    }

    #[test]
    fn repeated_oversized_views_keep_one_latest_authority_after_loss() {
        let bus = RoomUpdateBus::with_byte_capacity(1);
        let sub = bus.subscribe();
        bus.broadcast(RoomUpdate::Converged(view("r", 1)));
        bus.broadcast(RoomUpdate::Converged(view("r", 2)));
        let state = sub.state.lock().expect("subscriber");
        assert_eq!(state.bytes, 0);
        assert_eq!(state.queue.len(), 2);
        assert!(matches!(
            state.queue.front().map(|queued| queued.update.as_ref()),
            Some(RoomUpdate::Lagged { dropped: 1, .. })
        ));
        assert!(matches!(
            state.queue.back().map(|queued| queued.update.as_ref()),
            Some(RoomUpdate::Converged(view)) if view.generation == 2
        ));
    }

    #[test]
    fn oversized_allowance_never_erases_ordinary_byte_charges() {
        let small = RoomUpdate::Resyncing {
            room_id: RoomId::new("r2"),
            resync: ResyncRequired {
                generation: 1,
                reason: ResyncReason::Reconnect,
            },
        };
        let small_bytes = estimated_update_bytes(&small);
        let cap = small_bytes.saturating_mul(2);
        let big_room = "r".repeat(cap as usize);
        let big = RoomUpdate::Converged(view(&big_room, 1));
        assert!(estimated_update_bytes(&big) > cap);
        let bus = RoomUpdateBus::with_byte_capacity(cap);
        let sub = bus.subscribe();
        bus.broadcast(big);
        bus.broadcast(small);
        for (room, generation) in [("r3", 2), ("r4", 3)] {
            bus.broadcast(RoomUpdate::Resyncing {
                room_id: RoomId::new(room),
                resync: ResyncRequired {
                    generation,
                    reason: ResyncReason::Reconnect,
                },
            });
        }
        let state = sub.state.lock().expect("subscriber");
        let charged = state
            .queue
            .iter()
            .map(|queued| queued.charged_bytes)
            .sum::<u64>();
        assert_eq!(state.bytes, charged);
        assert_eq!(state.bytes, cap);
        assert!(state.bytes <= state.bytes_capacity);
        assert!(matches!(
            state.queue.front().map(|queued| queued.update.as_ref()),
            Some(RoomUpdate::Lagged { dropped: 2, .. })
        ));
    }

    #[test]
    fn mailbox_overflow_emits_loss_marker() {
        let bus = RoomUpdateBus::new();
        let sub = bus.subscribe();
        for generation in 0..=DEFAULT_UPDATE_CAPACITY as u64 {
            bus.broadcast(RoomUpdate::Resyncing {
                room_id: RoomId::new("r"),
                resync: ResyncRequired {
                    generation,
                    reason: ResyncReason::Reconnect,
                },
            });
        }
        let state = sub.state.lock().expect("subscriber");
        assert!(
            matches!(
                state.queue.front().map(|queued| queued.update.as_ref()),
                Some(RoomUpdate::Lagged {
                    room_id: Some(room_id),
                    dropped: 1,
                }) if room_id.as_str() == "r"
            ),
            "mailbox loss must precede every retained later update and remain attributed"
        );
        assert!(matches!(
            state.queue.get(1).map(|queued| queued.update.as_ref()),
            Some(RoomUpdate::Resyncing { resync, .. }) if resync.generation == 1
        ));
    }

    #[test]
    fn resyncing_notices_are_each_observable() {
        let bus = RoomUpdateBus::new();
        let sub = bus.subscribe();
        for generation in 1..=3 {
            bus.broadcast(RoomUpdate::Resyncing {
                room_id: RoomId::new("r"),
                resync: ResyncRequired {
                    generation,
                    reason: ResyncReason::Reconnect,
                },
            });
        }
        let state = sub.state.lock().expect("subscriber");
        assert_eq!(state.queue.len(), 3, "each resync notice is distinct");
    }

    #[test]
    fn debug_redacts_event_payloads() {
        let event: Event = serde_json::from_str(
            r#"{"pos":0,"event_id":"event-safe-id","at":"1970-01-01T00:00:00Z","author":{"state":"unresolved"},"kind":"message","content":{"body":"secret-message-body"}}"#,
        )
        .expect("event json deserializes");
        let view = RoomView {
            room_id: RoomId::new("room-safe-id"),
            generation: 7,
            timeline: vec![event],
            members: Vec::new(),
            peers: Vec::new(),
            reachability: Reachability::Offline,
        };
        let rendered = format!("{view:?}");
        assert!(rendered.contains("room-safe-id"));
        assert!(!rendered.contains("event-safe-id"));
        assert!(rendered.contains("timeline_range"));
        assert!(
            !rendered.contains("secret-message-body"),
            "RoomView Debug leaked an event payload: {rendered}"
        );

        let update = format!("{:?}", RoomUpdate::Converged(view));
        assert!(
            !update.contains("secret-message-body"),
            "RoomUpdate Debug leaked an event payload: {update}"
        );
    }

    #[test]
    fn subscription_outliving_the_bus_is_closed_on_last_owner_drop() {
        let sub = {
            let bus = RoomUpdateBus::new();
            bus.subscribe()
        };
        assert!(sub.state.lock().expect("subscriber").closed);
    }

    #[test]
    fn subscription_after_close_is_born_closed() {
        let bus = RoomUpdateBus::new();
        bus.close();
        let sub = bus.subscribe();
        assert!(sub.state.lock().expect("subscriber").closed);
    }
}
