//! The composed application root (§6): target-agnostic, driven by the three
//! separately injected inputs.
//!
//! [`AppRoot`] (the spec's `app_root`) receives a
//! [`jeliya_client::ClientHandle`] and a [`crate::PlatformServices`] as
//! **separate** props (never entangled), reads typed [`jeliya_api`] view
//! models through the handle, folds lifecycle events into [`crate::UiState`],
//! and renders the shared shell using the canonical stylesheet's classes. It
//! contains **no** platform business-logic `cfg` fork: which concrete
//! `ClientHandle` and which `PlatformServices` implementation back these props
//! is decided only in [`crate::compose`].

use dioxus::prelude::*;
use futures::StreamExt;
use jeliya_api::RoomList;
use jeliya_client::{ClientHandle, Dedup, State};

use crate::components::{BootScreen, EmptyCenter, RoomListItem, StatusFooter};
use crate::services::PlatformServices;
use crate::state::UiState;

/// The application root component (the spec's `app_root`).
///
/// `handle` and `services` are injected separately. The component subscribes to
/// the client's event stream, issues one typed `room.list` read (compile-time
/// paired with [`jeliya_api::RoomListOut`]), and renders a one-pane-aware shell
/// whose class names are the ones `ui/src/styles.css` already styles.
#[component]
pub fn AppRoot(handle: ClientHandle, services: PlatformServices) -> Element {
    let mut ui = use_signal(UiState::new);

    // Record that the shell mounted through the injected services seam — a
    // small, honest demonstration that platform authority is reached only
    // through `PlatformServices`, never directly.
    use_hook(move || {
        services.set_preference("ui.mounted", "1");
    });

    use_future(move || {
        let handle = handle.clone();
        async move {
            // Subscribe FIRST: subscriptions are live-only, so anything emitted
            // while the mount read is in flight would otherwise be permanently
            // missed. Then start the client and drive the read concurrently
            // with event consumption.
            let mut events = handle.subscribe();
            // Recover the current lifecycle state at subscription time. The
            // subscription is live-only, so if the composition driver
            // (compose.rs WebRoot) already transitioned to Ready before this
            // future first polled, the StateChanged events were never buffered
            // here. Reading state() after subscribing is safe: any transition
            // that fires after subscribe() AND concurrently with this read is
            // buffered by the subscription, so there is no gap.
            ui.write().lifecycle = handle.state();
            handle.start();

            let read = {
                let handle = handle.clone();
                async move {
                    match handle.call::<RoomList>(RoomList {}, Dedup::None).await {
                        Ok(out) => ui.write().set_rooms(out.rooms),
                        Err(error) => ui.write().set_notice(format!("room.list: {error:?}")),
                    }
                }
            };

            let consume = async {
                while let Some(event) = events.next().await {
                    ui.write().apply_event(&event);
                }
            };

            futures::join!(read, consume);
        }
    });

    let snapshot = ui();
    let lifecycle = format!("{:?}", snapshot.lifecycle);
    let ready = matches!(snapshot.lifecycle, State::Ready);

    // Until Ready, the boot screen is the component ROOT (as the React shell
    // renders it), never a child of the `.app` grid: auto-placed inside the
    // two-column grid, its `height: var(--vh-full)` would blow up the first
    // `auto` row and collapse the sidebar/center instead of covering them.
    // All hooks are declared above, so the early return is order-safe.
    if !ready {
        return rsx! {
            BootScreen { target: "connecting to the local daemon…".to_string() }
        };
    }

    rsx! {
        // A root pane state is always set (`pane-rooms`), because the shared
        // stylesheet hides `.sidebar`/`.center` on compact viewports unless a
        // pane is selected — a plain `app` root renders blank on a phone
        // system WebView, which is a target platform. The React client sets
        // `app pane-${pane}`; so does this.
        div { class: "app pane-rooms", id: "app-root",
            nav { class: "sidebar", id: "sidebar",
                // On compact viewports `pane-rooms` shows ONLY the sidebar
                // (`.center` is hidden), so an empty room list must render an
                // empty state here or a phone lands on a blank main area.
                // Mirrors the React shell's `rooms-empty muted` element.
                if snapshot.rooms.is_empty() {
                    div { class: "rooms-empty muted", id: "rooms-empty", "No rooms yet" }
                }
                for room in snapshot.rooms.iter() {
                    RoomListItem {
                        key: "{room.room_id}",
                        room: room.clone(),
                        selected: false,
                    }
                }
            }
            section { class: "center", id: "center",
                if let Some(notice) = snapshot.notice.as_ref() {
                    div { class: "error-note", id: "notice", "{notice}" }
                }
                EmptyCenter {}
            }
            StatusFooter { lifecycle }
        }
    }
}
