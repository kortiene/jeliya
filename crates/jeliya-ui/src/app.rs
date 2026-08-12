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
    use_announce_context, BootScreen, EmptyCenter, LiveRegion, RoomListItem, SkipLink, SkipLinks,
    StatusFooter,
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
    /// Injected side effect that applies the resolved TEXT-locale BCP-47 tag to
    /// the document (`<html lang>`), called reactively whenever the resolved
    /// locale changes. The web target passes a `web-sys` setter here; other
    /// targets pass `None`. Kept as an injected callback so this shared component
    /// stays free of `web-sys`/`cfg` (Decision-3).
    #[props(default)]
    on_locale_lang: Option<Callback<String>>,
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
    let announcers = use_announce_context();

    // `<html lang>` tracks the resolved TEXT locale REACTIVELY: the injected
    // `on_locale_lang` callback (a `web-sys` setter on the web target; `None`
    // elsewhere) is called on mount AND whenever the locale signal changes, so a
    // live locale switch updates the document language too — assistive tech reads
    // the page in its actual language rather than index.html's static `en` (§5.1).
    // The side effect is INJECTED so this shared component stays free of
    // `web-sys`/`cfg` (Decision-3); the switch UI is a later slice, the wiring is
    // proven now.
    use_effect(move || {
        let tag = locale.read().text.tag().to_string();
        if let Some(apply) = on_locale_lang {
            apply.call(tag);
        }
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
                                // that will never come. `rooms_loaded` stays
                                // FALSE — a failed read is not a successful empty
                                // result, so the shell must not announce "0
                                // rooms" or render "No rooms yet"; the shell
                                // gates loading/empty on `notice.is_none()`, so a
                                // terminal notice shows neither (just the error).
                                state.set_terminal_notice(diagnostic_notice(&error));
                                return;
                            }
                        }
                    }
                }
            };

            let consume = async {
                // Announce a connection PROBLEM (drop to Interrupted/Failed/Stopped)
                // or a RECOVERY (return to Ready after such a drop) from EACH
                // `StateChanged` EVENT, not from a render snapshot. The event loop
                // can apply several lifecycle writes before Dioxus renders, so a
                // render-effect could observe only the FINAL state (`previous == Ready`
                // for a batched Interrupted→Ready) and announce NEITHER transition;
                // deriving per event cannot miss one. The happy boot path
                // (Idle/Connecting → Ready) is deliberately not announced. `Announcer`
                // coalesces, so a repeated state still announces once. Announced
                // through the dedicated CONNECTION region so a room-count update in
                // the same render never overwrites it (`StatusIndicator` has no live
                // semantics — §5.6). Room-count/notice announcements stay render-
                // driven: they coalesce by design and are not transition-sensitive.
                let mut prev = None::<State>;
                while let Some(event) = events.next().await {
                    ui.write().apply_event(&event);
                    if let ClientEvent::StateChanged { to, .. } = event {
                        let is_problem =
                            matches!(to, State::Interrupted | State::Failed | State::Stopped);
                        let recovered = to == State::Ready
                            && matches!(
                                prev,
                                Some(State::Interrupted | State::Failed | State::Stopped)
                            );
                        if is_problem || recovered {
                            let resolved = locale.peek();
                            let word =
                                crate::l10n::wire::status_for(catalog_for(resolved.text), to);
                            announcers
                                .connection
                                .announce(catalog_for(resolved.text).conn_announcement(word));
                        }
                        prev = Some(to);
                    }
                }
            };

            futures::join!(read, consume);
        }
    });

    // Announce the loaded room count through the stable CONTENT region, once per
    // change: the effect re-runs only when the room list or the locale changes,
    // and `Announcer::announce` coalesces, so a list that re-renders many times
    // still announces once (the checklist's failure mode, designed out
    // structurally — §5.6).
    use_effect(move || {
        let snapshot = ui.read();
        let resolved = locale.read();
        if snapshot.rooms_loaded {
            let count = snapshot.rooms.len() as u64;
            let formatted = Formats::new(resolved.text, resolved.formatting).count(count);
            let category = plural_category(resolved.text, count);
            let message = catalog_for(resolved.text).rooms_count(&formatted, category);
            announcers.content.announce(message);
        }
    });

    // (The connection PROBLEM/RECOVERY announcement is driven per `StateChanged`
    // EVENT in the consume loop above, not by a render effect — a render snapshot
    // can miss a batched Interrupted→Ready pair.)

    // Announce a TERMINAL room-list failure through the stable CONTENT region. A
    // terminal read error sets the notice while the lifecycle stays `Ready` and
    // `rooms_loaded` stays false, so NEITHER the room-count effect (gated on
    // `rooms_loaded`) NOR the lifecycle effect (gated on a lifecycle change)
    // fires — without this a screen-reader user would keep hearing only the
    // loading state while the friendly error sits silently in the DOM (§5.6). The
    // retryable-disconnect notice is deliberately NOT announced here: its
    // Interrupted transition is already voiced by the lifecycle effect (in the
    // connection region), so announcing it again would be redundant. `Announcer`
    // coalesces, so a re-rendering shell still announces once.
    use_effect(move || {
        let snapshot = ui.read();
        let resolved = locale.read();
        if snapshot.notice.is_some() && snapshot.notice_terminal {
            let message = ErrorDisplay::room_list_failure(catalog_for(resolved.text), true).message;
            announcers.content.announce(message);
        }
    });

    let strings = use_strings();
    let snapshot = ui();

    // Primary copy for a failed room-list read is friendly catalog text; the raw
    // `room.list:` detail lives ONLY in the Diagnostics disclosure. Terminal
    // failures get copy that does not promise a retry that will never happen (§5.8
    // / the "no false recovery promise" rule). Rendered only in the mounted shell,
    // where the StatusFooter → Diagnostics disclosure the copy refers to actually
    // exists (the boot/terminal cover shows no room-list notice).
    let room_error = snapshot
        .notice
        .as_ref()
        .map(|_| ErrorDisplay::room_list_failure(strings, snapshot.notice_terminal).message);

    // The boot/terminal cover: initial activation ("connecting…") and the
    // stop/failure states, each with its own honest label. `Interrupted` was
    // Ready and is recovering, so the shell stays mounted (`StatusFooter` reports
    // it) rather than hiding the rooms behind a cover; the stop/failure states
    // render their own label — a "connecting" cover over a terminal state would be
    // a lie. Labels are catalog copy, so the cover speaks the resolved locale.
    let boot_target = match snapshot.lifecycle {
        State::Ready | State::Interrupted => None,
        State::Idle | State::Connecting => Some(strings.boot_connecting()),
        State::Stopping => Some(strings.boot_stopping()),
        State::Stopped => Some(strings.boot_stopped()),
        State::Failed => Some(strings.boot_failed()),
    };
    let rooms_label = strings.rooms_heading().to_string();
    let skip_rooms = strings.skip_to_rooms().to_string();
    let app_name = strings.app_name();
    let rooms_empty = strings.rooms_empty();
    let rooms_loading = strings.rooms_loading();

    // ONE render tree with ONE stable live region. The boot/terminal cover and the
    // mounted shell are the two arms of a single lifecycle conditional; the
    // `LiveRegion` sits OUTSIDE it, so it is the SAME template node — the SAME DOM
    // element — across a boot↔shell transition (`Ready`→`Failed`/`Stopped` and
    // back). Assistive tech then tracks one stable region rather than observing a
    // node removed and a different one mounted mid-announcement (§5.6). The boot
    // cover stays a ROOT child (never inside the `.app` grid, whose
    // `height: var(--vh-full)` a nested full-viewport cover would blow up); all
    // hooks are declared above, so this single return is order-safe.
    rsx! {
        if let Some(target) = boot_target {
            // No room-list notice on the cover: the room.list error is a
            // Ready-time read failure, orthogonal to why the client is
            // stopping/failed, and the friendly copy refers to a Diagnostics
            // disclosure this cover does not mount (a retryable notice would also
            // promise a retry beneath a terminal `Failed`). The boot/terminal
            // label is the honest primary message; the shell (with its
            // StatusFooter → Diagnostics) is where a room.list failure and its raw
            // detail belong.
            BootScreen {
                target: target.to_string(),
                notice: None,
            }
        } else {
            // Skip links are the FIRST focusable region and move focus (not just
            // scroll) to their `tabindex="-1"` landmark targets. Only "skip to
            // rooms" is offered: the rooms list is the one meaningful content
            // region and is visible on every viewport; a "skip to content" link
            // would point at the compact-hidden empty center.
            SkipLinks {
                SkipLink { anchor: "rooms-nav".to_string(), label: skip_rooms }
            }
            // A root pane state is always set (`pane-rooms`): the shared stylesheet
            // hides `.sidebar`/`.center` on compact viewports unless a pane is
            // selected, so a plain `app` root renders blank on a phone WebView.
            div { class: "app pane-rooms", id: "app-root",
                // The page's single `<h1>`, at the always-rendered root. Visually
                // hidden because the visible headings already show on screen; it
                // names the page for assistive tech.
                h1 { class: "visually-hidden", "{app_name}" }
                // The rooms pane is the PRIMARY content, so it is the `<main>`
                // landmark — and `pane-rooms` keeps it visible on EVERY viewport,
                // so every viewport has a main landmark (the fix for the compact
                // main gap). The room list within is a NAMED `<nav>`.
                main { class: "sidebar", id: "main-content", tabindex: "-1",
                    // The notice lives here, not the `.center` pane (hidden on
                    // compact). Terminal failures get copy that does not promise a
                    // retry (§5.8); the raw detail is in Diagnostics.
                    if let Some(message) = room_error.as_ref() {
                        div { class: "error-note", id: "notice", "{message}" }
                    }
                    nav {
                        class: "rooms-list",
                        id: "rooms-nav",
                        tabindex: "-1",
                        "aria-label": "{rooms_label}",
                        // Loading vs empty is shown ONLY when there is no notice: a
                        // terminal failure is neither "loading" nor an empty
                        // account, so the shell must not render "No rooms yet" or
                        // announce 0 rooms for a failed load.
                        if snapshot.rooms.is_empty() && snapshot.notice.is_none() {
                            // "No rooms yet" is an ANSWER, not a default: before the
                            // first reply an empty vector means "not answered yet".
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
                    // The footer reports the connection state accessibly and hosts
                    // the Diagnostics disclosure carrying the raw failure detail.
                    StatusFooter { state: snapshot.lifecycle, detail: snapshot.notice.clone() }
                }
                // The desktop-only detail pane — a plain `<section>` (NOT a second
                // landmark), `display:none` on compact. Carries the centre's h2.
                section { class: "center", id: "center", EmptyCenter {} }
            }
        }
        // TWO stable polite live regions — content and connection — both OUTSIDE
        // the lifecycle conditional so each is the SAME DOM node across boot↔shell
        // transitions, AND so a content announcement and a connection announcement
        // that fire in the same render do not overwrite each other. Visually hidden.
        LiveRegion { id: "live-region".to_string(), message: announcers.content.message() }
        LiveRegion {
            id: "connection-live-region".to_string(),
            message: announcers.connection.message(),
        }
    }
}

/// The raw, secret-scrubbed detail recorded for the Diagnostics dialog. Keeps
/// the `room.list:` context an operator needs while guaranteeing no token-shaped
/// value reaches the recorded string (§5.8).
fn diagnostic_notice(error: &CallError) -> String {
    format!("room.list: {}", ErrorDisplay::diagnostic_detail(error))
}
