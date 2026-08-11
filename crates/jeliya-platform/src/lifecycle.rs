//! The lifecycle capability (#174 §D7, AC-3): app resume, process restoration,
//! back, navigation, and window events, delivered on a bounded, multi-consumer,
//! **loss-visible** subscription.
//!
//! The subscription reuses the loss-visible fan-out philosophy of the client's
//! event bus: a slow consumer that missed events is told so
//! ([`LifecycleDelivery::Lagged`]); nothing is silently dropped. But the
//! **control intents that must not be lost or reordered** — [`LifecycleEvent::BackRequested`],
//! [`LifecycleEvent::ProcessRestored`], and terminal window events
//! ([`crate::window::WindowEvent::CloseRequested`]) — are always delivered
//! distinctly and never coalesced into another outcome (§K8). A lost Back that
//! silently exits, or a coalesced restore that skips resync, would be exactly
//! the honesty failure the clean-slate generation removes.
//!
//! The fan-out is a **local**, executor-agnostic bounded broadcast (Open
//! Question O-4 resolved to keep the platform crate free of the client seam):
//! an atomic-and-waker design with no wall clock and no runtime, `wasm32`-safe.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker};

use futures::Stream;

use crate::navigation::Route;
use crate::window::WindowEvent;

/// The closed lifecycle event model.
///
/// Only [`LifecycleEvent::Resumed`] is truly foreground; every other phase is
/// background. The model mirrors the behaviour inventory's Flutter lifecycle
/// with the honest Android additions (`ProcessRestored`) the architecture
/// already fixed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LifecycleEvent {
    /// Foreground and usable.
    Resumed,
    /// Left the foreground, carrying which phase so a consumer can distinguish
    /// "obscured" from "backgrounded".
    Backgrounded {
        /// The background phase entered.
        phase: BackgroundPhase,
    },
    /// The OS restored a previously killed process; any in-memory-only state is
    /// gone and the client must re-establish authoritatively. This is **not** a
    /// reconnect — it is a fresh process that must resync (`stream.resync`,
    /// never a fabricated socket reconnect).
    ProcessRestored,
    /// A system/predictive Back intent. The shared router consumes it and
    /// answers from the route; Back never mutates unseen state. An unconsumed
    /// `BackRequested` must **not** silently become an exit.
    BackRequested,
    /// A platform navigation intent (deep link / external route change).
    NavigationRequested {
        /// The requested route.
        route: Route,
    },
    /// A window event on platforms that have windows (desktop).
    Window(WindowEvent),
}

impl LifecycleEvent {
    /// Whether this is a control intent that must never be lost, reordered, or
    /// coalesced (§K8): a Back, a process restoration, or a terminal window
    /// event.
    pub fn is_control(&self) -> bool {
        match self {
            LifecycleEvent::BackRequested | LifecycleEvent::ProcessRestored => true,
            LifecycleEvent::Window(event) => event.is_control(),
            _ => false,
        }
    }
}

/// The background phase carried by [`LifecycleEvent::Backgrounded`] (mirrors
/// Flutter's `AppLifecycleState` non-`resumed` phases).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BackgroundPhase {
    /// Transiently inactive (a system overlay, an incoming call banner).
    Inactive,
    /// Backgrounded but resident.
    Paused,
    /// Hidden (all views obscured).
    Hidden,
    /// Detached — the engine is running with no attached view.
    Detached,
}

/// One item delivered by a [`LifecycleSubscription`].
///
/// Either a [`LifecycleEvent`] or a loss marker telling the consumer it lagged
/// the bounded buffer and missed `dropped` ordinary events. Control intents are
/// never dropped, so a `Lagged` never stands in for a Back or a restore.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LifecycleDelivery {
    /// A delivered lifecycle event.
    Event(LifecycleEvent),
    /// This consumer lagged the bounded buffer and missed `dropped` ordinary
    /// events. It must reconcile (typically by re-reading state). Dropping
    /// events and saying nothing is exactly the honesty failure this marker
    /// exists to prevent.
    Lagged {
        /// How many ordinary lifecycle events this consumer missed.
        dropped: u64,
    },
}

/// The lifecycle capability: subscribe to the loss-visible event stream.
pub trait Lifecycle {
    /// Register an independent subscription. Each observes every event; a slow
    /// consumer is told it lagged rather than silently starved.
    fn subscribe(&self) -> LifecycleSubscription;
}

/// The default per-subscription buffer depth before an ordinary event overflow
/// is reported as [`LifecycleDelivery::Lagged`]. Large enough that a
/// deterministic test's scripted sequences never lag by accident.
const DEFAULT_CAPACITY: usize = 256;

/// One subscriber's private mailbox.
struct SubscriberState {
    buffer: VecDeque<LifecycleDelivery>,
    dropped: u64,
    capacity: usize,
    waker: Option<Waker>,
    closed: bool,
}

/// A local, executor-agnostic bounded broadcast of [`LifecycleEvent`]s.
///
/// Any [`Lifecycle`] implementation (the fakes here; the M3–M5 targets later)
/// owns one and calls [`LifecycleBus::emit`] as platform events arrive.
/// Ordinary events overflow into a per-subscriber loss count; control intents
/// are always appended distinctly and never coalesced.
pub struct LifecycleBus {
    subscribers: Mutex<Vec<Weak<Mutex<SubscriberState>>>>,
    closed: AtomicBool,
}

impl Default for LifecycleBus {
    fn default() -> Self {
        Self::new()
    }
}

impl LifecycleBus {
    /// A fresh bus with no subscribers.
    pub fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
        }
    }

    /// Register an independent subscription. Created live: it does not receive
    /// past events. A subscription created after the bus closed is born closed.
    pub fn subscribe(&self) -> LifecycleSubscription {
        let mut subscribers = self.subscribers.lock().expect("lifecycle bus poisoned");
        subscribers.retain(|weak| weak.strong_count() > 0);
        let closed = self.closed.load(Ordering::Relaxed);
        let state = Arc::new(Mutex::new(SubscriberState {
            buffer: VecDeque::new(),
            dropped: 0,
            capacity: DEFAULT_CAPACITY,
            waker: None,
            closed,
        }));
        if !closed {
            subscribers.push(Arc::downgrade(&state));
        }
        LifecycleSubscription { state }
    }

    /// Deliver `event` to every live subscription in registration order.
    ///
    /// A **control intent** ([`LifecycleEvent::is_control`]) is always appended
    /// distinctly — it is rare and at-most-a-few per lifetime, so it cannot
    /// unbound the mailbox, and losing it would be an honesty failure. An
    /// **ordinary** event at capacity is dropped and counted, surfaced as a
    /// [`LifecycleDelivery::Lagged`] occupying the position in the sequence
    /// where the loss occurred — never a silent loss, never reported after a
    /// later event.
    pub fn emit(&self, event: LifecycleEvent) {
        let mut wakers = Vec::new();
        let mut subscribers = self.subscribers.lock().expect("lifecycle bus poisoned");
        subscribers.retain(|weak| weak.strong_count() > 0);
        let is_control = event.is_control();
        for weak in subscribers.iter() {
            let Some(state) = weak.upgrade() else {
                continue;
            };
            let mut state = state.lock().expect("lifecycle subscriber poisoned");
            if state.closed {
                continue;
            }
            // Materialize any pending loss before appending this event, so the
            // marker keeps its position: a consumer learns about the loss
            // before it sees anything delivered after the loss. A control
            // intent flushes it unconditionally (it always appends); an
            // ordinary event flushes it when a slot is free.
            let has_slot = state.buffer.len() < state.capacity;
            if state.dropped > 0 && (is_control || has_slot) {
                let dropped = std::mem::take(&mut state.dropped);
                state
                    .buffer
                    .push_back(LifecycleDelivery::Lagged { dropped });
            }
            if is_control || state.buffer.len() < state.capacity {
                state
                    .buffer
                    .push_back(LifecycleDelivery::Event(event.clone()));
            } else {
                state.dropped = state.dropped.saturating_add(1);
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

    /// Close every subscription. Already-buffered deliveries remain readable;
    /// once a mailbox drains, its stream yields `None`. Subscriptions created
    /// after this call are born closed.
    pub fn close(&self) {
        let mut wakers = Vec::new();
        {
            let mut subscribers = self.subscribers.lock().expect("lifecycle bus poisoned");
            self.closed.store(true, Ordering::Relaxed);
            for weak in subscribers.iter() {
                let Some(state) = weak.upgrade() else {
                    continue;
                };
                let mut state = state.lock().expect("lifecycle subscriber poisoned");
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

impl std::fmt::Debug for LifecycleBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LifecycleBus").finish_non_exhaustive()
    }
}

/// An independent, live view of the lifecycle event stream.
///
/// Implements [`Stream`]. Each [`Lifecycle::subscribe`] returns a distinct
/// subscription; every one observes every control intent, and a
/// [`LifecycleDelivery::Lagged`] marker occupies the position where any
/// ordinary-event loss occurred. Yields `None` once the bus has closed and the
/// mailbox is drained.
pub struct LifecycleSubscription {
    state: Arc<Mutex<SubscriberState>>,
}

impl Stream for LifecycleSubscription {
    type Item = LifecycleDelivery;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut state = self.state.lock().expect("lifecycle subscriber poisoned");
        if let Some(delivery) = state.buffer.pop_front() {
            return Poll::Ready(Some(delivery));
        }
        if state.dropped > 0 {
            let dropped = std::mem::take(&mut state.dropped);
            return Poll::Ready(Some(LifecycleDelivery::Lagged { dropped }));
        }
        if state.closed {
            return Poll::Ready(None);
        }
        state.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl LifecycleSubscription {
    /// Poll once outside an executor, for deterministic tests: return the next
    /// buffered delivery without registering a waker, or `None` if the mailbox
    /// is momentarily empty. This never blocks and never depends on task
    /// scheduling.
    pub fn try_next(&self) -> Option<LifecycleDelivery> {
        let mut state = self.state.lock().expect("lifecycle subscriber poisoned");
        if let Some(delivery) = state.buffer.pop_front() {
            return Some(delivery);
        }
        if state.dropped > 0 {
            let dropped = std::mem::take(&mut state.dropped);
            return Some(LifecycleDelivery::Lagged { dropped });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_intents_are_delivered_distinctly() {
        let bus = LifecycleBus::new();
        let sub = bus.subscribe();
        bus.emit(LifecycleEvent::BackRequested);
        bus.emit(LifecycleEvent::ProcessRestored);
        bus.emit(LifecycleEvent::Window(WindowEvent::CloseRequested));
        assert_eq!(
            sub.try_next(),
            Some(LifecycleDelivery::Event(LifecycleEvent::BackRequested))
        );
        assert_eq!(
            sub.try_next(),
            Some(LifecycleDelivery::Event(LifecycleEvent::ProcessRestored))
        );
        assert_eq!(
            sub.try_next(),
            Some(LifecycleDelivery::Event(LifecycleEvent::Window(
                WindowEvent::CloseRequested
            )))
        );
        assert_eq!(sub.try_next(), None);
    }

    #[test]
    fn ordinary_overflow_is_loss_visible_but_control_survives() {
        let bus = LifecycleBus::new();
        let sub = bus.subscribe();
        // Overflow the buffer with ordinary events, then emit a control intent.
        for _ in 0..(DEFAULT_CAPACITY + 10) {
            bus.emit(LifecycleEvent::Resumed);
        }
        bus.emit(LifecycleEvent::BackRequested);
        // Drain: the first DEFAULT_CAPACITY are Resumed, then a Lagged marker
        // for the 10 dropped, then the control BackRequested delivered
        // distinctly (never dropped).
        let mut resumed = 0;
        let mut lagged = 0;
        let mut saw_back = false;
        while let Some(delivery) = sub.try_next() {
            match delivery {
                LifecycleDelivery::Event(LifecycleEvent::Resumed) => resumed += 1,
                LifecycleDelivery::Event(LifecycleEvent::BackRequested) => saw_back = true,
                LifecycleDelivery::Lagged { dropped } => lagged += dropped,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(resumed, DEFAULT_CAPACITY);
        assert_eq!(lagged, 10);
        assert!(saw_back, "the control intent survived the overflow");
    }

    #[test]
    fn a_closed_bus_ends_the_stream_after_draining() {
        let bus = LifecycleBus::new();
        let sub = bus.subscribe();
        bus.emit(LifecycleEvent::Resumed);
        bus.close();
        assert_eq!(
            sub.try_next(),
            Some(LifecycleDelivery::Event(LifecycleEvent::Resumed))
        );
        // Drained; a real poll would now yield None. `try_next` returns None
        // for "momentarily empty" too, which is indistinguishable here — the
        // stream-level None is asserted in the integration suite.
        assert_eq!(sub.try_next(), None);
    }
}
