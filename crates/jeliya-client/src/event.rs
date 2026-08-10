//! The client state model, the four-signal event model, and the
//! multi-consumer event fan-out (#167 §D3–§D5).
//!
//! The architecture names four signals the seam *models* — `Push`,
//! `StateChanged`, `Gap`, and `ResyncRequired`. On the wire a `gap` is a push
//! and `resync_required` is an error reply; the seam lifts both to first-class
//! [`ClientEvent`] arms so a component gets a clean four-way model instead of
//! re-deriving them. A fifth arm, [`ClientEvent::Lagged`], surfaces *local*
//! fan-out overflow — a slow consumer that missed live pushes is told so,
//! never silently starved.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker};

use futures::Stream;
use jeliya_api::{
    ByteTotal, DeviceId, Event, GapReason, GapTo, Link, OpId, Push, RoomId, SubjectId,
};

/// The honest, observable client lifecycle (#167 §D4).
///
/// One enum serves every adapter: a component renders `State` identically
/// regardless of which backend is behind the handle. The honest differences
/// between adapters are *which* transitions occur, not *which type* the
/// component branches on — which is how the seam keeps per-component `cfg`
/// logic out of the UI (AC-6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Never started, or fully stopped-then-idle. No backend activity.
    Idle,
    /// A connect/activation attempt is in progress; not yet usable.
    Connecting,
    /// Usable: the backend has completed whatever validation its transport
    /// requires and will accept calls.
    Ready,
    /// Was [`State::Ready`], lost usability (connection dropped / transport
    /// interrupted) and is recovering. Calls may be refused with a
    /// delivery-classified error.
    Interrupted,
    /// [`stop`](crate::ClientHandle::stop) is draining accepted work; new
    /// calls are refused.
    Stopping,
    /// `stop` completed: all accepted work settled, all event streams closed.
    Stopped,
    /// A terminal, non-recoverable failure (for example a generation/protocol
    /// gate refusal the client cannot retry past). Carries no auto-retry.
    Failed,
}

/// The four-signal event model plus a local-overflow signal (#167 §D5).
///
/// **Replies never travel here.** A reply is delivered to exactly one place —
/// the future returned by [`call`](crate::ClientHandle::call). Pushes travel
/// on this fan-out. That mirrors the wire rule "a push carries `t` and never
/// `id`; a reply carries `id` and never `t`" and makes reply-vs-push confusion
/// unrepresentable at the seam.
#[derive(Clone, PartialEq, Debug)]
pub enum ClientEvent {
    /// A lifecycle transition ([`State`], §D4).
    StateChanged {
        /// The state left.
        from: State,
        /// The state entered.
        to: State,
    },
    /// A live room push that is neither a gap nor a resync: the wire `event`,
    /// `peer`, and `transfer` pushes, unchanged from `jeliya-api`.
    Push(RoomPush),
    /// A position discontinuity for one room (the wire `gap` push, lifted to a
    /// first-class event because "you missed data" renders differently from
    /// "here is new data").
    Gap {
        /// The room the discontinuity is in.
        room_id: RoomId,
        /// The position the gap starts after.
        from_pos: u64,
        /// Where the gap ends.
        to: GapTo,
        /// Why it exists.
        reason: GapReason,
    },
    /// The authoritative recovery instruction: discard back to `from_pos` and
    /// re-read. Synthesized by the kernel when it detects a discontinuity it
    /// cannot bridge, or lifted from a `resync_required` reply.
    ResyncRequired {
        /// The room to recover.
        room_id: RoomId,
        /// The position to re-read from.
        from_pos: u64,
    },
    /// **Local** fan-out overflow, distinct from a protocol [`ClientEvent::Gap`].
    /// This consumer lagged the bounded per-subscription buffer and missed
    /// live pushes; it must reconcile (typically by issuing `stream.resync`).
    /// Dropping pushes and saying nothing is exactly the honesty failure the
    /// clean-slate generation exists to remove.
    Lagged {
        /// The room, when the dropped pushes were all for one room; `None`
        /// when the overflow spanned rooms or the buffer cannot attribute it.
        room_id: Option<RoomId>,
        /// How many live pushes this consumer missed.
        dropped: u64,
    },
}

/// The non-gap pushes, reusing `jeliya-api`'s [`Push`] payloads verbatim.
///
/// Construct these from a wire [`Push`] via [`From<Push>`](ClientEvent) on
/// [`ClientEvent`], which routes [`Push::Gap`] to [`ClientEvent::Gap`] and the
/// rest here — the shapes are never redefined.
#[derive(Clone, PartialEq, Debug)]
pub enum RoomPush {
    /// A room event committed (the wire `event` push).
    Event {
        /// The room.
        room_id: RoomId,
        /// The committed event.
        event: Event,
    },
    /// A peer's link changed (the wire `peer` push).
    Peer {
        /// The room.
        room_id: RoomId,
        /// The peer's subject.
        subject_id: SubjectId,
        /// The peer's device.
        device_id: DeviceId,
        /// The new link state.
        link: Link,
        /// The connection generation that makes a stale teardown discardable.
        generation: u64,
    },
    /// A transfer made progress (the wire `transfer` push). Goes only to the
    /// principal that started the transfer.
    Transfer {
        /// The transfer's op_id.
        transfer_op_id: OpId,
        /// Bytes transferred so far.
        transferred_bytes: u64,
        /// The total, or genuinely unknown.
        total: ByteTotal,
    },
}

impl From<Push> for ClientEvent {
    /// Lift a wire push into the seam's event model: a `gap` becomes
    /// [`ClientEvent::Gap`], every other push becomes a [`ClientEvent::Push`].
    /// `resync_required` never arrives as a push (it is a reply), so it is not
    /// mapped here.
    fn from(push: Push) -> Self {
        match push {
            Push::Event { room_id, event } => ClientEvent::Push(RoomPush::Event { room_id, event }),
            Push::Gap {
                room_id,
                from_pos,
                to,
                reason,
            } => ClientEvent::Gap {
                room_id,
                from_pos,
                to,
                reason,
            },
            Push::Peer {
                room_id,
                subject_id,
                device_id,
                link,
                generation,
            } => ClientEvent::Push(RoomPush::Peer {
                room_id,
                subject_id,
                device_id,
                link,
                generation,
            }),
            Push::Transfer {
                transfer_op_id,
                transferred_bytes,
                total,
            } => ClientEvent::Push(RoomPush::Transfer {
                transfer_op_id,
                transferred_bytes,
                total,
            }),
        }
    }
}

/// The default per-subscription buffer depth before overflow is reported as
/// [`ClientEvent::Lagged`]. Large enough that the deterministic mock's scripted
/// sequences never lag by accident; overflow is a scripted or genuine
/// slow-consumer condition, never test noise.
const DEFAULT_FANOUT_CAPACITY: usize = 1024;

/// One subscriber's private mailbox on the fan-out.
struct SubscriberState {
    /// Delivered-but-unread events, in broadcast order.
    buffer: VecDeque<ClientEvent>,
    /// Live pushes dropped because `buffer` was full, not yet surfaced as a
    /// [`ClientEvent::Lagged`].
    dropped: u64,
    /// The buffer depth before overflow.
    capacity: usize,
    /// The waker to notify when an event (or close) arrives.
    waker: Option<Waker>,
    /// Set once the bus closes; the stream ends after the buffer drains.
    closed: bool,
}

/// The multi-consumer event fan-out (#167 §D3).
///
/// Every active [`EventSubscription`] observes **every** broadcast event, so
/// two components subscribing cannot starve each other — the opposite of a
/// single shared `mpsc` receiver, where whichever task polls first *consumes*
/// the item (the "silent stealing" the acceptance criterion forbids).
///
/// Ordering is fixed at broadcast time: [`broadcast`](EventBus::broadcast)
/// pushes into every subscriber's mailbox synchronously, so the observable
/// event order does not depend on task scheduling and is identical on wasm and
/// native. This is what lets the mock prove push-before-response
/// deterministically.
pub(crate) struct EventBus {
    subscribers: Mutex<Vec<Weak<Mutex<SubscriberState>>>>,
    /// Set once [`close`](Self::close) has run; guarded by the `subscribers`
    /// lock (every load and store happens while holding it).
    closed: AtomicBool,
}

impl EventBus {
    /// A fresh bus with no subscribers.
    pub(crate) fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
        }
    }

    /// Register an independent subscription. A subscription created *after* an
    /// event does not receive that past event; pushes are live. A subscription
    /// created after the bus has closed is **born closed**: its stream yields
    /// `None` immediately instead of pending forever on a bus that will never
    /// broadcast again.
    pub(crate) fn subscribe(&self) -> EventSubscription {
        let mut subscribers = self.subscribers.lock().expect("event bus poisoned");
        let closed = self.closed.load(Ordering::Relaxed);
        let state = Arc::new(Mutex::new(SubscriberState {
            buffer: VecDeque::new(),
            dropped: 0,
            capacity: DEFAULT_FANOUT_CAPACITY,
            waker: None,
            closed,
        }));
        if !closed {
            subscribers.push(Arc::downgrade(&state));
        }
        EventSubscription { state }
    }

    /// Deliver `event` to every live subscription in registration order. A
    /// subscriber whose mailbox is full has an ordinary push dropped and its
    /// `dropped` counter incremented, surfaced as a [`ClientEvent::Lagged`]
    /// that occupies the position in the sequence where the loss occurred —
    /// never a silent loss, and never reported *after* later events. Lifecycle
    /// transitions ([`ClientEvent::StateChanged`]) are control events: they
    /// always reach the subscriber, but under backpressure the **flapping
    /// family coalesces** rather than grow the mailbox without bound — at
    /// capacity, a new `Connecting`/`Ready`/`Interrupted` transition merges
    /// into the last undelivered non-terminal `StateChanged` (its `from`
    /// stays, its `to` advances). Terminal transitions (`Stopping`,
    /// `Stopped`, `Failed`) always append **distinctly** — the seam
    /// guarantees a subscriber observes each of them, and they occur at most
    /// once per lifetime, so the mailbox stays bounded while a flapping
    /// connection cannot grow it indefinitely (AC-7/§K12).
    pub(crate) fn broadcast(&self, event: ClientEvent) {
        let mut subscribers = self.subscribers.lock().expect("event bus poisoned");
        subscribers.retain(|weak| weak.strong_count() > 0);
        let is_control = matches!(event, ClientEvent::StateChanged { .. });
        for weak in subscribers.iter() {
            let Some(state) = weak.upgrade() else {
                continue;
            };
            let mut state = state.lock().expect("subscriber poisoned");
            if state.closed {
                continue;
            }
            // At capacity, a NON-TERMINAL control event coalesces into the
            // last undelivered non-terminal StateChanged instead of appending
            // (endpoints stay honest: the merged transition spans old.from →
            // new.to). Terminal transitions (Stopping/Stopped/Failed) always
            // append distinctly — the seam guarantees a subscriber observes
            // them individually, and they occur at most once per lifetime, so
            // they cannot unbound the mailbox; only the flapping family
            // (Connecting/Ready/Interrupted) repeats without limit.
            let mut merged = false;
            if is_control && state.buffer.len() >= state.capacity {
                if let ClientEvent::StateChanged { to: new_to, .. } = &event {
                    let non_terminal = matches!(
                        new_to,
                        State::Connecting | State::Ready | State::Interrupted
                    );
                    if non_terminal {
                        if let Some(last_to) =
                            state
                                .buffer
                                .iter_mut()
                                .rev()
                                .find_map(|queued| match queued {
                                    ClientEvent::StateChanged { to, .. } => Some(to),
                                    _ => None,
                                })
                        {
                            if matches!(
                                *last_to,
                                State::Connecting | State::Ready | State::Interrupted
                            ) {
                                *last_to = *new_to;
                                merged = true;
                            }
                        }
                    }
                }
            }
            // Materialize any pending drop count as a `Lagged` marker before
            // appending, so the marker keeps its position in the sequence: a
            // consumer learns about the loss before it sees anything broadcast
            // after the loss. An appending control event flushes it
            // unconditionally; ordinary pushes flush when a slot is free. A
            // coalescing control event appends nothing, so the marker waits.
            if state.dropped > 0 && ((is_control && !merged) || state.buffer.len() < state.capacity)
            {
                let dropped = std::mem::take(&mut state.dropped);
                state.buffer.push_back(ClientEvent::Lagged {
                    room_id: None,
                    dropped,
                });
            }
            if is_control {
                if !merged {
                    state.buffer.push_back(event.clone());
                }
            } else if state.buffer.len() >= state.capacity {
                state.dropped = state.dropped.saturating_add(1);
            } else {
                state.buffer.push_back(event.clone());
            }
            if let Some(waker) = state.waker.take() {
                waker.wake();
            }
        }
    }

    /// Close every subscription. Already-buffered events (including a final
    /// `StateChanged { to: Stopped }`) remain readable; once a mailbox drains,
    /// its stream yields `None`. Subscriptions created after this call are
    /// born closed.
    pub(crate) fn close(&self) {
        let mut subscribers = self.subscribers.lock().expect("event bus poisoned");
        self.closed.store(true, Ordering::Relaxed);
        for weak in subscribers.iter() {
            let Some(state) = weak.upgrade() else {
                continue;
            };
            let mut state = state.lock().expect("subscriber poisoned");
            state.closed = true;
            if let Some(waker) = state.waker.take() {
                waker.wake();
            }
        }
        subscribers.clear();
    }
}

/// An independent, live view of the client's [`ClientEvent`] stream.
///
/// Implements [`futures::Stream`]. Each [`subscribe`](crate::ClientHandle::subscribe)
/// call returns a distinct subscription; every one observes every event. A
/// [`ClientEvent::Lagged`] marker summarizing dropped pushes occupies the
/// position in the sequence where the loss occurred, so nothing broadcast
/// after a loss is observed before the loss is reported; a trailing loss with
/// no subsequent broadcast surfaces once the buffer drains. The stream yields
/// `None` once the bus has closed and the mailbox is drained; a subscription
/// created after close is born closed and yields `None` immediately.
pub struct EventSubscription {
    state: Arc<Mutex<SubscriberState>>,
}

impl Stream for EventSubscription {
    type Item = ClientEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut state = self.state.lock().expect("subscriber poisoned");
        if let Some(event) = state.buffer.pop_front() {
            return Poll::Ready(Some(event));
        }
        if state.dropped > 0 {
            let dropped = std::mem::take(&mut state.dropped);
            return Poll::Ready(Some(ClientEvent::Lagged {
                room_id: None,
                dropped,
            }));
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

    /// AC-7/§K12: a stalled subscriber under a flapping connection cannot
    /// grow its mailbox without bound — at capacity, lifecycle transitions
    /// coalesce into the last undelivered `StateChanged`, and the honest
    /// final state (`Stopped` included) survives the coalescing.
    #[test]
    fn flapping_lifecycle_coalesces_for_a_stalled_subscriber() {
        let bus = EventBus::new();
        let sub = bus.subscribe();
        for _ in 0..(DEFAULT_FANOUT_CAPACITY * 3) {
            bus.broadcast(ClientEvent::StateChanged {
                from: State::Ready,
                to: State::Interrupted,
            });
            bus.broadcast(ClientEvent::StateChanged {
                from: State::Interrupted,
                to: State::Ready,
            });
        }
        bus.broadcast(ClientEvent::StateChanged {
            from: State::Ready,
            to: State::Stopped,
        });
        let state = sub.state.lock().expect("subscriber poisoned");
        assert!(
            state.buffer.len() <= DEFAULT_FANOUT_CAPACITY + 3,
            "the mailbox is bounded under flapping (capacity plus the at-most-\
             once terminal transitions), got {}",
            state.buffer.len()
        );
        let last_transition = state.buffer.iter().rev().find_map(|event| match event {
            ClientEvent::StateChanged { to, .. } => Some(*to),
            _ => None,
        });
        assert_eq!(
            last_transition,
            Some(State::Stopped),
            "the terminal transition appends distinctly and is observable"
        );
    }
}
