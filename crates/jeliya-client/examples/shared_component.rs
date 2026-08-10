//! The verification component for #167 (AC-6).
//!
//! A minimal shared Dioxus component compiled against the deterministic mock
//! for **both** `wasm32-unknown-unknown` and a native target, with **no
//! per-component `cfg` logic** — this file contains no `cfg` token at all. The
//! seam keeps every target difference on the host side (which adapter builds
//! the [`ClientHandle`]); the component renders [`State`] and the latest
//! [`ClientEvent`] identically regardless.
//!
//! It is built (never run) by CI on both targets:
//!
//! ```text
//! cargo build --locked -p jeliya-client --example shared_component --features example
//! cargo build --locked -p jeliya-client --example shared_component --features example \
//!     --target wasm32-unknown-unknown
//! ```
//!
//! The example is gated by `required-features = ["example"]` in `Cargo.toml`
//! (the `example` feature enables `mock` plus the optional `dioxus`), so the
//! heavy Dioxus tree only compiles in these explicit steps — never under a
//! bare `cargo check --all-targets`. Dioxus is used only for RSX and
//! `#[component]`; no renderer runs.

use dioxus::prelude::*;
use futures::StreamExt;
use jeliya_api::RoomList;
use jeliya_client::mock::MockScript;
use jeliya_client::{ClientEvent, ClientHandle, Dedup};

fn main() {
    // Build-only verification: construct the shared component against a mock
    // backend and build the virtual DOM once. No renderer runs; the point is
    // that the component type-checks and links on this target.
    let (handle, _controller) = MockScript::new().build();
    let mut dom = VirtualDom::new_with_props(RoomStatus, RoomStatusProps { handle });
    dom.rebuild_in_place();
}

/// A minimal shared component: it subscribes to the client, tracks the current
/// [`State`] and the most recent [`ClientEvent`], and issues one call on mount.
///
/// The same source compiles for native and wasm; there is no `cfg` here.
#[component]
fn RoomStatus(handle: ClientHandle) -> Element {
    let mut state = use_signal(|| handle.state());
    let mut last_event = use_signal(|| Option::<ClientEvent>::None);

    use_future(move || {
        let handle = handle.clone();
        async move {
            // Subscribe FIRST: subscriptions are live-only, so anything
            // emitted while the mount call is pending would otherwise be
            // permanently missed (the mailbox buffers events until first
            // poll). Then drive the call concurrently with consumption, so a
            // hanging call cannot keep the event loop from starting.
            let mut events = handle.subscribe();

            // One call on mount. The reply is compile-time paired with
            // `RoomList::Output`; nothing here downcasts an untyped value.
            let call = async {
                let _ = handle.room_list(RoomList {}, Dedup::None).await;
            };

            // Live events on an independent subscription. A `StateChanged`
            // updates the rendered state without polling.
            let consume = async {
                while let Some(event) = events.next().await {
                    if let ClientEvent::StateChanged { to, .. } = &event {
                        state.set(*to);
                    }
                    last_event.set(Some(event));
                }
            };

            futures::join!(call, consume);
        }
    });

    let state_label = format!("{:?}", state());
    let event_label = match last_event() {
        Some(event) => format!("{event:?}"),
        None => "none".to_string(),
    };

    rsx! {
        section {
            p { "state: {state_label}" }
            p { "event: {event_label}" }
        }
    }
}
