//! Target-selected composition — the **only** place a target choice is made
//! (§6.4, Decision 3).
//!
//! Composition selects the concrete [`jeliya_client::ClientHandle`] source and
//! the concrete [`crate::PlatformServices`] implementation, then hands them to
//! [`crate::AppRoot`] as separate props. It contains **no** product or
//! business-logic `cfg` — only wiring. For this M3 foundation slice there is
//! one real target (the browser, via `bin/web.rs`); the `native`
//! system-WebView target (M4) selects its own composition here later.
//!
//! #176 renders against the deterministic **mock** — the reference behaviour.
//! The mock is driven to `Ready` and its scripted mount reads are settled
//! event-driven: the driver awaits [`MockController::pending_call`] and
//! delivers when the app's own dispatch wakes it (no wall clock, no busy
//! loop, no scheduling guess). The real browser transport (`WsWeb`, #168)
//! replaces [`web_composition`]'s mock with the live adapter behind the same
//! handle; nothing else in this crate changes.

use std::rc::Rc;

use dioxus::prelude::*;
use jeliya_api::{
    CapabilityToken, DeviceId, FleetList, FleetListOut, FleetRow, InviteId, InviteList,
    InviteListOut, InviteRow, LastEvent, LastSeen, LatestStatus, Link, Liveness, MemberRow,
    PeerRow, Reachability, Redeemability, Role, RoomId, RoomList, RoomListOut, RoomMembers,
    RoomMembersOut, RoomPeers, RoomPeersOut, RoomRow, Standing, StatusLabel, SubjectId, Timestamp,
    Truncated,
};
use jeliya_client::mock::{MockController, MockScript, Program};
use jeliya_client::{ClientHandle, State};

use crate::app::AppRoot;
use crate::PlatformServices;

/// A fully wired composition: the injected handle and services, plus the mock
/// controller that drives the reference backend for #176.
#[derive(Clone)]
pub struct WebComposition {
    /// The client seam handed to [`crate::AppRoot`].
    pub handle: ClientHandle,
    /// The platform-authority seam handed to [`crate::AppRoot`], injected
    /// separately from `handle`.
    pub services: PlatformServices,
    /// The mock controller. `Rc` so the composition stays `Clone` for a Dioxus
    /// hook; single-threaded on the browser target.
    pub controller: Rc<MockController>,
    /// Preference-schema anomalies found at boot (corrupt/unsupported new-format
    /// state that was reset), driving the visible recovery banner (#178 §6.4).
    /// Empty on the host fake and on a fresh browser tab.
    pub anomalies: Vec<crate::prefs::SchemaAnomaly>,
}

/// Build the browser composition: the deterministic mock behind a
/// [`ClientHandle`], the browser-appropriate [`PlatformServices`], and the
/// controller that settles the mock's reads.
///
/// The mock scripts an empty `room.list` reply — the honest state of a fresh
/// daemon and enough to render the shell. Timeline, membership, and the
/// composer are the Room Workbench port (a later M3 slice), out of scope here.
pub fn web_composition() -> WebComposition {
    // Scripted reference replies for the #180 destination reads (People / Agents
    // / Fleet), so a room navigated to via the `?rooms=N` offline fixture renders
    // real content against the deterministic mock rather than an honest read
    // failure. Matched by op NAME (not room), so one reply serves any room; a
    // second read of the same op (a post-mutation refresh, or a second pane)
    // exhausts the script and surfaces the truthful Failed/offline state — the
    // real web end-to-end proof is #171/#182 (R1/R4).
    let (handle, controller) = MockScript::new()
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&RoomListOut {
                rooms: fixture_rooms(),
            }),
        )
        .on(
            "room.members",
            Program::reply_ok::<RoomMembers>(&fixture_members()),
        )
        .on(
            "invite.list",
            Program::reply_ok::<InviteList>(&fixture_invites()),
        )
        .on(
            "fleet.list",
            Program::reply_ok::<FleetList>(&fixture_fleet()),
        )
        .on(
            "room.peers",
            Program::reply_ok::<RoomPeers>(&fixture_peers()),
        )
        .build();
    // Inject the real browser `WebPlatform` on the `web` target (#178 §5.A/§3.3):
    // History-API navigation, browser lifecycle, session-scoped versioned
    // preferences + the boot-time legacy purge, in-memory secrets. Off the `web`
    // target (host tests, MSRV, the compose-shape check) the browser-shaped
    // deterministic fake stands in — same browser SHAPE (session-scoped, no
    // window, no private directory), host-compilable. The `ClientHandle` stays
    // the deterministic mock until #171's `WsWeb` lands behind the same handle.
    let (services, anomalies) = {
        #[cfg(feature = "web")]
        {
            crate::platform_web::WebPlatform::build()
        }
        #[cfg(not(feature = "web"))]
        {
            (PlatformServices::fake_browser(), Vec::new())
        }
    };
    WebComposition {
        handle,
        services,
        controller: Rc::new(controller),
        anomalies,
    }
}

/// The browser root component: builds the composition once, drives the mock to
/// `Ready` and settles its mount reads, and renders [`crate::AppRoot`] with the
/// separately injected handle and services.
#[component]
pub fn WebRoot() -> Element {
    let composition = use_hook(web_composition);
    let handle = composition.handle.clone();
    let services = composition.services.clone();

    // #178: the visible recovery banner (from boot-time schema anomalies), the
    // launch path (for the router's mount canonicalization + last-room restore),
    // and the responsive shell — all read here at composition, so `AppRoot` stays
    // free of `web-sys`/`cfg` (Decision-3).
    let recovery = crate::shell::bootstrap::RecoveryView::from_anomalies(&composition.anomalies);
    let raw_path = web_raw_path();
    let initial_shell = web_shell();
    // The scripted daemon connection snapshot (§5.D / D2) that drives AppRoot's
    // bootstrap/onboarding fold. `None` on the ordinary fresh tab (the seam does
    // not yet surface `Hello.subject` — D2/R1), so the shell mounts on the
    // lifecycle alone; a marker-gated `?onboard=` fixture scripts a snapshot so the
    // offline suite can drive first-run onboarding against the deterministic mock.
    let connection = web_connection_snapshot();

    // The injected `<html lang>` setter AppRoot calls REACTIVELY on the resolved
    // locale (mount and any live switch). Confined here (web-sys); never in the
    // shared AppRoot (Decision-3). Other targets pass `None`.
    #[cfg(feature = "web")]
    let on_locale_lang = Some(use_callback(|tag: String| set_document_lang(&tag)));
    #[cfg(not(feature = "web"))]
    let on_locale_lang: Option<Callback<String>> = None;

    // The viewport watcher AppRoot registers its shell handler with ONCE: a
    // `matchMedia` listener at the compact boundary recomputes the shell from
    // the live width on every crossing (resize, zoom-induced crossing, device
    // rotation), so the rail/bottom-bar element identity never freezes at the
    // launch width. Confined here (web-sys); other targets pass `None` and
    // keep the launch shell, as before.
    #[cfg(feature = "web")]
    let on_shell_subscribe = Some(use_callback(|handler: Callback<crate::shell::Shell>| {
        install_shell_watcher(handler)
    }));
    #[cfg(not(feature = "web"))]
    let on_shell_subscribe: Option<Callback<Callback<crate::shell::Shell>>> = None;

    use_future(move || {
        let composition = composition.clone();
        async move {
            // e2e-only, marker-gated: expose a JS hook to drive connection transitions
            // so the a11y matrix can exercise the connection live region against a REAL
            // transition (inert in production; see `install_e2e_connection_hook`).
            #[cfg(feature = "web")]
            install_e2e_connection_hook(composition.controller.clone());
            // Drive the reference backend deterministically: reach `Ready` so
            // the shell leaves the boot state, then settle the mount reads
            // EVENT-DRIVEN. A fixed deliver-and-yield pass count guesses the
            // executor's scheduling — and guessed wrong here: Dioxus polls a
            // self-waking task ahead of its siblings, so the passes ran out
            // (or, unbounded, starved the app entirely) before the read task
            // ever dispatched, leaving the shipped shell on "Loading rooms…"
            // forever. `pending_call` parks until the app's dispatch itself
            // wakes the driver, and resolves on stop so this task never
            // outlives the backend. If the app never dispatches, the driver
            // stays parked (quiescent, no busy loop) and the e2e settle
            // assertion reports the missing read.
            composition.handle.start();
            // Deterministic BOOT/terminal fixture for the offline a11y matrix:
            // `?boot=<state>` drives the mock to that lifecycle (leaving the
            // BootScreen cover mounted) instead of the Ready shell, so the boot
            // branch — otherwise never reached once the mock settles — can be
            // axe-swept. Absent or unrecognized → the normal Ready shell + read.
            // Harmless in production (a curious user only sees the honest cover).
            match boot_fixture_state() {
                Some(state) => composition.controller.set_state(state),
                None => {
                    composition.controller.set_state(State::Ready);
                    // #180: the destination panes issue their reads at NAVIGATION
                    // time, not only at mount, so a fixed pass count would leave a
                    // People/Agents/Fleet read unsettled. Pump continuously —
                    // parking on `pending_call` between dispatches (no busy loop)
                    // and delivering each scripted reply as the app dispatches it.
                    pump_scripted_replies(&composition.controller).await;
                }
            }
        }
    });

    rsx! {
        AppRoot {
            handle,
            services,
            platform_locale: platform_locale(),
            on_locale_lang,
            raw_path,
            initial_shell,
            on_shell_subscribe,
            recovery,
            connection,
        }
    }
}

/// Install the ONE viewport watcher for the app's lifetime: a `matchMedia`
/// listener at the compact boundary ([`crate::shell::COMPACT_MAX`]) that
/// recomputes the shell from the live `innerWidth` on every crossing and
/// pushes it to AppRoot's registered handler. Never removed — the root lives
/// as long as the page, matching the composition's other root-level installs.
#[cfg(feature = "web")]
fn install_shell_watcher(handler: Callback<crate::shell::Shell>) {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    let Some(window) = web_sys::window() else {
        return;
    };
    let query = format!("(max-width: {}px)", crate::shell::COMPACT_MAX);
    let Ok(Some(mql)) = window.match_media(&query) else {
        return;
    };
    let on_change = Closure::<dyn FnMut()>::new(move || {
        let width = web_sys::window()
            .and_then(|w| w.inner_width().ok())
            .and_then(|v| v.as_f64())
            .unwrap_or(crate::shell::WIDE_MIN);
        handler.call(crate::shell::shell_for(width));
    });
    mql.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    on_change.forget();
}

/// The scripted daemon connection snapshot (#178 §5.D / D2) for `AppRoot`'s
/// bootstrap/onboarding fold — the ONE place the browser's onboarding fixture is
/// read.
///
/// In production the `ClientHandle` seam does not yet surface `Hello.subject`
/// (D2/R1), so this returns `None` and the shell mounts on the lifecycle alone
/// (the fresh-tab / a11y-matrix path). A marker-gated `?onboard=<identity|rooms>`
/// fixture scripts a snapshot so the offline suite can drive first-run onboarding
/// against the deterministic mock: `identity` → subject `Absent` (the identity
/// step), `rooms` → subject `Present` with the scripted empty `room.list` (the
/// rooms step). Gated on the SAME e2e `localStorage` marker as `?boot=`/`?rooms=`,
/// so production — loopback or not — never arms it, and `None` everywhere off the
/// `web` target.
fn web_connection_snapshot() -> Option<crate::shell::bootstrap::ConnectionSnapshot> {
    #[cfg(feature = "web")]
    {
        use crate::shell::bootstrap::ConnectionSnapshot;
        use jeliya_api::{DeviceId, SubjectId, SubjectState};

        let window = web_sys::window()?;
        let armed = window
            .local_storage()
            .ok()
            .flatten()
            .and_then(|storage| storage.get_item(BOOT_FIXTURE_MARKER).ok().flatten())
            .as_deref()
            == Some("1");
        if !armed {
            return None;
        }
        let search = window.location().search().ok()?;
        let value = search
            .trim_start_matches('?')
            .split('&')
            .find_map(|pair| pair.strip_prefix("onboard="))?;
        let subject = match value {
            "identity" => SubjectState::Absent,
            "rooms" => SubjectState::Present {
                subject_id: SubjectId::new("blake3:onboarding-subject"),
                device_id: DeviceId::new("blake3:onboarding-device"),
            },
            _ => return None,
        };
        Some(ConnectionSnapshot {
            subject,
            storage_generation: 1,
        })
    }
    #[cfg(not(feature = "web"))]
    {
        None
    }
}

/// The launch `location.pathname` for the router's mount canonicalization
/// (#178 §5.C) — the ONE place the raw pre-parse path is read. `None` off the
/// `web` target (host/native), where `AppRoot` defaults to the canonical Rooms
/// path.
fn web_raw_path() -> Option<String> {
    #[cfg(feature = "web")]
    {
        Some(crate::platform_web::location_pathname())
    }
    #[cfg(not(feature = "web"))]
    {
        None
    }
}

/// The responsive shell at launch (#178 §8), from the viewport width. A static
/// initial read (the `AppRoot` shell signal exists for a future `matchMedia`
/// resize subscription — Q4); defaults to the wide shell off the `web` target.
fn web_shell() -> crate::shell::Shell {
    #[cfg(feature = "web")]
    {
        let width = web_sys::window()
            .and_then(|w| w.inner_width().ok())
            .and_then(|v| v.as_f64())
            .unwrap_or(crate::shell::WIDE_MIN);
        crate::shell::shell_for(width)
    }
    #[cfg(not(feature = "web"))]
    {
        crate::shell::Shell::Wide
    }
}

/// The lifecycle a `?boot=<state>` query parameter requests, for the a11y matrix's
/// deterministic boot/terminal fixture (web target only; `None` everywhere else
/// and for any absent/unrecognized value → the normal Ready shell).
///
/// Only `connecting` and `failed` are accepted: `MockController::set_state`
/// deliberately PANICS on `Stopping`/`Stopped` (those carry the settled-work
/// guarantee only `ClientHandle::stop()` provides), so driving them here would
/// crash the app rather than render a cover. The BootScreen structure the a11y
/// matrix sweeps is identical across cover states (only the label differs), so
/// `failed` (terminal) + `connecting` (initial) cover the branch.
/// The `localStorage` marker an e2e harness sets (via `addInitScript`, before any
/// app script runs) to arm the `?boot=<state>` fixture. Shared verbatim with
/// `e2e/a11y-harness.ts`; changing it here means changing it there.
#[cfg(feature = "web")]
const BOOT_FIXTURE_MARKER: &str = "jeliya-e2e-boot-fixture";

fn boot_fixture_state() -> Option<State> {
    #[cfg(feature = "web")]
    {
        let window = web_sys::window()?;
        // DEV/TEST ONLY, gated on an EXPLICIT TEST MARKER — never the hostname. The
        // packaged daemon serves this SAME production UI from `127.0.0.1`
        // (`crates/jeliyad/src/serve.rs`), so a loopback-host gate would leave the
        // fixture live in the real app, where a shared or retained `?boot=…` link
        // could disable it. A `localStorage` marker only an e2e harness sets cannot
        // be forged by a plain URL, so production — loopback or not — never arms it.
        let armed = window
            .local_storage()
            .ok()
            .flatten()
            .and_then(|storage| storage.get_item(BOOT_FIXTURE_MARKER).ok().flatten())
            .as_deref()
            == Some("1");
        if !armed {
            return None;
        }
        let search = window.location().search().ok()?;
        // A tiny hand-parse (no url crate): find `boot=` in the query string.
        let value = search
            .trim_start_matches('?')
            .split('&')
            .find_map(|pair| pair.strip_prefix("boot="))?;
        match value {
            "connecting" => Some(State::Connecting),
            "failed" => Some(State::Failed),
            _ => None,
        }
    }
    #[cfg(not(feature = "web"))]
    {
        None
    }
}

/// Synthetic room rows for the a11y matrix's POPULATED-shell fixture (`?rooms=N`,
/// marker-gated, web only). The default shell renders an EMPTY list, so the
/// target-size sweep never measures `.room-select` rows; this fixture populates
/// them so their geometry is under test. Empty on every other target and in
/// production (no marker), which render the honest empty list. Mirrors the
/// `test_room` shape used by the state-fold unit tests.
fn fixture_rooms() -> Vec<RoomRow> {
    let mut caps = fixture_caps();
    // The Activity composer is gated on `MessageSend` (#179 D8): the populated
    // fixture always grants it so the offline Activity pane renders a live
    // composer rather than the departed-room read-only floor. The `?caps=1`
    // fixture adds the full #180 affordance set on top.
    if !caps.contains(&CapabilityToken::MessageSend) {
        caps.push(CapabilityToken::MessageSend);
    }
    (0..rooms_fixture_count())
        .map(|i| RoomRow {
            room_id: RoomId::new(format!("fixture-room-{i}")),
            name: format!("Fixture room {i}"),
            standing: Standing::Active,
            live: false,
            role: Role::Member,
            member_count: 1,
            last_event: LastEvent::Absent,
            capabilities: caps.clone(),
        })
        .collect()
}

/// The capability list for the `?caps=1` offline fixture (web only, marker-gated).
/// When armed, every action token is present so the #180 e2e suite can verify
/// that capability-gated affordances appear (D6 positive case). Returns an empty
/// list everywhere else, so the default fixture (affordances absent, D6 negative
/// case) is unchanged.
fn fixture_caps() -> Vec<CapabilityToken> {
    #[cfg(feature = "web")]
    {
        let Some(window) = web_sys::window() else {
            return Vec::new();
        };
        let armed = window
            .local_storage()
            .ok()
            .flatten()
            .and_then(|storage| storage.get_item(BOOT_FIXTURE_MARKER).ok().flatten())
            .as_deref()
            == Some("1");
        if !armed {
            return Vec::new();
        }
        let Ok(search) = window.location().search() else {
            return Vec::new();
        };
        let has_caps = search
            .trim_start_matches('?')
            .split('&')
            .any(|pair| pair == "caps=1");
        if has_caps {
            vec![
                CapabilityToken::InviteMint,
                CapabilityToken::InviteRevoke,
                CapabilityToken::MemberRemove,
                CapabilityToken::RoomLeave,
                CapabilityToken::RoomActivate,
            ]
        } else {
            Vec::new()
        }
    }
    #[cfg(not(feature = "web"))]
    {
        Vec::new()
    }
}

/// A deterministic reference instant for the offline fixtures (the mock has no
/// clock; the value is cosmetic).
fn fixture_ts() -> Timestamp {
    Timestamp::new(time::OffsetDateTime::UNIX_EPOCH)
}

/// A reference roster: an authority and an ordinary member (the second is also
/// an agent in `fixture_fleet`, exercising the derived agent marker).
fn fixture_members() -> RoomMembersOut {
    RoomMembersOut {
        room_id: RoomId::new("fixture-room-0"),
        members: vec![
            MemberRow {
                subject_id: SubjectId::new("blake3:fixture-authority"),
                role: Role::Authority,
                standing: Standing::Active,
                joined_at: fixture_ts(),
            },
            MemberRow {
                subject_id: SubjectId::new("blake3:fixture-agent"),
                role: Role::Member,
                standing: Standing::Active,
                joined_at: fixture_ts(),
            },
        ],
    }
}

/// A reference invitations page: one outstanding, one expired (offers Revoke and
/// Re-invite respectively).
fn fixture_invites() -> InviteListOut {
    InviteListOut {
        room_id: RoomId::new("fixture-room-0"),
        invites: vec![
            InviteRow {
                invite_id: InviteId::new("inv-outstanding"),
                subject_id: SubjectId::new("blake3:fixture-invitee"),
                role: Role::Member,
                expires_at: fixture_ts(),
                redeemability: Redeemability::Outstanding,
            },
            InviteRow {
                invite_id: InviteId::new("inv-expired"),
                subject_id: SubjectId::new("blake3:fixture-expired"),
                role: Role::Member,
                expires_at: fixture_ts(),
                redeemability: Redeemability::Expired,
            },
        ],
        truncated: Truncated::Complete,
    }
}

/// A reference fleet: a working agent and a blocked one (needs-a-person).
fn fixture_fleet() -> FleetListOut {
    FleetListOut {
        agents: vec![
            FleetRow {
                subject_id: SubjectId::new("blake3:fixture-agent"),
                room_id: RoomId::new("fixture-room-0"),
                liveness: Liveness::Working,
                latest_status: LatestStatus::Present {
                    label: StatusLabel::Working,
                    at: fixture_ts(),
                },
                last_seen: LastSeen::Present { at: fixture_ts() },
            },
            FleetRow {
                subject_id: SubjectId::new("blake3:fixture-blocked"),
                room_id: RoomId::new("fixture-room-0"),
                liveness: Liveness::Stale,
                latest_status: LatestStatus::Present {
                    label: StatusLabel::Blocked,
                    at: fixture_ts(),
                },
                last_seen: LastSeen::Absent,
            },
        ],
    }
}

/// A reference peers answer for a live room.
fn fixture_peers() -> RoomPeersOut {
    RoomPeersOut {
        room_id: RoomId::new("fixture-room-0"),
        reachability: Reachability::Connected,
        peers: vec![PeerRow {
            subject_id: SubjectId::new("blake3:fixture-agent"),
            device_id: DeviceId::new("blake3:fixture-device"),
            link: Link::Direct {
                since: fixture_ts(),
            },
        }],
    }
}

/// The room count a `?rooms=N` query parameter requests (a11y matrix populated-shell
/// fixture; web only, gated on the SAME `localStorage` marker as `?boot=`, so
/// production never arms it). `0` everywhere else and for any absent/invalid value,
/// and capped so a hand-typed `?rooms=99999` cannot bloat the fixture.
fn rooms_fixture_count() -> usize {
    #[cfg(feature = "web")]
    {
        let Some(window) = web_sys::window() else {
            return 0;
        };
        let armed = window
            .local_storage()
            .ok()
            .flatten()
            .and_then(|storage| storage.get_item(BOOT_FIXTURE_MARKER).ok().flatten())
            .as_deref()
            == Some("1");
        if !armed {
            return 0;
        }
        let Ok(search) = window.location().search() else {
            return 0;
        };
        search
            .trim_start_matches('?')
            .split('&')
            .find_map(|pair| pair.strip_prefix("rooms="))
            .and_then(|n| n.parse::<usize>().ok())
            .map(|n| n.min(50))
            .unwrap_or(0)
    }
    #[cfg(not(feature = "web"))]
    {
        0
    }
}

/// Install a marker-gated e2e hook — `window.__jeliyaE2eConnState(state)` — that drives
/// the mock's connection LIFECYCLE, so the a11y matrix can verify the connection
/// live-region's announce-once wiring against a REAL `Interrupted`/`Ready` transition
/// (the announcer reacts to each `StateChanged` EVENT, so no DOM poke can substitute for
/// a driven transition). Gated on the SAME `localStorage` marker as the `?boot=` fixture,
/// so production — loopback or not — never arms it; the code is inert without the marker.
/// `MockController::set_state` panics on `Stopping`/`Stopped`, so only `interrupted`,
/// `ready`, and `failed` are accepted; anything else is ignored.
#[cfg(feature = "web")]
fn install_e2e_connection_hook(controller: Rc<MockController>) {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};

    let Some(window) = web_sys::window() else {
        return;
    };
    let armed = window
        .local_storage()
        .ok()
        .flatten()
        .and_then(|storage| storage.get_item(BOOT_FIXTURE_MARKER).ok().flatten())
        .as_deref()
        == Some("1");
    if !armed {
        return;
    }
    let closure = Closure::<dyn Fn(String)>::new(move |state: String| {
        let state = match state.as_str() {
            "interrupted" => State::Interrupted,
            "ready" => State::Ready,
            "failed" => State::Failed,
            _ => return,
        };
        controller.set_state(state);
    });
    let _ = js_sys::Reflect::set(
        &window,
        &JsValue::from_str("__jeliyaE2eConnState"),
        closure.as_ref().unchecked_ref(),
    );
    // Leak the closure so the hook stays callable for the page's lifetime. This runs
    // ONLY under the e2e marker, so it is never a production leak path.
    closure.forget();
}

/// The platform UI language tag to seed locale resolution with — the ONE place
/// a target reads it (Decision-5): the browser's language preference on the web
/// target, `None` elsewhere (the M4 desktop/Android bins inject their own OS
/// locale here later). Confining the `web-sys` read to composition keeps
/// [`crate::AppRoot`] and every shared component free of `web-sys`/`cfg`.
///
/// Reads the ORDERED `navigator.languages` list, not just `navigator.language`:
/// a browser configured as `['de-DE', 'fr-FR']` must reach French (its next
/// SUPPORTED preference), not fall to English on the unsupported primary tag. We
/// return the first tag whose primary subtag this app supports; `navigator.language`
/// is the fallback, then `None`. The narrowing to a supported catalog still
/// happens in `LocaleState::resolve` — this only chooses WHICH platform tag to
/// feed it.
fn platform_locale() -> Option<String> {
    #[cfg(feature = "web")]
    {
        let navigator = web_sys::window()?.navigator();
        // `navigator.languages` is the user's ordered preference list; pick the
        // first entry this app can actually render (so a supported non-primary
        // preference is honored before the English fallback), else fall back to
        // the primary `navigator.language` tag.
        let languages = navigator.languages();
        let ordered = (0..languages.length()).filter_map(|i| languages.get(i).as_string());
        first_supported_language_tag(ordered).or_else(|| navigator.language())
    }
    #[cfg(not(feature = "web"))]
    {
        None
    }
}

/// The first tag in `tags` whose primary subtag this app supports, else `None`.
/// The ordered-preference selection behind [`platform_locale`], factored out pure
/// so it is testable without a browser: `['de-DE', 'fr-FR']` yields `fr-FR` (the
/// user's next SUPPORTED preference), not the unsupported German primary that
/// would otherwise fall through to English. The narrowing to a catalog still
/// happens in `LocaleState::resolve`; this only chooses which tag to feed it.
#[cfg(any(feature = "web", test))]
fn first_supported_language_tag(tags: impl Iterator<Item = String>) -> Option<String> {
    tags.into_iter()
        .find(|tag| crate::l10n::Locale::from_tag(tag).is_some())
}

/// Set `<html lang>` to `tag`, web target only. Injected into `AppRoot` as the
/// `on_locale_lang` callback so the document language tracks the resolved TEXT
/// locale REACTIVELY (mount and any live switch), while the `web-sys` access stays
/// confined here — never in the shared `AppRoot` (Decision-3; #177 §5.1). `AppRoot`
/// owns the locale resolution and hands the resolved tag to this setter.
#[cfg(feature = "web")]
fn set_document_lang(tag: &str) {
    if let Some(element) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = element.set_attribute("lang", tag);
    }
}

/// How many scripted mount reads the native composition carries — exactly the
/// `room.list` reply today. (The web root now pumps continuously — #180 — so
/// this fixed count applies only to the `native` stub.)
#[cfg(feature = "native")]
const SCRIPTED_MOUNT_READS: usize = 1;

/// Await each dispatch and settle it, `expected` times: the event-driven
/// pump the native root uses for its fixed mount reads.
#[cfg(feature = "native")]
async fn drive_scripted_replies(controller: &MockController, expected: usize) {
    for _ in 0..expected {
        controller.pending_call().await;
        while controller.deliver_next() {}
    }
}

/// Continuously settle scripted replies as the app dispatches them (#180): park
/// on `pending_call` until a call is pending, deliver every currently
/// deliverable reply, and repeat. On `stop()` `pending_call` resolves with
/// nothing deliverable, ending the pump — so it never outlives the backend and
/// never busy-loops (the reference script contains no hanging programs). This
/// lets navigation-time pane reads settle without guessing a pass count.
async fn pump_scripted_replies(controller: &MockController) {
    loop {
        controller.pending_call().await;
        let mut delivered = false;
        while controller.deliver_next() {
            delivered = true;
        }
        // Nothing deliverable after a wake means the backend is stopping (no
        // scripted program hangs), so the pump is done.
        if !delivered {
            break;
        }
    }
}

/// The native (system-WebView, M4: #186–#189) composition seam. This M3 slice
/// defines the **boundary** only: the same deterministic mock behind the
/// [`ClientHandle`], and the canonical `jeliya-platform` desktop-shaped fake
/// (#174) standing in until M4's target implementations land. M4 replaces the
/// internals — the live adapter behind the same handle, the desktop/Android
/// [`PlatformServices`], its renderer and `bin` — **at this seam**, without
/// inventing the target-selection boundary.
#[cfg(feature = "native")]
#[derive(Clone)]
pub struct NativeComposition {
    /// The client seam handed to [`crate::AppRoot`].
    pub handle: ClientHandle,
    /// The platform-authority seam handed to [`crate::AppRoot`], injected
    /// separately from `handle`.
    pub services: PlatformServices,
    /// The mock controller driving the reference backend until the live
    /// adapter (#171–#173) replaces it behind the same handle.
    pub controller: Rc<MockController>,
}

/// Build the native seam stub (see [`NativeComposition`]). The services are
/// the same deterministic in-memory implementation the browser composition
/// uses today — a stand-in this function exists to let M4 swap, not a claim
/// that native platform authority is implemented here.
#[cfg(feature = "native")]
pub fn native_composition() -> NativeComposition {
    let (handle, controller) = MockScript::new()
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&RoomListOut { rooms: Vec::new() }),
        )
        .build();
    NativeComposition {
        handle,
        // The desktop-shaped deterministic fake stands in until M4's real
        // desktop/Android services land behind the same facade — persistent
        // preferences, window actions, native-path sources.
        services: PlatformServices::fake_desktop(),
        controller: Rc::new(controller),
    }
}

/// The native root: identical deterministic drive to [`WebRoot`]; the M4
/// system-WebView shell mounts this through its own renderer and replaces the
/// composition internals at [`native_composition`].
#[cfg(feature = "native")]
#[component]
pub fn NativeRoot() -> Element {
    let composition = use_hook(native_composition);
    let handle = composition.handle.clone();
    let services = composition.services.clone();

    use_future(move || {
        let composition = composition.clone();
        async move {
            composition.handle.start();
            composition.controller.set_state(State::Ready);
            // Same event-driven delivery contract as WebRoot.
            drive_scripted_replies(&composition.controller, SCRIPTED_MOUNT_READS).await;
        }
    });

    rsx! {
        AppRoot { handle, services, platform_locale: platform_locale() }
    }
}

#[cfg(test)]
mod tests {
    use super::first_supported_language_tag;

    #[test]
    fn first_supported_language_tag_honors_the_ordered_preference() {
        let of = |v: &[&str]| first_supported_language_tag(v.iter().map(|s| s.to_string()));
        // Unsupported primary, supported secondary → the secondary (not English).
        assert_eq!(of(&["de-DE", "fr-FR"]), Some("fr-FR".to_string()));
        // The first SUPPORTED tag wins.
        assert_eq!(of(&["en-US", "fr-FR"]), Some("en-US".to_string()));
        assert_eq!(of(&["fr", "en"]), Some("fr".to_string()));
        // No supported entry → None (caller falls back to navigator.language).
        assert_eq!(of(&["de-DE", "es-ES"]), None);
        assert_eq!(of(&[]), None);
    }
}
