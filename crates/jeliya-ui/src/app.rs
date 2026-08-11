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
use jeliya_client::{CallError, ClientEvent, ClientHandle, Dedup, State};

use crate::components::{BootScreen, EmptyCenter, RoomListItem, StatusFooter};
use crate::state::UiState;
use crate::PlatformServices;

/// The application root component (the spec's `app_root`).
///
/// `handle` and `services` are injected separately. The component subscribes to
/// the client's event stream, issues one typed `room.list` read (compile-time
/// paired with [`jeliya_api::RoomListOut`]), and renders a one-pane-aware shell
/// whose class names are the ones `ui/src/styles.css` already styles.
#[component]
pub fn AppRoot(handle: ClientHandle, services: PlatformServices) -> Element {
    let mut ui = use_signal(UiState::new);

    // Touch the injected services seam once on mount — a small, honest
    // demonstration that platform authority is reached only through
    // `PlatformServices`, never directly. Reading the storage durability is a
    // side-effect-free platform fact through the canonical contract (#174).
    use_hook(move || services.preferences().durability());

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
                    // The seam accepts calls only in `Ready` (`event.rs`
                    // §State: Connecting is "not yet usable"), and this
                    // component never retries — so a read dispatched while a
                    // real adapter is still Connecting would be refused and
                    // leave the room list empty for the session. The mock
                    // masks that by accepting calls in every non-stopping
                    // state. Gate the sole mount read on Ready: a dedicated
                    // subscription taken BEFORE the first state check (so a
                    // transition between the two is buffered, not missed),
                    // and the CURRENT state — not the observed event — is
                    // what authorizes dispatch: a buffered Ready followed by
                    // a buffered Interrupted must keep waiting for the next
                    // Ready, not fire on the stale event. Every transition
                    // emits a control `StateChanged` (never dropped), so
                    // re-checking `state()` per event cannot miss Ready. If
                    // the stream ends first (stopped or failed before ever
                    // Ready), there is nothing to read.
                    let mut ready = handle.subscribe();
                    loop {
                        while handle.state() != State::Ready {
                            if ready.next().await.is_none() {
                                return;
                            }
                        }
                        match handle.call::<RoomList>(RoomList {}, Dedup::None).await {
                            Ok(out) => {
                                let mut state = ui.write();
                                state.set_rooms(out.rooms);
                                // A retry that succeeds must clear the
                                // transient disconnect notice the failed
                                // attempt recorded, or the recovered shell
                                // shows a stale connection-loss error next
                                // to successfully loaded data forever.
                                state.notice = None;
                                return;
                            }
                            // An accepted call can still die mid-flight when
                            // the transport drops (Ready → Interrupted before
                            // the reply). room.list is a pure idempotent
                            // read, so a Disconnected verdict retries after
                            // the next recovery to Ready; every other error
                            // is a genuine reply or refusal and is recorded
                            // once — retrying those could loop forever on a
                            // persistent failure.
                            Err(error @ CallError::Disconnected { .. }) => {
                                ui.write().set_notice(format!("room.list: {error:?}"));
                                // A Disconnected settlement can outrun the
                                // lifecycle event — the seam permits settling
                                // pending calls before publishing
                                // Interrupted — so "state still Ready" does
                                // not mean the connection recovered, and an
                                // immediate retry against the dying
                                // connection could be refused with a
                                // non-retryable error, ending this task
                                // before recovery. Prove leave-and-re-enter
                                // from evidence dated AFTER this failure: a
                                // FRESH subscription (the long-lived one may
                                // hold pre-dispatch transitions that would
                                // impersonate the recovery) plus the current
                                // state as the leave witness when the
                                // Interrupted event outran the subscribe.
                                // Residual honesty: a full leave-and-re-enter
                                // completing entirely between the settlement
                                // and this subscribe is indistinguishable
                                // without kernel sequencing (#270's problem)
                                // and parks this task until the next
                                // transition — quiescent, never a busy loop.
                                let mut recovery = handle.subscribe();
                                let mut left_ready = handle.state() != State::Ready;
                                loop {
                                    if left_ready && handle.state() == State::Ready {
                                        break;
                                    }
                                    match recovery.next().await {
                                        Some(ClientEvent::StateChanged { to, .. }) => {
                                            if to != State::Ready {
                                                left_ready = true;
                                            } else if left_ready {
                                                break;
                                            }
                                        }
                                        Some(_) => continue,
                                        None => return,
                                    }
                                }
                            }
                            Err(error) => {
                                let mut state = ui.write();
                                state.set_notice(format!("room.list: {error:?}"));
                                // The read has ANSWERED — with a terminal
                                // error this task will not retry — so the
                                // loading state must end: leaving
                                // rooms_loaded false would show
                                // "Loading rooms…" forever next to the error
                                // notice with nothing in flight.
                                state.rooms_loaded = true;
                                return;
                            }
                        }
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

    // Outside the shell, the boot screen is the component ROOT (as the React
    // shell renders it), never a child of the `.app` grid: auto-placed inside
    // the two-column grid, its `height: var(--vh-full)` would blow up the
    // first `auto` row and collapse the sidebar/center instead of covering
    // them. All hooks are declared above, so the early return is order-safe.
    //
    // The "connecting" cover is reserved for initial activation. `Interrupted`
    // was Ready and is recovering, so the shell stays mounted (`StatusFooter`
    // reports the state) rather than hiding the rooms behind a boot screen;
    // the stop and failure states render their own honest label — a terminal
    // state that claims to be connecting would be a lie the client never
    // recovers from — plus any recorded notice, which the Ready-only shell
    // would otherwise swallow.
    let boot_target = match snapshot.lifecycle {
        State::Ready | State::Interrupted => None,
        State::Idle | State::Connecting => Some("connecting to the local daemon…"),
        State::Stopping => Some("stopping — draining accepted work…"),
        State::Stopped => Some("stopped"),
        State::Failed => Some("the client failed and will not retry"),
    };
    if let Some(target) = boot_target {
        return rsx! {
            BootScreen {
                target: target.to_string(),
                notice: snapshot.notice.clone(),
            }
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
                // The notice lives in the SIDEBAR, not `.center`: on compact
                // viewports `pane-rooms` hides `.center` entirely, and this
                // slice's shell is fixed at `pane-rooms` — a notice rendered
                // there would leave a phone showing a misleading "No rooms
                // yet" while the one diagnostic that explains it is
                // unreachable. The sidebar is the region visible in every
                // pane layout this shell can produce.
                if let Some(notice) = snapshot.notice.as_ref() {
                    div { class: "error-note", id: "notice", "{notice}" }
                }
                // `.rooms-list` is the scroll container the stylesheet
                // styles (flex: 1, overflow-y: auto, min-height: 0). Rows as
                // direct children of `.sidebar` would compress or clip once
                // the list outgrows the viewport — desktop `.sidebar` does
                // not scroll. Mirrors the React shell's nested container.
                div { class: "rooms-list", id: "rooms-list",
                    // On compact viewports `pane-rooms` shows ONLY the
                    // sidebar (`.center` is hidden), so an empty room list
                    // must render an empty state here or a phone lands on a
                    // blank main area. Mirrors the React shell's
                    // `rooms-empty muted` element.
                    if snapshot.rooms.is_empty() {
                        // "No rooms yet" is an ANSWER, not a default: before
                        // the first room.list reply lands, an empty vector
                        // means "not answered yet", and claiming an empty
                        // account during a slow read would be false.
                        if snapshot.rooms_loaded {
                            div { class: "rooms-empty muted", id: "rooms-empty", "No rooms yet" }
                        } else {
                            div { class: "rooms-empty muted", id: "rooms-loading", "Loading rooms…" }
                        }
                    }
                    for room in snapshot.rooms.iter() {
                        RoomListItem {
                            key: "{room.room_id}",
                            room: room.clone(),
                            selected: false,
                        }
                    }
                }
                // The footer sits at the BOTTOM of the sidebar's flex
                // column (after the flex-1 rooms list), not as a loose
                // `.app` child: the grid auto-places an unpositioned child
                // into row 1 — the full-width strip the stylesheet reserves
                // for `.conn-region` — which would render the footer above
                // the sidebar and open an empty band over the center. The
                // sidebar is also the one pane visible in every layout this
                // shell produces, which is the point of the state label.
                StatusFooter { lifecycle }
            }
            section { class: "center", id: "center", EmptyCenter {} }
        }
    }
}
