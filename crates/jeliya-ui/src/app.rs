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
use jeliya_api::{RoomId, RoomList, RoomRow, Standing, SubjectState};
use jeliya_client::{CallError, ClientEvent, ClientHandle, Dedup, State};
use jeliya_platform::navigation::{RoomDest, Route};
use jeliya_platform::PreferenceKey;

use crate::components::{
    use_announce_context, BootScreen, EmptyCenter, FleetPane, GlobalNav, LiveRegion, NavLandmark,
    Onboarding, RecoveryBanner, RoomArchivePane, RoomShell, RoomUnavailable, SettingsPane,
    SkipLink, SkipLinks, StatusFooter,
};
use crate::l10n::{
    catalog_for, plural_category, use_locale_context, use_strings, ErrorDisplay, Formats,
    LocaleState,
};
use crate::shell::bootstrap::{
    derive_boot_view, BootView, ConnectionSnapshot, FailureView, OnboardStep, RecoveryView,
    RoomsKnowledge,
};
use crate::shell::router::{use_route, NavIntent};
use crate::shell::Shell;
use crate::state::UiState;
use crate::PlatformServices;

/// A selectable room row (#178 §5.F): the existing `.room-select` markup, made
/// navigable — selecting the room pushes `Route::Room{ id, Activity }`. Kept
/// beside the display-only `RoomListItem` primitive so the row that navigates is
/// distinct from the one used in non-routed contexts.
#[component]
fn RoomSelectableItem(room: RoomRow, navigate: Callback<NavIntent>) -> Element {
    let strings = use_strings();
    let untitled = strings.room_untitled();
    let label = if room.name.trim().is_empty() {
        untitled.to_string()
    } else {
        room.name.clone()
    };
    let room_id = room.room_id.clone();
    rsx! {
        div { class: "room-item", "data-room": "{room.room_id}",
            button {
                class: "room-select",
                onclick: move |_| {
                    navigate
                        .call(
                            NavIntent::push(Route::Room {
                                room_id: room_id.clone(),
                                dest: RoomDest::Activity,
                            }),
                        )
                },
                span { class: "room-info",
                    span { class: "room-name-line",
                        span { class: "room-name", "{label}" }
                    }
                }
            }
        }
    }
}

/// Whether a `prev → to` lifecycle transition is announced through the CONNECTION
/// live region: a DROP to a problem state (`Interrupted`/`Failed`/`Stopped`), or a
/// RECOVERY back to `Ready` after such a drop. The happy boot path (`Idle`/
/// `Connecting` → `Ready`) is deliberately silent — those are not problem states,
/// so a first `Ready` seeded from them (or from a subscribe-time `Interrupted`
/// snapshot, which correctly DOES announce the recovery) is classified honestly.
///
/// `through_problem` is the event's `coalesced_through_problem` flag: under
/// backpressure the client coalesces a queued `Ready → Interrupted` + `Interrupted
/// → Ready` into a net `Ready → Ready` with honest endpoints, so `prev`/`to` alone
/// no longer reveal the drop; the flag records that the window passed through a
/// problem, so the recovery is still announced on resume (§5.6 / §K12).
fn announces_connection_change(prev: Option<State>, to: State, through_problem: bool) -> bool {
    let is_problem = matches!(to, State::Interrupted | State::Failed | State::Stopped);
    let recovered = to == State::Ready
        && (through_problem
            || matches!(
                prev,
                Some(State::Interrupted | State::Failed | State::Stopped)
            ));
    is_problem || recovered
}

/// The room-list knowledge the bootstrap fold (§5.D) consumes, read from the
/// folded [`UiState`]: a terminal read failure is [`RoomsKnowledge::Failed`] (a
/// shell-time error, never an empty account), an answered read is
/// [`RoomsKnowledge::Loaded`], and anything before the first answer is
/// [`RoomsKnowledge::Unknown`] — *booting is unknown, not zero*. A retryable
/// (non-terminal) disconnect notice leaves `rooms_loaded` false, so it too reads
/// as `Unknown` (recovering), never as a spurious zero.
fn rooms_knowledge(ui: &UiState) -> RoomsKnowledge {
    if ui.notice.is_some() && ui.notice_terminal {
        RoomsKnowledge::Failed
    } else if ui.rooms_loaded {
        RoomsKnowledge::Loaded {
            count: ui.rooms.len(),
        }
    } else {
        RoomsKnowledge::Unknown
    }
}

/// Local onboarding progress the pure daemon-truth fold cannot express on its
/// own: the deterministic mock has no re-`Hello`, so once the user creates an
/// identity the scripted connection snapshot still reads `Absent`, and once they
/// create a room the scripted `room.list` still reads zero. This records the
/// user's own advance so the shell moves forward — the spec's "*or user advanced
/// past onboarding*" [`BootView::Shell`] path (§5.D). It rides `AppRoot` as a
/// signal, never a second copy of daemon truth.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OnboardProgress {
    /// No local advance yet — follow the fold as-is.
    Following,
    /// Identity created locally → force the rooms step (the snapshot still reads
    /// `Absent`).
    RoomsStep,
    /// A room was created/joined → onboarding complete; mount the shell (the
    /// snapshot still reads zero rooms).
    Done,
}

/// Fold the local onboarding [`OnboardProgress`] over the daemon-truth
/// [`BootView`]. Only the onboarding steps are overridable; a cover/failure/shell
/// view from the fold is authoritative and passes through unchanged (a terminal
/// state must win over a stale local advance — §5.D).
fn advance_boot(view: BootView, progress: OnboardProgress) -> BootView {
    match (view, progress) {
        (BootView::Onboarding(_), OnboardProgress::Done) => BootView::Shell,
        (BootView::Onboarding(OnboardStep::Identity), OnboardProgress::RoomsStep) => {
            BootView::Onboarding(OnboardStep::Rooms)
        }
        (other, _) => other,
    }
}

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
    /// The pre-parse `location.pathname` at launch (#178 §5.C), injected from
    /// composition where the `web-sys` read is confined. Drives the router's
    /// mount canonicalization and last-room restore. Defaults to the canonical
    /// Rooms path for host/mock renders.
    #[props(default)]
    raw_path: Option<String>,
    /// The responsive shell at launch (#178 §8), an injected reactive input (the
    /// web target subscribes `matchMedia` in composition; other targets provide
    /// their own). Defaults to the wide shell.
    #[props(default = Shell::Wide)]
    initial_shell: Shell,
    /// The visible recovery banner to show when corrupt/unsupported new-format
    /// preference state was reset at boot (#178 §6.4). `None` on the ordinary
    /// fresh-tab path.
    #[props(default)]
    recovery: Option<RecoveryView>,
    /// The read-only daemon connection snapshot (#178 §5.D / D2): identity
    /// presence + storage generation captured from `Hello`. Drives the
    /// bootstrap/onboarding fold ([`derive_boot_view`]) — subject `Absent` opens
    /// the identity step, subject `Present` + zero rooms the rooms step, and a
    /// `Present` subject with rooms the mounted shell. Scripted by composition
    /// against the deterministic mock today (the `ClientHandle` seam does not yet
    /// surface it — D2/R1); the real value arrives from `Hello` with #171/#270.
    /// `None` is the honest seam-gap default: no snapshot from the seam, so the
    /// shell mounts on the lifecycle alone exactly as before (the fresh-tab and
    /// a11y-matrix path).
    #[props(default)]
    connection: Option<ConnectionSnapshot>,
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

    // The router (#178 §5.C): the mirror `Signal<Route>` + a navigate handle.
    // Seeded from the injected raw path (canonicalized once at mount via
    // replace) and the `LastRoom` preference (restored once, only from the bare
    // root). The route IS the navigation state — no second machine.
    let router_raw_path = raw_path.clone().unwrap_or_else(|| Route::Rooms.to_path());
    let last_room = preferences.get(&PreferenceKey::LastRoom).map(RoomId::new);
    let router = use_route(services.clone(), router_raw_path, last_room);
    // The responsive shell — an injected reactive input; composition pushes
    // `matchMedia` updates on the web target (§8). Held in a signal so a future
    // resize updates it; seeded from the prop.
    let shell = use_signal(|| initial_shell);
    // The visible recovery banner (§6.4), dismissable once acknowledged. The
    // corrupt/unsupported keys were already reset at boot (that is what produced
    // this view); the explicit action clears the remaining enumerated top-level
    // preferences and dismisses the banner.
    let mut recovery_view = use_signal(|| recovery.clone());

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

    // A clone of the client seam for the onboarding surface (the prop is moved
    // into the event-consumption future below). Onboarding's identity/rooms steps
    // dispatch `subject.ensure`/`room.create` through this same handle.
    let onboarding_handle = handle.clone();
    // A clone of the client seam for the departed-room archive pane (#91). The
    // prop `handle` is moved into the event-consumption future below, so the
    // render body cannot name it; the archive pane dispatches `room.archive`
    // through this clone (every clone is the same client — an `Arc` bump).
    let archive_handle = handle.clone();
    // The user's local onboarding advance (§5.D "user advanced past onboarding"),
    // folded over daemon truth by `advance_boot`. Starts `Following` (pure fold);
    // the onboarding step callbacks advance it as the user completes each step.
    let mut onboarding_progress = use_signal(|| OnboardProgress::Following);

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
            let initial_state = handle.state();
            ui.write().lifecycle = initial_state;
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
                // Classify each transition from the EVENT'S OWN `from`/`to`, not a
                // tracked snapshot: a subscription is live-only, so if a drop→recovery
                // happened between `subscribe()` and the `initial_state` snapshot, the
                // subscription BUFFERS `StateChanged { from: Interrupted, to: Ready }`
                // while `initial_state` is already `Ready`. A snapshot-seeded `prev`
                // would compare `Ready → Ready` and swallow the recovery; the event's own
                // `from` records the real origin, so the recovery is still announced. The
                // client coalesces flapping transitions into one event and PRESERVES the
                // earliest `from`, so per-event classification respects coalescing too.
                // The happy boot path (`Idle`/`Connecting` → `Ready`) is still silent
                // because those `from`s are not problem states.
                while let Some(event) = events.next().await {
                    ui.write().apply_event(&event);
                    if let ClientEvent::StateChanged {
                        from,
                        to,
                        coalesced_through_problem,
                    } = event
                    {
                        if announces_connection_change(Some(from), to, coalesced_through_problem) {
                            let resolved = locale.peek();
                            let word =
                                crate::l10n::wire::status_for(catalog_for(resolved.text), to);
                            announcers
                                .connection
                                .announce(catalog_for(resolved.text).conn_announcement(word));
                        }
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

    // The boot/terminal cover label from the lifecycle ALONE — the honest default
    // when no connection snapshot is available from the seam (D2/R1). `Interrupted`
    // was Ready and is recovering, so the shell stays mounted (`StatusFooter`
    // reports it) rather than hiding the rooms behind a cover; the stop/failure
    // states render their own label — a "connecting" cover over a terminal state
    // would be a lie. Labels are catalog copy, so the cover speaks the resolved
    // locale.
    let lifecycle_cover = match snapshot.lifecycle {
        State::Ready | State::Interrupted => None,
        State::Idle | State::Connecting => Some(strings.boot_connecting().to_string()),
        State::Stopping => Some(strings.boot_stopping().to_string()),
        State::Stopped => Some(strings.boot_stopped().to_string()),
        State::Failed => Some(strings.boot_failed().to_string()),
    };

    // The bootstrap/onboarding fold (§5.D / D2). When a connection snapshot IS
    // available (scripted against the mock today; the real `Hello` snapshot with
    // #171/#270), daemon truth — lifecycle + identity presence + room-list
    // knowledge — decides cover vs. onboarding vs. shell, with the user's local
    // advance folded in (`advance_boot`). When it is absent (`None`, the seam-gap
    // default), the shell mounts on the lifecycle alone, preserving the fresh-tab /
    // a11y-matrix path. `cover_label` and `onboarding_step` are mutually exclusive;
    // when both are `None` the mounted shell renders.
    let (cover_label, onboarding_step): (Option<String>, Option<OnboardStep>) =
        match connection.as_ref() {
            None => (lifecycle_cover, None),
            Some(conn) => {
                let view = advance_boot(
                    derive_boot_view(snapshot.lifecycle, Some(conn), rooms_knowledge(&snapshot)),
                    onboarding_progress(),
                );
                match view {
                    BootView::Booting => (Some(strings.boot_connecting().to_string()), None),
                    BootView::Failed(FailureView::Stopping) => {
                        (Some(strings.boot_stopping().to_string()), None)
                    }
                    BootView::Failed(FailureView::Stopped) => {
                        (Some(strings.boot_stopped().to_string()), None)
                    }
                    BootView::Failed(FailureView::Failed) => {
                        (Some(strings.boot_failed().to_string()), None)
                    }
                    BootView::Onboarding(step) => (None, Some(step)),
                    BootView::Shell => (None, None),
                }
            }
        };
    let rooms_label = strings.rooms_heading().to_string();
    let skip_rooms = strings.skip_to_rooms().to_string();
    let app_name = strings.app_name();
    let rooms_empty = strings.rooms_empty();
    let rooms_loading = strings.rooms_loading();

    // The current route (the mirror) and shell, read for the destination switch
    // and the global-nav highlight. Reading the signals here subscribes this
    // render to them, so a navigation or a resize re-renders the shell.
    let current_route = router.route.read().clone();
    let active_shell = shell();
    let navigate = router.navigate;
    // Clones for the closures/props below; the prop `services` stays available.
    let services_reset = services.clone();
    let services_settings = services.clone();
    // The caller's own subject, when the connection snapshot names it (#91 D4):
    // it lets a departed-room archive identify the caller's OWN signed removal to
    // name the remover. `None` is the honest default while the seam does not
    // surface `Hello.subject` (#178 D2/R1); the archive then states "removed"
    // without naming anyone (#91 R5).
    let me = connection.as_ref().and_then(|conn| match &conn.subject {
        SubjectState::Present { subject_id, .. } => Some(subject_id.clone()),
        SubjectState::Absent => None,
    });

    // ONE render tree with ONE stable live region. The boot/terminal cover, the
    // onboarding surface, and the mounted shell are the three arms of a single
    // conditional; the `LiveRegion`s sit OUTSIDE it, so each is the SAME template
    // node — the SAME DOM element — across a cover↔onboarding↔shell transition
    // (`Ready`→`Failed`/`Stopped` and back, or onboarding→shell). Assistive tech
    // then tracks one stable region rather than observing a node removed and a
    // different one mounted mid-announcement (§5.6). The boot cover stays a ROOT
    // child (never inside the `.app` grid, whose `height: var(--vh-full)` a nested
    // full-viewport cover would blow up); all hooks are declared above, so this
    // single return is order-safe.
    rsx! {
        if let Some(label) = cover_label {
            // No room-list notice on the cover: the room.list error is a
            // Ready-time read failure, orthogonal to why the client is
            // stopping/failed, and the friendly copy refers to a Diagnostics
            // disclosure this cover does not mount (a retryable notice would also
            // promise a retry beneath a terminal `Failed`). The boot/terminal
            // label is the honest primary message; the shell (with its
            // StatusFooter → Diagnostics) is where a room.list failure and its raw
            // detail belong.
            BootScreen {
                target: label,
                notice: None,
            }
        } else if let Some(step) = onboarding_step {
            // First-run onboarding (§5.E), reached only when the connection
            // snapshot says so: subject `Absent` → the identity step, subject
            // `Present` with zero rooms → the rooms step. Advancing folds into
            // `onboarding_progress` — identity done moves to the rooms step, a
            // created/joined room completes onboarding and mounts the shell opened
            // at that room. The full-page onboarding surface stands in for the
            // shell (no global nav), so the live regions below stay the app's one
            // stable pair across the onboarding↔shell transition.
            Onboarding {
                handle: onboarding_handle.clone(),
                services: services.clone(),
                step,
                on_advance: move |room: Option<RoomId>| {
                    match room {
                        None => onboarding_progress.set(OnboardProgress::RoomsStep),
                        Some(room_id) => {
                            onboarding_progress.set(OnboardProgress::Done);
                            navigate
                                .call(
                                    NavIntent::push(Route::Room {
                                        room_id,
                                        dest: RoomDest::Activity,
                                    }),
                                );
                        }
                    }
                },
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
                // Global-destination navigation (#178 §5.F/§8): Rooms / Fleet /
                // Settings, with placement (rail vs. compact bottom bar) driven by
                // the injected `Shell` value — never a `cfg` (D6). The active
                // destination is derived from the route (D1).
                GlobalNav { route: current_route.clone(), navigate, shell: active_shell }
                // The visible recovery banner (§6.4, AC-5): shown when corrupt or
                // unsupported new-format preference state was reset at boot. It
                // overlays the shell rather than replacing it — the app stays
                // usable (D7). The action clears the enumerated top-level
                // preferences and dismisses the banner (the corrupt keys were
                // already purged at boot).
                if let Some(rec) = recovery_view() {
                    RecoveryBanner {
                        recovery: rec,
                        on_reset: move |_| {
                            for key in [
                                PreferenceKey::LastRoom,
                                PreferenceKey::Aliases,
                                PreferenceKey::SelfLabel,
                                PreferenceKey::TextLocale,
                                PreferenceKey::FormattingLocale,
                            ] {
                                services_reset.preferences().remove(&key);
                            }
                            recovery_view.set(None);
                        },
                    }
                }
                // The destination pane, switched on the route (D1 — the route IS
                // the state). Rooms keeps the existing landmarked rooms shell; the
                // other destinations render skeletons (content is #179–#181).
                {
                    match current_route.clone() {
                        Route::Rooms => rsx! {
                            // The rooms pane is the PRIMARY content, so it is the
                            // `<main>` landmark, visible on every viewport. The room
                            // list within is a NAMED `<nav>` (the `NavLandmark`
                            // primitive, Decision-6).
                            main { class: "sidebar", id: "main-content", tabindex: "-1",
                                if let Some(message) = room_error.as_ref() {
                                    div { class: "error-note", id: "notice", "{message}" }
                                }
                                NavLandmark {
                                    class: "rooms-list",
                                    id: "rooms-nav",
                                    label: "{rooms_label}",
                                    if snapshot.rooms.is_empty() && snapshot.notice.is_none() {
                                        if snapshot.rooms_loaded {
                                            div { class: "rooms-empty muted", id: "rooms-empty", "{rooms_empty}" }
                                        } else {
                                            div { class: "rooms-empty muted", id: "rooms-loading", "{rooms_loading}" }
                                        }
                                    }
                                    for room in snapshot.rooms.iter() {
                                        RoomSelectableItem {
                                            key: "{room.room_id}",
                                            room: room.clone(),
                                            navigate,
                                        }
                                    }
                                }
                                StatusFooter { state: snapshot.lifecycle, detail: snapshot.last_diagnostic.clone() }
                            }
                            section { class: "center", id: "center", EmptyCenter {} }
                        },
                        // A room-scoped route selects its composition by the row's
                        // STANDING (#91 D1/§7), a standing-driven, dest-agnostic
                        // choice — no new `Route` variant:
                        //   Active         -> the live `RoomShell` (#179 surface)
                        //   Left | Removed -> the read-only `RoomArchivePane` (#91)
                        //   absent from room.list, once ANSWERED -> `RoomUnavailable`
                        //     (the recoverable "not on this device", Rooms as the way
                        //     out) — never a shell over a room the user does not hold.
                        // Before the list answers (unknown ≠ unreachable) the loading
                        // room frame is shown so a deep link never flashes
                        // "unavailable" or the archive while the list is still loading.
                        Route::Room { room_id, dest } => {
                            let standing = snapshot
                                .rooms
                                .iter()
                                .find(|r| r.room_id == room_id)
                                .map(|r| r.standing);
                            let surface = if !snapshot.rooms_loaded {
                                rsx! { RoomShell { room_id: room_id.clone(), dest: dest.clone(), navigate } }
                            } else {
                                match standing {
                                    Some(Standing::Active) => rsx! {
                                        RoomShell { room_id: room_id.clone(), dest: dest.clone(), navigate }
                                    },
                                    Some(departed @ (Standing::Left | Standing::Removed)) => rsx! {
                                        // The departed room opens as a local read-only
                                        // archive. Keyed by room id so navigating
                                        // between two departed rooms re-mounts the pane
                                        // and re-reads its `room.archive`.
                                        RoomArchivePane {
                                            key: "{room_id}",
                                            room_id: room_id.clone(),
                                            my_standing: departed,
                                            me: me.clone(),
                                            navigate,
                                            handle: archive_handle.clone(),
                                        }
                                    },
                                    None => rsx! { RoomUnavailable { navigate } },
                                }
                            };
                            rsx! {
                                main { class: "sidebar", id: "main-content", tabindex: "-1",
                                    {surface}
                                    StatusFooter { state: snapshot.lifecycle, detail: snapshot.last_diagnostic.clone() }
                                }
                            }
                        }
                        Route::Fleet => rsx! {
                            main { class: "sidebar", id: "main-content", tabindex: "-1",
                                FleetPane {}
                                StatusFooter { state: snapshot.lifecycle, detail: snapshot.last_diagnostic.clone() }
                            }
                        },
                        Route::Settings => rsx! {
                            main { class: "sidebar", id: "main-content", tabindex: "-1",
                                SettingsPane { services: services_settings.clone(), subject_id: None }
                                StatusFooter { state: snapshot.lifecycle, detail: snapshot.last_diagnostic.clone() }
                            }
                        },
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::{advance_boot, announces_connection_change, rooms_knowledge, OnboardProgress};
    use crate::shell::bootstrap::{
        derive_boot_view, BootView, ConnectionSnapshot, FailureView, OnboardStep, RoomsKnowledge,
    };
    use crate::state::UiState;
    use jeliya_api::{
        DeviceId, LastEvent, Role, RoomId, RoomRow, Standing, SubjectId, SubjectState,
    };
    use jeliya_client::State;

    fn test_room(id: &str) -> RoomRow {
        RoomRow {
            room_id: RoomId::new(id),
            name: id.to_string(),
            standing: Standing::Active,
            live: false,
            role: Role::Member,
            member_count: 1,
            last_event: LastEvent::Absent,
            capabilities: Vec::new(),
        }
    }

    fn absent() -> ConnectionSnapshot {
        ConnectionSnapshot {
            subject: SubjectState::Absent,
            storage_generation: 1,
        }
    }

    fn present() -> ConnectionSnapshot {
        ConnectionSnapshot {
            subject: SubjectState::Present {
                subject_id: SubjectId::new("blake3:subject"),
                device_id: DeviceId::new("blake3:device"),
            },
            storage_generation: 1,
        }
    }

    #[test]
    fn a_recovery_after_a_missed_interrupt_is_announced() {
        // Subscribe-after-interrupt: the tracker is seeded from the recovered
        // snapshot (`Interrupted`), so the first `Ready` is a RECOVERY. With the old
        // `None` seed this went unannounced.
        assert!(announces_connection_change(
            Some(State::Interrupted),
            State::Ready,
            false
        ));
        assert!(announces_connection_change(
            Some(State::Failed),
            State::Ready,
            false
        ));
        assert!(announces_connection_change(
            Some(State::Stopped),
            State::Ready,
            false
        ));
    }

    #[test]
    fn a_coalesced_problem_window_still_announces_the_recovery() {
        // Backpressure coalesces `Ready → Interrupted` + `Interrupted → Ready` into a
        // net `Ready → Ready` with HONEST endpoints and `coalesced_through_problem`
        // set. `from`/`to` alone read as no change, so the flag is what keeps the
        // recovery audible on resume (§K12).
        assert!(announces_connection_change(
            Some(State::Ready),
            State::Ready,
            true
        ));
    }

    #[test]
    fn the_happy_boot_path_stays_silent() {
        // Seeded from the boot states (or `None`), a first `Ready` announces nothing.
        assert!(!announces_connection_change(
            Some(State::Idle),
            State::Ready,
            false
        ));
        assert!(!announces_connection_change(
            Some(State::Connecting),
            State::Ready,
            false
        ));
        assert!(!announces_connection_change(None, State::Ready, false));
        // Ready → Ready with NO problem in the window (a plain coalesced repeat) is
        // not a transition to announce.
        assert!(!announces_connection_change(
            Some(State::Ready),
            State::Ready,
            false
        ));
    }

    #[test]
    fn a_drop_to_a_problem_state_is_always_announced() {
        for to in [State::Interrupted, State::Failed, State::Stopped] {
            assert!(announces_connection_change(Some(State::Ready), to, false));
            assert!(announces_connection_change(None, to, false));
        }
    }

    #[test]
    fn rooms_knowledge_reads_the_folded_snapshot() {
        // Before any answer — unknown, never a spurious zero.
        let ui = UiState::new();
        assert_eq!(rooms_knowledge(&ui), RoomsKnowledge::Unknown);

        // An answered empty read is Loaded{0} (the rooms-onboarding trigger), not
        // Unknown — the daemon has answered "no rooms".
        let mut loaded_empty = UiState::new();
        loaded_empty.set_rooms(Vec::new());
        assert_eq!(
            rooms_knowledge(&loaded_empty),
            RoomsKnowledge::Loaded { count: 0 }
        );

        // An answered non-empty read carries the count.
        let mut loaded = UiState::new();
        loaded.set_rooms(vec![test_room("r-1"), test_room("r-2")]);
        assert_eq!(
            rooms_knowledge(&loaded),
            RoomsKnowledge::Loaded { count: 2 }
        );

        // A TERMINAL read failure is Failed (a shell-time error), not an empty
        // account.
        let mut failed = UiState::new();
        failed.set_terminal_notice("room.list: boom");
        assert_eq!(rooms_knowledge(&failed), RoomsKnowledge::Failed);

        // A RETRYABLE (non-terminal) disconnect notice leaves the read unanswered
        // — still Unknown (recovering), never a spurious zero or a Failed.
        let mut retrying = UiState::new();
        retrying.set_notice("room.list: transient");
        assert_eq!(rooms_knowledge(&retrying), RoomsKnowledge::Unknown);
    }

    #[test]
    fn advance_boot_overrides_only_the_onboarding_steps() {
        use BootView::*;
        // No local advance yet — the fold passes through.
        assert_eq!(
            advance_boot(
                Onboarding(OnboardStep::Identity),
                OnboardProgress::Following
            ),
            Onboarding(OnboardStep::Identity)
        );
        // Identity done locally → the rooms step (the mock snapshot still reads
        // Absent; there is no re-Hello).
        assert_eq!(
            advance_boot(
                Onboarding(OnboardStep::Identity),
                OnboardProgress::RoomsStep
            ),
            Onboarding(OnboardStep::Rooms)
        );
        // RoomsStep overrides ONLY the identity step, never the rooms step itself.
        assert_eq!(
            advance_boot(Onboarding(OnboardStep::Rooms), OnboardProgress::RoomsStep),
            Onboarding(OnboardStep::Rooms)
        );
        // A created/joined room completes onboarding → the shell, from either step.
        assert_eq!(
            advance_boot(Onboarding(OnboardStep::Rooms), OnboardProgress::Done),
            Shell
        );
        assert_eq!(
            advance_boot(Onboarding(OnboardStep::Identity), OnboardProgress::Done),
            Shell
        );
        // Non-onboarding views are authoritative — a local advance never rewrites
        // them (a terminal state must win over a stale advance).
        assert_eq!(advance_boot(Shell, OnboardProgress::Done), Shell);
        assert_eq!(advance_boot(Booting, OnboardProgress::RoomsStep), Booting);
        assert_eq!(
            advance_boot(Failed(FailureView::Stopping), OnboardProgress::Done),
            Failed(FailureView::Stopping)
        );
    }

    #[test]
    fn app_boot_decision_walks_first_run_onboarding_to_the_shell() {
        // The exact decision `AppRoot` makes, end to end, from a scripted snapshot:
        // a fresh daemon (subject Absent) at Ready opens the IDENTITY step — the
        // first-run onboarding the shell used to skip.
        let identity = advance_boot(
            derive_boot_view(State::Ready, Some(&absent()), RoomsKnowledge::Unknown),
            OnboardProgress::Following,
        );
        assert_eq!(identity, BootView::Onboarding(OnboardStep::Identity));

        // After the identity step completes (local RoomsStep advance), the ROOMS
        // step shows even though the mock snapshot still reads Absent.
        let rooms = advance_boot(
            derive_boot_view(State::Ready, Some(&absent()), RoomsKnowledge::Unknown),
            OnboardProgress::RoomsStep,
        );
        assert_eq!(rooms, BootView::Onboarding(OnboardStep::Rooms));

        // A Present subject with zero answered rooms is itself the rooms step…
        let present_zero = advance_boot(
            derive_boot_view(
                State::Ready,
                Some(&present()),
                RoomsKnowledge::Loaded { count: 0 },
            ),
            OnboardProgress::Following,
        );
        assert_eq!(present_zero, BootView::Onboarding(OnboardStep::Rooms));

        // …and once a room exists (or the user advanced past onboarding) the shell
        // mounts.
        let shell_from_rooms = advance_boot(
            derive_boot_view(
                State::Ready,
                Some(&present()),
                RoomsKnowledge::Loaded { count: 1 },
            ),
            OnboardProgress::Following,
        );
        assert_eq!(shell_from_rooms, BootView::Shell);
        let shell_from_advance = advance_boot(
            derive_boot_view(
                State::Ready,
                Some(&present()),
                RoomsKnowledge::Loaded { count: 0 },
            ),
            OnboardProgress::Done,
        );
        assert_eq!(shell_from_advance, BootView::Shell);
    }
}
