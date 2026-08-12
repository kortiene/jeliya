//! Live regions and the announce-once seam (§5.6).
//!
//! One **stable** polite region per channel, updated through [`Announcer`], which
//! **coalesces** consecutive identical announcements (a rebuilding list re-renders
//! many times but the string is unchanged, so it announces **once** — the exact
//! failure mode the accessibility checklist warns automation cannot hear, designed
//! out structurally) while **retaining** distinct announcements in a FIFO queue.
//! When two DISTINCT transitions are enqueued before Dioxus renders (a buffered
//! `Interrupted` then `Ready` consumed without yielding), a single latest-string
//! signal would let the recovery write clobber the interruption before the DOM
//! ever published it. The queue instead drains **one message per committed render**
//! ([`LiveRegion`]'s post-render effect), so every distinct message is exposed to
//! assistive tech for at least one frame. The region node is stable (always
//! present, only its text changes) so assistive tech tracks it rather than
//! re-discovering a new node.

use dioxus::prelude::*;
use std::collections::VecDeque;

/// Append `message` to the pending FIFO unless it repeats the last message already
/// announced or enqueued — consecutive-duplicate coalescing, so a re-rendering
/// caller still announces once. DISTINCT messages are retained in order so a later
/// one cannot clobber an earlier one before the DOM has published it. Returns
/// whether the queue changed (a coalesced duplicate must not dirty the region).
/// Pure, so the ordering/coalescing guarantee is unit-tested without a runtime.
fn push_announcement(queue: &mut VecDeque<String>, last: &mut String, message: String) -> bool {
    if *last == message {
        return false;
    }
    last.clone_from(&message);
    queue.push_back(message);
    true
}

/// A handle to one announcement region. `Copy`, so it rides in props and closures.
/// Provided once by the app root ([`use_announce_context`]) and read by descendants
/// ([`use_announce`]).
#[derive(Clone, Copy, PartialEq)]
pub struct Announcer {
    /// Pending distinct announcements; [`LiveRegion`] renders the FRONT and drains
    /// one per committed render.
    queue: Signal<VecDeque<String>>,
    /// The last message announced or enqueued, for consecutive-duplicate coalescing.
    last: Signal<String>,
}

impl Announcer {
    /// Announce `message`. Coalesces a repeat of the last message (assistive tech
    /// is not re-triggered) but appends a distinct message to the queue so it is
    /// published in turn rather than overwriting an earlier one still awaiting a
    /// render.
    pub fn announce(&self, message: impl Into<String>) {
        let message = message.into();
        // `peek` reads without subscribing, so calling `announce` from an effect
        // does not make that effect depend on its own writes.
        let mut queue = self.queue.peek().clone();
        let mut last = self.last.peek().clone();
        if push_announcement(&mut queue, &mut last, message) {
            let mut queue_sig = self.queue;
            let mut last_sig = self.last;
            queue_sig.set(queue);
            last_sig.set(last);
        }
    }

    /// The current announcement text — the FRONT of the queue (read by
    /// [`LiveRegion`]; subscribes the reader so the region re-renders when it
    /// changes). Empty when nothing is pending.
    pub fn message(&self) -> String {
        self.queue.read().front().cloned().unwrap_or_default()
    }

    /// Drop the just-published front so the next queued message becomes current on
    /// the following render; no-op when empty. Called from [`LiveRegion`]'s
    /// post-render effect.
    fn advance(&self) {
        if self.queue.peek().front().is_some() {
            let mut queue = self.queue;
            queue.write().pop_front();
        }
    }
}

/// Two INDEPENDENT stable regions — `connection` for lifecycle status and
/// `content` for room-count / content announcements — because a connection
/// announcement and a content announcement can fire in the SAME render (a
/// room-list read that lands the instant the client returns to `Ready`). A single
/// shared region would interleave the two channels' queues; two regions let both
/// be announced (§5.6).
#[derive(Clone, Copy, PartialEq)]
pub struct Announcers {
    /// Connection-lifecycle announcements (interruption / recovery).
    pub connection: Announcer,
    /// Content announcements (settled room count, terminal room-list failure).
    pub content: Announcer,
}

// Distinct newtypes so the two regions' signals do not collide — Dioxus keys
// context by type, so two bare `(Signal, Signal)` providers would alias. Each
// carries the region's pending queue and its last-announced string.
#[derive(Clone, Copy)]
struct ConnectionRegion(Signal<VecDeque<String>>, Signal<String>);
#[derive(Clone, Copy)]
struct ContentRegion(Signal<VecDeque<String>>, Signal<String>);

/// Provide BOTH announcement regions to a subtree and return their handles.
/// Called once by the app root; descendants read them with [`use_announce`].
pub fn use_announce_context() -> Announcers {
    let connection = use_context_provider(|| {
        ConnectionRegion(Signal::new(VecDeque::new()), Signal::new(String::new()))
    });
    let content = use_context_provider(|| {
        ContentRegion(Signal::new(VecDeque::new()), Signal::new(String::new()))
    });
    Announcers {
        connection: Announcer {
            queue: connection.0,
            last: connection.1,
        },
        content: Announcer {
            queue: content.0,
            last: content.1,
        },
    }
}

/// The announcement handles from context. Falls back to component-local regions
/// when no provider is present (an isolated component test), so a consumer never
/// panics for lack of a root.
pub fn use_announce() -> Announcers {
    // Both fallbacks run unconditionally (Rules of Hooks); each is used only when
    // its provider is absent above this component.
    let local_conn_queue = use_signal(VecDeque::new);
    let local_conn_last = use_signal(String::new);
    let local_content_queue = use_signal(VecDeque::new);
    let local_content_last = use_signal(String::new);
    let connection = try_use_context::<ConnectionRegion>()
        .map(|r| Announcer {
            queue: r.0,
            last: r.1,
        })
        .unwrap_or(Announcer {
            queue: local_conn_queue,
            last: local_conn_last,
        });
    let content = try_use_context::<ContentRegion>()
        .map(|r| Announcer {
            queue: r.0,
            last: r.1,
        })
        .unwrap_or(Announcer {
            queue: local_content_queue,
            last: local_content_last,
        });
    Announcers {
        connection,
        content,
    }
}

/// A stable polite live region. Rendered once PER region near the app root; its
/// text is the current [`Announcer`] message (the queue front). `aria-atomic` so
/// the whole sentence is read, not a diff. `id` distinguishes the content region
/// (`live-region`, the default) from the connection region.
#[component]
pub fn LiveRegion(
    announcer: Announcer,
    #[props(default = "live-region".to_string())] id: String,
) -> Element {
    let message = announcer.message();
    // Drain the published front AFTER this render commits, so the next queued
    // message becomes current on the FOLLOWING frame and every distinct
    // announcement is exposed to assistive tech for at least one committed render
    // (a later message cannot overwrite an earlier one before the DOM shows it).
    // Reading the queue (via `message()`) re-runs this once per publish, draining
    // one-per-frame, and it idles (no write) once the queue is empty.
    use_effect(move || {
        if !announcer.message().is_empty() {
            announcer.advance();
        }
    });
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
    use super::push_announcement;
    use std::collections::VecDeque;

    fn run(messages: &[&str]) -> (Vec<String>, String) {
        let mut queue = VecDeque::new();
        let mut last = String::new();
        for m in messages {
            push_announcement(&mut queue, &mut last, (*m).to_string());
        }
        (queue.into_iter().collect(), last)
    }

    #[test]
    fn distinct_announcements_are_retained_in_order() {
        // The finding-2 regression: a recovery must NOT clobber the interruption
        // before the DOM publishes it. Both distinct messages survive, in order —
        // a latest-string signal would keep only the last (queue length 1).
        let (queue, _) = run(&["Connection interrupted", "Reconnected"]);
        assert_eq!(
            queue,
            vec![
                "Connection interrupted".to_string(),
                "Reconnected".to_string()
            ]
        );
    }

    #[test]
    fn consecutive_duplicates_coalesce_to_one() {
        // A re-rendering caller announcing the same string repeatedly still
        // enqueues it once (the checklist's announce-once contract).
        let (queue, _) = run(&["5 rooms", "5 rooms", "5 rooms"]);
        assert_eq!(queue, vec!["5 rooms".to_string()]);
    }

    #[test]
    fn a_distinct_message_after_a_repeat_is_enqueued() {
        let (queue, last) = run(&["5 rooms", "5 rooms", "6 rooms"]);
        assert_eq!(queue, vec!["5 rooms".to_string(), "6 rooms".to_string()]);
        assert_eq!(last, "6 rooms");
    }

    #[test]
    fn a_message_repeating_the_last_enqueued_is_dropped_even_mid_queue() {
        // Coalescing is against the LAST enqueued, not the whole queue: A, B, then
        // B again drops the trailing duplicate, but A, B, A keeps the re-entry.
        let (dropped, _) = run(&["A", "B", "B"]);
        assert_eq!(dropped, vec!["A".to_string(), "B".to_string()]);
        let (kept, _) = run(&["A", "B", "A"]);
        assert_eq!(
            kept,
            vec!["A".to_string(), "B".to_string(), "A".to_string()]
        );
    }
}
