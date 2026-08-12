//! Live regions and the announce-once seam (§5.6).
//!
//! One **stable** polite region, updated through [`Announcer`], which
//! **coalesces**: a rebuilding list re-renders many times but the announcement
//! string is unchanged, so the region announces **once** — the exact failure
//! mode the accessibility checklist warns automation cannot hear, designed out
//! structurally. The region node is stable (always present, only its text
//! changes) so assistive tech tracks it rather than re-discovering a new node.

use dioxus::prelude::*;

/// A handle to the single announcement region. `Copy`, so it rides in props and
/// closures. Provided once by the app root ([`use_announce_context`]) and read
/// by descendants ([`use_announce`]).
#[derive(Clone, Copy, PartialEq)]
pub struct Announcer {
    signal: Signal<String>,
}

impl Announcer {
    /// Announce `message`, coalescing: if the region already says exactly this,
    /// nothing changes and assistive tech is not re-triggered. This is what
    /// makes a re-rendering caller announce once rather than per render.
    pub fn announce(&self, message: impl Into<String>) {
        let message = message.into();
        // `peek` reads without subscribing, so calling `announce` from an effect
        // does not make that effect depend on its own writes.
        if *self.signal.peek() != message {
            let mut signal = self.signal;
            signal.set(message);
        }
    }

    /// The current announcement text (read by [`LiveRegion`]; subscribes the
    /// reader so the region re-renders when it changes).
    pub fn message(&self) -> String {
        self.signal.read().clone()
    }
}

/// Provide the announcement context to a subtree and return the handle. Called
/// once by the app root; descendants read it with [`use_announce`].
pub fn use_announce_context() -> Announcer {
    let signal = use_context_provider(|| Signal::new(String::new()));
    Announcer { signal }
}

/// The announcement handle from context. Falls back to a component-local region
/// when no provider is present (an isolated component test), so a consumer never
/// panics for lack of a root.
pub fn use_announce() -> Announcer {
    // Both hooks run unconditionally (Rules of Hooks); the local signal is the
    // fallback used only when no provider is above this component.
    let local = use_signal(String::new);
    match try_use_context::<Signal<String>>() {
        Some(signal) => Announcer { signal },
        None => Announcer { signal: local },
    }
}

/// The single, stable polite live region. Rendered once near the app root; its
/// text is the current [`Announcer`] message. `aria-atomic` so the whole
/// sentence is read, not a diff.
#[component]
pub fn LiveRegion(message: String) -> Element {
    rsx! {
        div {
            class: "visually-hidden",
            id: "live-region",
            role: "status",
            "aria-live": "polite",
            "aria-atomic": "true",
            "{message}"
        }
    }
}
