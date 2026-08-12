//! The lifecycle capability (#174 §D7, AC-3): app resume, process restoration,
//! back, navigation, and window events, delivered on a bounded, multi-consumer,
//! **loss-visible** subscription.
//!
//! The subscription reuses the loss-visible fan-out philosophy of the client's
//! event bus: a slow consumer that missed events is told so
//! ([`LifecycleDelivery::Lagged`]); nothing is silently dropped. The **control
//! intents that must never be silently lost or reordered** —
//! [`LifecycleEvent::BackRequested`], [`LifecycleEvent::ProcessRestored`], and
//! terminal window events ([`crate::window::WindowEvent::CloseRequested`]) —
//! are delivered losslessly *and* boundedly (§K8): a saturated mailbox
//! run-length-encodes a Back burst (every Back is still delivered, one per
//! poll), and a close/restore restated while the identical intent is still
//! undelivered is absorbed into the pending one (the consumer cannot have
//! answered an intent it has not yet received, so the restatement adds no
//! information). The mailbox therefore never exceeds `DEFAULT_CAPACITY +
//! CONTROL_ALLOWANCE` entries even while the consumer is stalled. A lost Back
//! that silently exits, or a coalesced restore that skips resync, would be
//! exactly the honesty failure the clean-slate generation removes — bounding
//! must never become dropping.
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
    /// Whether this is a control intent that must never be silently lost or
    /// reordered (§K8): a Back, a process restoration, or a terminal window
    /// event. Control intents survive overflow — a saturated mailbox
    /// run-length-encodes a Back burst and absorbs a restated close/restore
    /// into its still-undelivered twin, rather than dropping either.
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
/// the bounded buffer and missed `dropped` ordinary events. Control intents
/// are never silently lost, so a `Lagged` never stands in for a Back or a
/// restore — every Back is delivered (a saturated mailbox run-length-encodes
/// the burst), and a pending close/restore is delivered exactly once.
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

/// The hard number of slots control traffic may occupy **past** capacity in a
/// stalled mailbox. The accounting: a restated close/restore is absorbed into
/// its still-undelivered twin, so non-Back control entries cap at 1 each
/// (CloseRequested + ProcessRestored = 2); Back runs absorb into the trailing
/// run, so distinct runs must be separated by one of those non-Back entries,
/// capping runs at 3; that is 5 control slots, plus at most one `Lagged`
/// marker directly before each = 10. The mailbox therefore never exceeds
/// `DEFAULT_CAPACITY + CONTROL_ALLOWANCE` entries, however long the consumer
/// stalls and however hard the user mashes Back.
const CONTROL_ALLOWANCE: usize = 10;

/// One mailbox entry. `Backs` is a run-length-encoded burst of
/// [`LifecycleEvent::BackRequested`] absorbed at capacity: delivered one Back
/// per poll, so the consumer still observes every distinct intent (N adjacent
/// Backs carry exactly the information "Back N times" — lossless, O(1) memory
/// per run).
enum Slot {
    /// One ordinary delivery (an event or a loss marker).
    One(LifecycleDelivery),
    /// A run of `count` adjacent Back intents.
    Backs { count: u64 },
}

/// One subscriber's private mailbox.
struct SubscriberState {
    buffer: VecDeque<Slot>,
    dropped: u64,
    capacity: usize,
    waker: Option<Waker>,
    closed: bool,
}

impl SubscriberState {
    /// Materialize the pending loss count as a `Lagged` entry, so the marker
    /// keeps its position: the consumer learns about the loss before anything
    /// delivered after it.
    fn flush_lagged(&mut self) {
        if self.dropped > 0 {
            let dropped = std::mem::take(&mut self.dropped);
            self.buffer
                .push_back(Slot::One(LifecycleDelivery::Lagged { dropped }));
        }
    }

    /// Take the next delivery. A Back run is popped one Back per call so
    /// run-length encoding stays invisible to the consumer; the run slot is
    /// removed only when its count is exhausted.
    fn pop_delivery(&mut self) -> Option<LifecycleDelivery> {
        if let Some(Slot::Backs { count }) = self.buffer.front_mut() {
            if *count > 1 {
                *count -= 1;
            } else {
                self.buffer.pop_front();
            }
            return Some(LifecycleDelivery::Event(LifecycleEvent::BackRequested));
        }
        if let Some(Slot::One(delivery)) = self.buffer.pop_front() {
            return Some(delivery);
        }
        if self.dropped > 0 {
            let dropped = std::mem::take(&mut self.dropped);
            return Some(LifecycleDelivery::Lagged { dropped });
        }
        None
    }
}

/// A local, executor-agnostic bounded broadcast of [`LifecycleEvent`]s.
///
/// Any [`Lifecycle`] implementation (the fakes here; the M3–M5 targets later)
/// owns one and calls [`LifecycleBus::emit`] as platform events arrive.
/// Ordinary events overflow into a per-subscriber loss count. Control intents
/// are never *lost*, and the two kinds behave differently under saturation:
/// every `BackRequested` is delivered distinctly (a burst is run-length
/// encoded, one Back per poll, so none is merged away), while a
/// `ProcessRestored` or a terminal window event restated **while an identical
/// one is still undelivered** is absorbed into that pending intent. Coalescing
/// only identical undelivered restatements is what keeps the mailbox inside its
/// fixed control allowance without ever dropping an intent — resyncing twice
/// for one restore, or closing twice for one close, would be redundant work,
/// whereas two Backs mean two navigations.
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
    /// Below capacity every event appends distinctly. At capacity, an
    /// **ordinary** event is dropped and counted, surfaced as a
    /// [`LifecycleDelivery::Lagged`] occupying the position in the sequence
    /// where the loss occurred — never a silent loss, never reported after a
    /// later event. A **control intent** ([`LifecycleEvent::is_control`]) is
    /// never dropped, but its bounded representation depends on what a
    /// lossless one is: a Back burst is run-length-encoded into the trailing
    /// Back run (delivered one Back per poll — every distinct Back is still
    /// observed); a close/restore restated while the identical intent is
    /// still undelivered in the mailbox is absorbed into the pending one.
    /// Total mailbox growth is therefore capped at
    /// `DEFAULT_CAPACITY + CONTROL_ALLOWANCE`.
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
            let at_capacity = state.buffer.len() >= state.capacity;
            if is_control && at_capacity {
                if event == LifecycleEvent::BackRequested {
                    Self::append_back_saturated(&mut state);
                } else {
                    // ProcessRestored / Window(control): absorb into the
                    // identical still-undelivered intent if one is pending —
                    // the consumer cannot have answered an intent it has not
                    // yet received, so the restatement adds no information.
                    // (An O(capacity) scan, but only on this rare saturated
                    // path.)
                    let pending = state.buffer.iter().any(|slot| {
                        matches!(slot, Slot::One(LifecycleDelivery::Event(e)) if e == &event)
                    });
                    if !pending {
                        state.flush_lagged();
                        state
                            .buffer
                            .push_back(Slot::One(LifecycleDelivery::Event(event.clone())));
                    }
                    // If pending: append nothing; any pending drop count
                    // waits for the next append that can carry its marker.
                }
            } else if state.buffer.len() < state.capacity {
                // Materialize any pending loss before appending, so the
                // marker keeps its position.
                state.flush_lagged();
                if state.buffer.len() < state.capacity {
                    state
                        .buffer
                        .push_back(Slot::One(LifecycleDelivery::Event(event.clone())));
                } else if is_control {
                    // The flush itself consumed the last free slot; a control
                    // intent still appends (this is the first entry of the
                    // control allowance), an ordinary event overflows.
                    state
                        .buffer
                        .push_back(Slot::One(LifecycleDelivery::Event(event.clone())));
                } else {
                    state.dropped = state.dropped.saturating_add(1);
                }
            } else {
                state.dropped = state.dropped.saturating_add(1);
            }
            // The accounting in CONTROL_ALLOWANCE's doc, enforced: no append
            // path may push the mailbox past the hard ceiling.
            debug_assert!(
                state.buffer.len() <= state.capacity + CONTROL_ALLOWANCE,
                "lifecycle mailbox exceeded its hard ceiling"
            );
            if let Some(waker) = state.waker.take() {
                wakers.push(waker);
            }
        }
        drop(subscribers);
        for waker in wakers {
            waker.wake();
        }
    }

    /// Append one Back to a saturated mailbox: absorb it into the trailing
    /// Back run when the tail is one (trailing-only, so Backs are never
    /// reordered around other control intents), else open a new run. Any
    /// pending loss count becomes a `Lagged` marker directly **before** the
    /// run it interrupted — a repeat accumulates into that marker rather than
    /// growing the buffer.
    ///
    /// The marker therefore precedes the whole run even when the loss happened
    /// *between* two of its Backs. That is a deliberate, bounded approximation,
    /// not an oversight: placing it exactly would split the run, costing a slot
    /// per loss episode, and an alternating Back/loss burst would grow the
    /// mailbox without bound. See [`LifecycleSubscription`] for the contract
    /// this trades against.
    fn append_back_saturated(state: &mut SubscriberState) {
        let absorbed = match state.buffer.back_mut() {
            Some(Slot::Backs { count }) => {
                *count = count.saturating_add(1);
                true
            }
            Some(slot @ Slot::One(LifecycleDelivery::Event(LifecycleEvent::BackRequested))) => {
                *slot = Slot::Backs { count: 2 };
                true
            }
            _ => false,
        };
        if absorbed {
            if state.dropped > 0 {
                let dropped = std::mem::take(&mut state.dropped);
                // The loss happened before this Back: the marker sits
                // directly before the run (the run occupies the tail slot).
                let run_index = state.buffer.len() - 1;
                match run_index
                    .checked_sub(1)
                    .and_then(|before| state.buffer.get_mut(before))
                {
                    Some(Slot::One(LifecycleDelivery::Lagged { dropped: total })) => {
                        *total = total.saturating_add(dropped);
                    }
                    _ => {
                        state
                            .buffer
                            .insert(run_index, Slot::One(LifecycleDelivery::Lagged { dropped }));
                    }
                }
            }
        } else {
            state.flush_lagged();
            state.buffer.push_back(Slot::Backs { count: 1 });
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
/// subscription and every one observes every control intent, in order.
///
/// A [`LifecycleDelivery::Lagged`] marker reports ordinary-event loss at the
/// position it occurred, with **one bounded exception**: a loss that happens
/// while a run-length-encoded Back run sits at the tail is reported
/// immediately *before* that run rather than inside it, so the emission
/// `Back, dropped ordinary, Back` is observed as `Lagged{1}, Back, Back`.
/// Splitting the run to place the marker exactly would cost a slot per loss
/// episode and an adversarial alternation would then grow the mailbox without
/// bound — the very thing the run encoding exists to prevent (§K8 requires
/// lossless *and* bounded). Reporting early rather than late is the safer of
/// the two placements: the consumer reconciles its state before acting on the
/// Backs. Backs are never reordered relative to each other or to any other
/// control intent, and none is ever lost.
///
/// Yields `None` once the bus has closed and the mailbox is drained.
pub struct LifecycleSubscription {
    state: Arc<Mutex<SubscriberState>>,
}

impl Stream for LifecycleSubscription {
    type Item = LifecycleDelivery;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut state = self.state.lock().expect("lifecycle subscriber poisoned");
        if let Some(delivery) = state.pop_delivery() {
            return Poll::Ready(Some(delivery));
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
        self.state
            .lock()
            .expect("lifecycle subscriber poisoned")
            .pop_delivery()
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
    fn a_back_burst_cannot_grow_a_stalled_mailbox_without_bound() {
        let bus = LifecycleBus::new();
        let sub = bus.subscribe();
        for _ in 0..DEFAULT_CAPACITY {
            bus.emit(LifecycleEvent::Resumed);
        }
        for _ in 0..(DEFAULT_CAPACITY * 3) {
            bus.emit(LifecycleEvent::BackRequested);
        }
        {
            let state = sub.state.lock().expect("lifecycle subscriber poisoned");
            assert!(
                state.buffer.len() <= DEFAULT_CAPACITY + CONTROL_ALLOWANCE,
                "a stalled mailbox stays bounded under a Back burst, got {}",
                state.buffer.len()
            );
        }
        let mut backs = 0u64;
        while let Some(delivery) = sub.try_next() {
            if delivery == LifecycleDelivery::Event(LifecycleEvent::BackRequested) {
                backs += 1;
            }
        }
        assert_eq!(
            backs,
            (DEFAULT_CAPACITY * 3) as u64,
            "every Back intent is still delivered — bounding must not become dropping"
        );
    }

    #[test]
    fn control_growth_past_capacity_reaches_exactly_the_allowance_and_stops() {
        let bus = LifecycleBus::new();
        let sub = bus.subscribe();
        for _ in 0..DEFAULT_CAPACITY {
            bus.emit(LifecycleEvent::Resumed);
        }
        // Worst-case interleaving: each dropped ordinary earns one marker,
        // each control opens a new slot (runs separated by close/restore).
        for event in [
            LifecycleEvent::Resumed,
            LifecycleEvent::BackRequested,
            LifecycleEvent::Resumed,
            LifecycleEvent::Window(WindowEvent::CloseRequested),
            LifecycleEvent::Resumed,
            LifecycleEvent::BackRequested,
            LifecycleEvent::Resumed,
            LifecycleEvent::ProcessRestored,
            LifecycleEvent::Resumed,
            LifecycleEvent::BackRequested,
        ] {
            bus.emit(event);
        }
        {
            let state = sub.state.lock().expect("lifecycle subscriber poisoned");
            assert_eq!(
                state.buffer.len(),
                DEFAULT_CAPACITY + CONTROL_ALLOWANCE,
                "the worst-case interleaving reaches exactly the allowance"
            );
        }
        // Past the ceiling, further control restatements cause zero growth.
        for event in [
            LifecycleEvent::BackRequested,
            LifecycleEvent::Window(WindowEvent::CloseRequested),
            LifecycleEvent::ProcessRestored,
            LifecycleEvent::BackRequested,
        ] {
            bus.emit(event);
        }
        {
            let state = sub.state.lock().expect("lifecycle subscriber poisoned");
            assert_eq!(
                state.buffer.len(),
                DEFAULT_CAPACITY + CONTROL_ALLOWANCE,
                "the ceiling is hard: absorbed control traffic adds no entries"
            );
        }
        // Drain: the pending close and restore arrive exactly once, every
        // Back arrives, and Lagged totals equal the ordinaries dropped.
        let mut backs = 0u64;
        let mut closes = 0u64;
        let mut restores = 0u64;
        let mut lagged = 0u64;
        while let Some(delivery) = sub.try_next() {
            match delivery {
                LifecycleDelivery::Event(LifecycleEvent::BackRequested) => backs += 1,
                LifecycleDelivery::Event(LifecycleEvent::Window(WindowEvent::CloseRequested)) => {
                    closes += 1;
                }
                LifecycleDelivery::Event(LifecycleEvent::ProcessRestored) => restores += 1,
                LifecycleDelivery::Lagged { dropped } => lagged += dropped,
                _ => {}
            }
        }
        assert_eq!(backs, 5, "3 run-opening + 2 absorbed Backs all delivered");
        assert_eq!(
            closes, 1,
            "the restated close was absorbed into the pending one"
        );
        assert_eq!(restores, 1, "the restated restore was absorbed too");
        assert_eq!(lagged, 5, "loss markers account for every dropped ordinary");
    }

    #[test]
    /// Pins the bounded approximation documented on [`LifecycleSubscription`]:
    /// a loss between two absorbed Backs is reported immediately BEFORE the
    /// run, not inside it, because splitting the run per loss episode would
    /// make the mailbox unbounded. This test exists to make that trade
    /// explicit and to catch it silently changing.
    fn the_lagged_marker_sits_directly_before_the_absorbing_back_run() {
        let bus = LifecycleBus::new();
        let sub = bus.subscribe();
        for _ in 0..DEFAULT_CAPACITY {
            bus.emit(LifecycleEvent::Resumed);
        }
        bus.emit(LifecycleEvent::BackRequested); // opens the trailing run
        bus.emit(LifecycleEvent::Resumed); // dropped: 1
        bus.emit(LifecycleEvent::BackRequested); // absorbs; marker before the run
        bus.emit(LifecycleEvent::Resumed); // dropped: 1
        bus.emit(LifecycleEvent::BackRequested); // absorbs; marker accumulates
        {
            let state = sub.state.lock().expect("lifecycle subscriber poisoned");
            assert_eq!(state.buffer.len(), DEFAULT_CAPACITY + 2);
            assert!(
                matches!(
                    state.buffer[DEFAULT_CAPACITY],
                    Slot::One(LifecycleDelivery::Lagged { dropped: 2 })
                ),
                "the accumulated marker sits directly before the run"
            );
            assert!(matches!(
                state.buffer[DEFAULT_CAPACITY + 1],
                Slot::Backs { count: 3 }
            ));
        }
        // Drain order proves the consumer learns of the loss before the run.
        for _ in 0..DEFAULT_CAPACITY {
            assert_eq!(
                sub.try_next(),
                Some(LifecycleDelivery::Event(LifecycleEvent::Resumed))
            );
        }
        assert_eq!(
            sub.try_next(),
            Some(LifecycleDelivery::Lagged { dropped: 2 })
        );
        for _ in 0..3 {
            assert_eq!(
                sub.try_next(),
                Some(LifecycleDelivery::Event(LifecycleEvent::BackRequested))
            );
        }
        assert_eq!(sub.try_next(), None);
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
