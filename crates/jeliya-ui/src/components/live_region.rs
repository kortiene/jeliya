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

// Distinct newtypes so the two `String` signals do not collide — Dioxus keys
// context by type, so two bare `Signal<String>` providers would alias.
#[derive(Clone, Copy)]
struct ConnectionRegion(Signal<String>);
#[derive(Clone, Copy)]
struct ContentRegion(Signal<String>);

/// Provide BOTH announcement regions to a subtree and return their handles.
/// Called once by the app root; descendants read them with [`use_announce`].
pub fn use_announce_context() -> Announcers {
    let connection = use_context_provider(|| ConnectionRegion(Signal::new(String::new()))).0;
    let content = use_context_provider(|| ContentRegion(Signal::new(String::new()))).0;
    Announcers {
        connection: Announcer { signal: connection },
        content: Announcer { signal: content },
    }
}

/// The announcement handles from context. Falls back to component-local regions
/// when no provider is present (an isolated component test), so a consumer never
/// panics for lack of a root.
pub fn use_announce() -> Announcers {
    // Both fallbacks run unconditionally (Rules of Hooks); each is used only when
    // its provider is absent above this component.
    let local_connection = use_signal(String::new);
    let local_content = use_signal(String::new);
    let connection = try_use_context::<ConnectionRegion>()
        .map(|r| r.0)
        .unwrap_or(local_connection);
    let content = try_use_context::<ContentRegion>()
        .map(|r| r.0)
        .unwrap_or(local_content);
    Announcers {
        connection: Announcer { signal: connection },
        content: Announcer { signal: content },
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
