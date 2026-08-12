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
//!
//! #177 makes localization and accessibility structural: the root **provides**
//! the resolved-locale and live-region contexts (resolving the two persisted
//! locale preferences through the injected [`Preferences`](jeliya_platform::Preferences)
//! capability), renders a properly landmarked page (skip links, a named `nav`,
//! a `main` with one `h1`, a stable live region), and routes the raw failure
//! detail into the Diagnostics dialog while primary copy stays friendly catalog
//! text.

use dioxus::prelude::*;
use futures::StreamExt;
use jeliya_api::RoomList;
use jeliya_client::{CallError, ClientEvent, ClientHandle, Dedup, State};
use jeliya_platform::PreferenceKey;

use crate::components::{
    use_announce_context, BootScreen, EmptyCenter, LiveRegion, MainRegion, NavLandmark,
    RoomListItem, SkipLink, SkipLinks, StatusFooter,
};
use crate::l10n::{
    catalog_for, plural_category, use_locale_context, use_strings, ErrorDisplay, Formats,
    LocaleState,
};
use crate::state::UiState;
use crate::PlatformServices;

/// The application root component (the spec's `app_root`).
///
/// `handle` and `services` are injected separately. The component subscribes to
/// the client's event stream, issues one typed `room.list` read (compile-time
/// paired with [`jeliya_api::RoomListOut`]), and renders a one-pane-aware shell
/// whose class names are the ones `ui/src/styles.css` already styles.
#[component]
pub fn AppRoot(
    handle: ClientHandle,
    services: PlatformServices,
    /// The platform's UI language tag (browser `navigator.language` / OS
    /// locale), injected at composition (`compose.rs`) — never read from
    /// `web-sys`/`cfg` in this shared component. `None` when the platform
    /// exposes none. It is the second input to locale resolution after the
    /// persisted preferences, so a fresh French-browser user reaches the French
    /// catalog with no stored preference (Decision-5).
    #[props(default)]
    platform_locale: Option<String>,
) -> Element {
    let mut ui = use_signal(UiState::new);

    // Resolve the locale from the two persisted preferences AND the injected
    // platform language, in that precedence (Decision-5, `LocaleState::resolve`):
    // an explicit stored preference wins, else the platform language, else the
    // fallback. Reading the persisted prefs goes through the injected
    // `Preferences` capability (#174) — the platform-authority boundary a shared
    // component may read, never `localStorage`/`cfg` directly (Decision-3). The
    // platform language arrives as a prop from composition (never `web-sys` here),
    // so a fresh French-browser user with no stored preference reaches the French
    // catalog. The concrete storage keys are the platform contract's
    // `TextLocale` / `FormattingLocale` (#178 owns their browser namespace).
    let preferences = services.preferences();
    let text_pref = preferences.get(&PreferenceKey::TextLocale);
    let formatting_pref = preferences.get(&PreferenceKey::FormattingLocale);
    let initial_locale = LocaleState::resolve(
        text_pref.as_deref(),
        formatting_pref.as_deref(),
        platform_locale.as_deref(),
        platform_locale.as_deref(),
    );

    // Provide the resolved-locale and live-region contexts to the whole subtree.
    // `use_locale_context` returns the switch signal (a settings surface, a later
    // slice, assigns it to change locale live); the foundation proves the wiring
    // and the per-render resolution that makes a switch apply with no restart.
    let locale = use_locale_context(initial_locale);
    let announcer = use_announce_context();

    // `<html lang>` is set from the resolved text locale at composition
    // (`compose::apply_document_lang`, web-sys, web target only), so assistive
    // tech reads the page in its actual language rather than the static `en` in
    // index.html (§5.1). It is set there, not here, to keep this shared
    // component free of `web-sys`/`cfg`; a reactive update on a live locale
    // switch rides with that later slice (there is no switch UI in this
    // foundation yet).

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
                                state.clear_notice();
                                return;
                            }
                            // An accepted call can still die mid-flight when
                            // the transport drops (Ready → Interrupted before
                            // the reply). room.list is a pure idempotent
                            // read, so a Disconnected verdict retries after
                            // the next recovery to Ready; every other error
                            // is a genuine reply or refusal and is recorded
                            // once — retrying those could loop forever on a
                            // persistent failure. The recorded notice is the
                            // RAW, secret-scrubbed detail (the Diagnostics
                            // dialog's content); primary copy stays friendly.
                            Err(error @ CallError::Disconnected { .. }) => {
                                ui.write().set_notice(diagnostic_notice(&error));
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
                                // TERMINAL: this task will not retry, so the
                                // notice is recorded as terminal and the shell
                                // shows copy that does not promise a recovery
                                // that will never come.
                                state.set_terminal_notice(diagnostic_notice(&error));
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

    // Announce the loaded room count through the single stable live region, once
    // per change: the effect re-runs only when the room list or the locale
    // changes, and `Announcer::announce` coalesces, so a list that re-renders
    // many times still announces once (the checklist's failure mode, designed
    // out structurally — §5.6).
    use_effect(move || {
        let snapshot = ui.read();
        let resolved = locale.read();
        if snapshot.rooms_loaded {
            let count = snapshot.rooms.len() as u64;
            let formatted = Formats::new(resolved.text, resolved.formatting).count(count);
            let category = plural_category(resolved.text, count);
            let message = catalog_for(resolved.text).rooms_count(&formatted, category);
            announcer.announce(message);
        }
    });

    // Announce a connection PROBLEM or a RECOVERY from one through the same
    // polite region, so a screen-reader user not on the footer still hears it —
    // `StatusIndicator` has no live semantics (§5.6). The happy boot path
    // (`Idle`/`Connecting` → `Ready`) is deliberately NOT announced: reaching
    // Ready for the first time is not a change worth interrupting a user for,
    // and announcing it would fight the room-count announcement for the one
    // region. Only a drop to Interrupted/Failed/Stopped, or a return to Ready
    // AFTER such a drop, is announced. `Announcer` coalesces, so a state that
    // re-renders many times still announces once.
    let mut prev_lifecycle = use_signal(|| Option::<State>::None);
    use_effect(move || {
        let state = ui.read().lifecycle;
        let resolved = locale.read();
        // `peek`, not a subscribing read: this effect WRITES `prev_lifecycle`,
        // and subscribing to a signal it also writes would re-trigger itself
        // forever (an infinite render loop that hangs the app). It must re-run
        // only when the lifecycle or locale changes.
        let previous = *prev_lifecycle.peek();
        prev_lifecycle.set(Some(state));
        let is_problem = matches!(state, State::Interrupted | State::Failed | State::Stopped);
        let recovered = state == State::Ready
            && matches!(
                previous,
                Some(State::Interrupted | State::Failed | State::Stopped)
            );
        if is_problem || recovered {
            let word = crate::l10n::wire::status_for(catalog_for(resolved.text), state);
            announcer.announce(catalog_for(resolved.text).conn_announcement(word));
        }
    });

    let strings = use_strings();
    let snapshot = ui();

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
    // recovers from. Labels are catalog copy, so the cover speaks the resolved
    // locale.
    let boot_target = match snapshot.lifecycle {
        State::Ready | State::Interrupted => None,
        State::Idle | State::Connecting => Some(strings.boot_connecting()),
        State::Stopping => Some(strings.boot_stopping()),
        State::Stopped => Some(strings.boot_stopped()),
        State::Failed => Some(strings.boot_failed()),
    };
    if let Some(target) = boot_target {
        return rsx! {
            BootScreen {
                target: target.to_string(),
                notice: snapshot.notice.clone(),
            }
        };
    }

    // Primary copy for a failed room-list read is friendly catalog text; the raw
    // detail lives only in the Diagnostics dialog (carried via `StatusFooter`).
    // Terminal failures get copy that does not promise a retry that will never
    // happen (§5.8 / the "no false recovery promise" rule).
    let room_error = snapshot
        .notice
        .as_ref()
        .map(|_| ErrorDisplay::room_list_failure(strings, snapshot.notice_terminal).message);
    let rooms_label = strings.rooms_heading().to_string();
    let skip_rooms = strings.skip_to_rooms().to_string();
    let app_name = strings.app_name();
    let rooms_empty = strings.rooms_empty();
    let rooms_loading = strings.rooms_loading();

    rsx! {
        // Skip links are the FIRST focusable region on the page and move focus
        // (not just scroll) to their `tabindex="-1"` landmark targets. Only
        // "skip to rooms" is offered: the rooms list is the foundation's one
        // meaningful content region and is visible on every viewport. A
        // "skip to content" link is deliberately NOT offered — the center is an
        // empty placeholder here AND `pane-rooms` hides it on compact, so its
        // target would be an unfocusable `display:none` node. The Room Workbench
        // port adds that link with the real content it points at.
        SkipLinks {
            SkipLink { anchor: "rooms-nav".to_string(), label: skip_rooms }
        }
        // A root pane state is always set (`pane-rooms`), because the shared
        // stylesheet hides `.sidebar`/`.center` on compact viewports unless a
        // pane is selected — a plain `app` root renders blank on a phone
        // system WebView, which is a target platform. The React client sets
        // `app pane-${pane}`; so does this.
        div { class: "app pane-rooms", id: "app-root",
            // The page's single `<h1>`, at the always-rendered root (never a
            // pane-hidden region), so EVERY viewport — including compact, where
            // the `.center` main is `display:none` — exposes exactly one h1 in
            // the accessibility tree. Visually hidden because the visible
            // section headings (the nav's accessible name, the centre's h2)
            // already show on screen; the h1 names the page for assistive tech.
            h1 { class: "visually-hidden", "{app_name}" }
            // The sidebar is a NAMED navigation landmark, so landmark
            // navigation can distinguish it from the main region and a skip
            // link can move focus into it.
            NavLandmark {
                class: "sidebar".to_string(),
                id: "rooms-nav".to_string(),
                label: rooms_label,
                // The notice lives in the SIDEBAR, not `.center`: on compact
                // viewports `pane-rooms` hides `.center` entirely, and this
                // slice's shell is fixed at `pane-rooms`. Primary copy is the
                // friendly message; the raw detail is in Diagnostics.
                if let Some(message) = room_error.as_ref() {
                    div { class: "error-note", id: "notice", "{message}" }
                }
                // `.rooms-list` is the scroll container the stylesheet
                // styles (flex: 1, overflow-y: auto, min-height: 0). Rows as
                // direct children of `.sidebar` would compress or clip once
                // the list outgrows the viewport. Mirrors the React shell.
                div { class: "rooms-list", id: "rooms-list",
                    // On compact viewports `pane-rooms` shows ONLY the
                    // sidebar, so an empty room list must render an empty
                    // state here or a phone lands on a blank main area.
                    if snapshot.rooms.is_empty() {
                        // "No rooms yet" is an ANSWER, not a default: before
                        // the first room.list reply lands, an empty vector
                        // means "not answered yet", and claiming an empty
                        // account during a slow read would be false.
                        if snapshot.rooms_loaded {
                            div { class: "rooms-empty muted", id: "rooms-empty", "{rooms_empty}" }
                        } else {
                            div { class: "rooms-empty muted", id: "rooms-loading", "{rooms_loading}" }
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
                // The footer sits at the BOTTOM of the sidebar's flex column,
                // reporting the connection state accessibly and hosting the
                // Diagnostics disclosure that carries the raw failure detail.
                StatusFooter { state: snapshot.lifecycle, detail: snapshot.notice.clone() }
            }
            // The `<main>` landmark. Visible on desktop; `pane-rooms` hides it on
            // compact (a known limitation of this fixed-pane foundation shell —
            // the pane-navigation slice exposes per-pane main content). It
            // carries the centre's own h2, under the root h1.
            MainRegion { id: "main-content".to_string(), EmptyCenter {} }
            // The single, stable polite live region for connection/content
            // announcements. Visually hidden, so it does not disturb layout.
            LiveRegion { message: announcer.message() }
        }
    }
}

/// The raw, secret-scrubbed detail recorded for the Diagnostics dialog. Keeps
/// the `room.list:` context an operator needs while guaranteeing no token-shaped
/// value reaches the recorded string (§5.8).
fn diagnostic_notice(error: &CallError) -> String {
    format!("room.list: {}", ErrorDisplay::diagnostic_detail(error))
}
