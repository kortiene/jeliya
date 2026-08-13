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
//! the latest per room rather than an unbounded backlog.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker};

use futures::Stream;
use jeliya_api::{Event, MemberRow, PeerRow, Reachability, RoomId};

use crate::reconcile::reason::ResyncRequired;

/// The converged, per-room view the room UI (#178+) renders.
///
/// The timeline is **gap-free and position-ordered** (dense `pos`, deduplicated
/// by `event_id`), and the membership/presence snapshots are **replaced
/// wholesale** from authoritative reads — never merged, so a missed removal
/// converges (§R8). `generation` is the reconciler-local epoch this view was
/// reconciled under (§R4), so a consumer can discard a view older than one it
/// already holds.
#[derive(Clone, PartialEq, Debug)]
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

/// One item a [`RoomUpdate`] consumer observes.
#[derive(Clone, PartialEq, Debug)]
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
}

impl RoomUpdate {
    /// The room this update concerns.
    fn room_id(&self) -> &RoomId {
        match self {
            RoomUpdate::Resyncing { room_id, .. } => room_id,
            RoomUpdate::Converged(view) => &view.room_id,
        }
    }
}

/// The default number of pending [`RoomUpdate`]s one subscription holds before
/// the oldest un-coalesced notice is dropped. Converged views coalesce per room
/// and so do not count against growth; this bounds the `Resyncing` notices a
/// stalled consumer can accumulate.
const DEFAULT_UPDATE_CAPACITY: usize = 256;

/// One subscriber's private mailbox on the [`RoomUpdateBus`].
struct SubscriberState {
    /// Delivered-but-unread updates, in broadcast order. A `Converged` view
    /// replaces any earlier unread `Converged` for the same room (latest wins).
    queue: VecDeque<RoomUpdate>,
    /// The depth before the oldest notice is dropped.
    capacity: usize,
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
}

impl RoomUpdateBus {
    /// A fresh bus with no subscribers.
    pub(crate) fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
            closed: Mutex::new(false),
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
            let coalesced = matches!(update, RoomUpdate::Converged(_))
                && coalesce_converged(&mut state.queue, &update);
            if !coalesced {
                if state.queue.len() >= state.capacity {
                    state.queue.pop_front();
                }
                state.queue.push_back(update.clone());
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

/// Replace an earlier unread `Converged` for the same room with `update`,
/// returning `true` if one was found and replaced. This is the per-room
/// latest-wins coalescing that keeps a stalled consumer's mailbox bounded by the
/// number of active rooms.
fn coalesce_converged(queue: &mut VecDeque<RoomUpdate>, update: &RoomUpdate) -> bool {
    for existing in queue.iter_mut() {
        if matches!(existing, RoomUpdate::Converged(_)) && existing.room_id() == update.room_id() {
            *existing = update.clone();
            return true;
        }
    }
    false
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
        if let Some(update) = state.queue.pop_front() {
            return Poll::Ready(Some(update));
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
            state.queue.front(),
            Some(RoomUpdate::Converged(v)) if v.generation == 2
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
    fn subscription_after_close_is_born_closed() {
        let bus = RoomUpdateBus::new();
        bus.close();
        let sub = bus.subscribe();
        assert!(sub.state.lock().expect("subscriber").closed);
    }
}
