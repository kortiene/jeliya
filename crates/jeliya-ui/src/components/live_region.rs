//! Live regions and the announce-once seam (§5.6).
//!
//! One **stable** polite region per channel, updated through [`Announcer`], which
//! **coalesces**: a rebuilding list re-renders many times but the announcement
//! string is unchanged, so the region announces **once** — the exact failure
//! mode the accessibility checklist warns automation cannot hear, designed out
//! structurally. The region node is stable (always present, only its text
//! changes) so assistive tech tracks it rather than re-discovering a new node.
//!
//! DISTINCT rapid announcements, however, must each be heard: when a connection
//! DROP is immediately followed by a RECOVERY, both `StateChanged` events can be
//! processed by the consume loop before Dioxus renders. A single latest-string
//! signal would let the recovery overwrite the drop before it reached the DOM, so
//! the drop would never be announced (#276). [`Announcer`] therefore keeps an
//! ordered backlog of distinct pending messages and advances the visible line by
//! exactly one message per committed render (a render-loop-driven drain), so every
//! distinct message occupies the live region for its own render frame while
//! consecutive identical messages still coalesce.

use dioxus::prelude::*;
use std::collections::VecDeque;

/// A handle to one announcement region. `Copy`, so it rides in props and
/// closures. Provided once by the app root ([`use_announce_context`]) and read
/// by descendants ([`use_announce`]).
///
/// Two signals form the pipeline `displayed :: pending`:
///
/// - `displayed` — the text [`LiveRegion`] currently renders (what [`message`]
///   returns). Starts empty.
/// - `pending` — DISTINCT messages waiting to reach `displayed`, in FIFO order.
///
/// The invariant `displayed :: pending` never has two equal consecutive entries
/// (enforced by [`announce`]) is what makes coalescing correct AND guarantees
/// every drain step changes the visible text (its own DOM mutation).
///
/// [`message`]: Announcer::message
/// [`announce`]: Announcer::announce
#[derive(Clone, Copy, PartialEq)]
pub struct Announcer {
    displayed: Signal<String>,
    pending: Signal<VecDeque<String>>,
}

impl Announcer {
    /// Build a fresh region: an empty visible line and an empty backlog. Used by
    /// the provider closures (each called once). The [`use_announce`] fallback
    /// builds its signals with `use_signal` instead so isolated component tests
    /// get component-local regions.
    fn new_signals() -> Self {
        Announcer {
            displayed: Signal::new(String::new()),
            pending: Signal::new(VecDeque::new()),
        }
    }

    /// Announce `message`. Coalesces against the TAIL of the pipeline — the last
    /// still-pending message, or the currently displayed text when nothing is
    /// pending: a consecutive identical announce (a re-rendering list) is dropped,
    /// which is the announce-once seam. A DISTINCT message is enqueued so it is
    /// not lost when a later announce follows before the first has rendered
    /// (#276). This preserves the `displayed :: pending` no-equal-consecutive
    /// invariant.
    pub fn announce(&self, message: impl Into<String>) {
        let message = message.into();
        // `peek` reads without subscribing, so calling `announce` from an effect
        // does not make that effect depend on its own writes.
        let tail_is_message = {
            let pending = self.pending.peek();
            match pending.back() {
                Some(last) => *last == message,
                None => *self.displayed.peek() == message,
            }
        };
        if tail_is_message {
            return;
        }
        let mut pending = self.pending;
        pending.write().push_back(message);
    }

    /// Advance the region by ONE pending message. Reads `pending` (SUBSCRIBES) so
    /// the drain effect re-runs when [`announce`](Announcer::announce) enqueues;
    /// writing `displayed` dirties the scope that reads [`message`](Announcer::message),
    /// which forces a render + DOM commit BEFORE this effect can run again (Dioxus
    /// runs effects at lowest priority and returns from the task pump as soon as an
    /// effect dirties a scope). So each pending message reaches the DOM in its own
    /// render. When nothing is pending this writes no signal, so the effect chain
    /// is self-terminating — never a busy loop.
    fn drain_one(&self) {
        // read -> subscribe, so the effect wakes on the next enqueue.
        if self.pending.read().is_empty() {
            return;
        }
        let mut pending = self.pending;
        // Bind the pop first so the `write()` lock is released at the semicolon,
        // before `displayed` is written (never hold two signal guards at once).
        let next = pending.write().pop_front();
        if let Some(next) = next {
            let mut displayed = self.displayed;
            displayed.set(next);
        }
    }

    /// The current announcement text (read by [`LiveRegion`]; subscribes the
    /// reader so the region re-renders when it changes).
    pub fn message(&self) -> String {
        self.displayed.read().clone()
    }
}

/// Two INDEPENDENT stable regions — `connection` for lifecycle status and
/// `content` for room-count / content announcements — because a connection
/// announcement and a content announcement can fire in the SAME render (a
/// room-list read that lands the instant the client returns to `Ready`). A single
/// latest-string signal would have the second write clobber the first before
/// assistive tech observed it; two regions let both be announced (§5.6).
#[derive(Clone, Copy, PartialEq)]
pub struct Announcers {
    /// Connection-lifecycle announcements (interruption / recovery).
    pub connection: Announcer,
    /// Content announcements (settled room count, terminal room-list failure).
    pub content: Announcer,
}

// Distinct newtypes so the two regions do not collide — Dioxus keys context by
// type, so two bare `Announcer` providers would alias.
#[derive(Clone, Copy)]
struct ConnectionRegion(Announcer);
#[derive(Clone, Copy)]
struct ContentRegion(Announcer);

/// Provide BOTH announcement regions to a subtree and return their handles.
/// Called once by the app root; descendants read them with [`use_announce`].
pub fn use_announce_context() -> Announcers {
    let connection = use_context_provider(|| ConnectionRegion(Announcer::new_signals())).0;
    let content = use_context_provider(|| ContentRegion(Announcer::new_signals())).0;
    let announcers = Announcers {
        connection,
        content,
    };
    // Render-loop-driven backlog drains (one per region, registered
    // unconditionally in stable order — Rules of Hooks): each re-runs when its
    // region enqueues, advances the visible line by exactly one message, and (by
    // writing `displayed`) forces a render/DOM commit before it can run again, so
    // every distinct queued announcement occupies the live region for its own
    // render (#276). Self-terminating: when `pending` empties the effect writes
    // nothing and parks on the `pending` subscription — no busy loop, no timer.
    use_effect(move || announcers.connection.drain_one());
    use_effect(move || announcers.content.drain_one());
    announcers
}

/// The announcement handles from context. Falls back to component-local regions
/// when no provider is present (an isolated component test), so a consumer never
/// panics for lack of a root.
pub fn use_announce() -> Announcers {
    // Both fallbacks run unconditionally (Rules of Hooks); each is used only when
    // its provider is absent above this component.
    let local_connection = Announcer {
        displayed: use_signal(String::new),
        pending: use_signal(VecDeque::new),
    };
    let local_content = Announcer {
        displayed: use_signal(String::new),
        pending: use_signal(VecDeque::new),
    };
    // Drain the local fallback regions too (unconditionally), so a component that
    // renders its own `LiveRegion` and announces still advances. Harmless no-ops
    // when a provider is present: the locals never enqueue, so the effects park on
    // an empty backlog.
    use_effect(move || local_connection.drain_one());
    use_effect(move || local_content.drain_one());
    let connection = try_use_context::<ConnectionRegion>()
        .map(|r| r.0)
        .unwrap_or(local_connection);
    let content = try_use_context::<ContentRegion>()
        .map(|r| r.0)
        .unwrap_or(local_content);
    Announcers {
        connection,
        content,
    }
}

/// A stable polite live region. Rendered once PER region near the app root; its
/// text is the current [`Announcer`] message. `aria-atomic` so the whole
/// sentence is read, not a diff. `id` distinguishes the content region
/// (`live-region`, the default) from the connection region.
#[component]
pub fn LiveRegion(
    message: String,
    #[props(default = "live-region".to_string())] id: String,
) -> Element {
    rsx! {
        div {
            class: "visually-hidden",
            id: "{id}",
            role: "status",
            "aria-live": "polite",
            "aria-atomic": "true",
            "{message}"
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    /// Mirror of [`Announcer::announce`] applied to plain `(displayed, pending)` state.
    /// Coalesces against the TAIL of `pending`, or against `displayed` when `pending`
    /// is empty — identical to the signal-based implementation.
    fn announce(displayed: &str, pending: &mut VecDeque<String>, msg: &str) {
        let tail_is_msg = match pending.back() {
            Some(last) => last == msg,
            None => displayed == msg,
        };
        if !tail_is_msg {
            pending.push_back(msg.to_string());
        }
    }

    /// Mirror of [`Announcer::drain_one`]. Pops one message from the front of
    /// `pending` into `displayed`. Returns `true` when a message was drained,
    /// `false` when `pending` was empty (the self-terminating no-op case).
    fn drain_one(displayed: &mut String, pending: &mut VecDeque<String>) -> bool {
        match pending.pop_front() {
            Some(next) => {
                *displayed = next;
                true
            }
            None => false,
        }
    }

    // --- #276 regression: DROP followed immediately by RECOVERY ---

    /// When `Interrupted` and `Ready` events are processed by the consume loop
    /// before Dioxus renders, both announcements must be enqueued — neither
    /// overwrites the other. The old single-signal design would let the second
    /// `announce` clobber the first, so the drop was never heard.
    #[test]
    fn drop_then_recovery_without_intervening_render_both_enqueue() {
        let displayed = String::new();
        let mut pending = VecDeque::new();
        announce(&displayed, &mut pending, "Reconnecting\u{2026}");
        announce(&displayed, &mut pending, "Connected");
        assert_eq!(
            pending.len(),
            2,
            "#276: both distinct messages must be queued, not just the latest"
        );
        assert_eq!(pending[0], "Reconnecting\u{2026}", "drop is first in queue");
        assert_eq!(pending[1], "Connected", "recovery is second in queue");
    }

    /// Each queued message must reach the DOM in its own drain step (its own
    /// render frame). After the first `drain_one` the drop is visible and the
    /// recovery is still pending; after the second both are consumed.
    #[test]
    fn drop_then_recovery_drain_each_in_own_frame() {
        let mut displayed = String::new();
        let mut pending = VecDeque::new();
        announce(&displayed, &mut pending, "Reconnecting\u{2026}");
        announce(&displayed, &mut pending, "Connected");

        assert!(
            drain_one(&mut displayed, &mut pending),
            "first drain must succeed"
        );
        assert_eq!(
            displayed, "Reconnecting\u{2026}",
            "drop must be surfaced first"
        );
        assert_eq!(
            pending.len(),
            1,
            "recovery must remain pending after first drain"
        );

        assert!(
            drain_one(&mut displayed, &mut pending),
            "second drain must succeed"
        );
        assert_eq!(
            displayed, "Connected",
            "recovery must be surfaced on second drain"
        );
        assert!(
            pending.is_empty(),
            "queue must be empty after both messages are drained"
        );
    }

    // --- Announce-once coalescing seam (room-count regression protection) ---

    /// A rebuilding list re-renders many times but `announce` is called with the
    /// same string repeatedly. All but the first call must be no-ops: only one
    /// entry enters `pending` and the region announces exactly once.
    #[test]
    fn repeated_identical_announces_coalesce_to_one_pending_entry() {
        let displayed = String::new();
        let mut pending = VecDeque::new();
        let msg = "3 rooms";
        for _ in 0..5 {
            announce(&displayed, &mut pending, msg);
        }
        assert_eq!(
            pending.len(),
            1,
            "five identical announces must coalesce to a single pending entry"
        );
        assert_eq!(pending[0], msg);
    }

    /// After `drain_one` advances `displayed`, a re-announce of the same text
    /// (e.g. the room-list re-renders) must coalesce against `displayed` when
    /// `pending` is empty — no duplicate announcement on the next render.
    #[test]
    fn announce_coalesces_against_displayed_text_when_pending_is_empty() {
        let mut displayed = String::new();
        let mut pending = VecDeque::new();
        announce(&displayed, &mut pending, "3 rooms");
        drain_one(&mut displayed, &mut pending);
        assert_eq!(displayed, "3 rooms");
        announce(&displayed, &mut pending, "3 rooms");
        assert!(
            pending.is_empty(),
            "re-announce of currently displayed text must coalesce when nothing is pending"
        );
    }

    // --- Self-terminating drain (no busy loop) ---

    /// `drain_one` must be a strict no-op — not writing `displayed` — when
    /// `pending` is empty. Writing the same value would still dirty the signal
    /// and re-trigger the effect, creating a busy loop.
    #[test]
    fn drain_one_on_empty_pending_is_a_no_op() {
        let mut displayed = "Connected".to_string();
        let mut pending: VecDeque<String> = VecDeque::new();
        let drained = drain_one(&mut displayed, &mut pending);
        assert!(!drained, "drain on empty pending must report no drain");
        assert_eq!(
            displayed, "Connected",
            "displayed must be unchanged when nothing is pending"
        );
    }

    // --- Tail-comparison semantics ---

    /// The coalescing comparison is against the TAIL of `pending`, not the head.
    /// A,B,B must yield [A,B], not [A,B,B] — the second B is identical to the tail.
    #[test]
    fn consecutive_duplicate_at_tail_is_dropped() {
        let displayed = String::new();
        let mut pending = VecDeque::new();
        announce(&displayed, &mut pending, "Reconnecting\u{2026}");
        announce(&displayed, &mut pending, "Connected");
        announce(&displayed, &mut pending, "Connected"); // duplicate of tail
        assert_eq!(
            pending.len(),
            2,
            "second 'Connected' must be dropped as a duplicate of the tail"
        );
        assert_eq!(pending[1], "Connected");
    }

    /// A message identical to a non-tail entry but NOT the tail is still distinct
    /// and must be enqueued (only the immediate tail is compared).
    #[test]
    fn non_consecutive_identical_message_enqueues_when_not_at_tail() {
        let displayed = String::new();
        let mut pending = VecDeque::new();
        announce(&displayed, &mut pending, "Reconnecting\u{2026}");
        announce(&displayed, &mut pending, "Connected");
        announce(&displayed, &mut pending, "Reconnecting\u{2026}"); // same as [0] but not the tail
        assert_eq!(
            pending.len(),
            3,
            "non-tail duplicate must enqueue as a distinct message"
        );
    }

    // --- Full connection-cycle pipeline (#276 scenario end-to-end) ---

    /// Simulate the exact #276 scenario: two `StateChanged` events processed by
    /// the consume loop before Dioxus renders, then two separate render-driven
    /// drains — each message occupies the live region for its own frame.
    #[test]
    fn connection_cycle_drop_recovery_full_pipeline() {
        let mut displayed = String::new();
        let mut pending = VecDeque::new();

        // Consume loop: Interrupted then Ready, no render in between.
        announce(&displayed, &mut pending, "Reconnecting\u{2026}");
        announce(&displayed, &mut pending, "Connected");
        assert_eq!(pending.len(), 2, "both events queued before any render");

        // Render frame 1: drain_one → DOM shows drop, recovery still pending.
        drain_one(&mut displayed, &mut pending);
        assert_eq!(displayed, "Reconnecting\u{2026}");
        assert_eq!(pending.len(), 1);

        // Re-announce of drop is now a no-op (tail matches displayed after drain,
        // but pending is non-empty with "Connected" so tail is "Connected" — the
        // drop string is NOT the tail here, but we're testing no duplicate enqueue
        // of the drop at this point).

        // Render frame 2: drain_one → DOM shows recovery, queue empty.
        drain_one(&mut displayed, &mut pending);
        assert_eq!(displayed, "Connected");
        assert!(
            pending.is_empty(),
            "both messages consumed across two render frames"
        );

        // Further announces of "Connected" coalesce (a re-render after settling).
        announce(&displayed, &mut pending, "Connected");
        assert!(
            pending.is_empty(),
            "post-settle re-announce coalesces against displayed"
        );
    }
}
