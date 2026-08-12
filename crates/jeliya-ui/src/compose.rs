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
use jeliya_api::{RoomList, RoomListOut};
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
}

/// Build the browser composition: the deterministic mock behind a
/// [`ClientHandle`], the browser-appropriate [`PlatformServices`], and the
/// controller that settles the mock's reads.
///
/// The mock scripts an empty `room.list` reply — the honest state of a fresh
/// daemon and enough to render the shell. Timeline, membership, and the
/// composer are the Room Workbench port (a later M3 slice), out of scope here.
pub fn web_composition() -> WebComposition {
    let (handle, controller) = MockScript::new()
        .on(
            "room.list",
            Program::reply_ok::<RoomList>(&RoomListOut { rooms: Vec::new() }),
        )
        .build();
    WebComposition {
        handle,
        // The browser-shaped deterministic fake from the canonical
        // `jeliya-platform` contract (#174): session-scoped preferences, no
        // window actions, browser-blob sources. M3's live web-sys services
        // replace it behind the unchanged facade.
        services: PlatformServices::fake_browser(),
        controller: Rc::new(controller),
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

    // Set `<html lang>` once from the resolved locale (web target only). A
    // `use_hook` runs exactly once on mount — the right place for a one-shot
    // boot side effect that must not re-run every render.
    #[cfg(feature = "web")]
    {
        let lang_services = composition.services.clone();
        use_hook(move || apply_document_lang(&lang_services, platform_locale().as_deref()));
    }

    use_future(move || {
        let composition = composition.clone();
        async move {
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
                    drive_scripted_replies(&composition.controller, SCRIPTED_MOUNT_READS).await;
                }
            }
        }
    });

    rsx! {
        AppRoot { handle, services, platform_locale: platform_locale() }
    }
}

/// The lifecycle a `?boot=<state>` query parameter requests, for the a11y matrix's
/// deterministic boot/terminal fixture (web target only; `None` everywhere else
/// and for any absent/unrecognized value → the normal Ready shell).
fn boot_fixture_state() -> Option<State> {
    #[cfg(feature = "web")]
    {
        let search = web_sys::window()?.location().search().ok()?;
        // A tiny hand-parse (no url crate): find `boot=` in the query string.
        let value = search
            .trim_start_matches('?')
            .split('&')
            .find_map(|pair| pair.strip_prefix("boot="))?;
        match value {
            "connecting" => Some(State::Connecting),
            "stopping" => Some(State::Stopping),
            "stopped" => Some(State::Stopped),
            "failed" => Some(State::Failed),
            _ => None,
        }
    }
    #[cfg(not(feature = "web"))]
    {
        None
    }
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

/// Set `<html lang>` from the resolved TEXT locale (the same resolution
/// `AppRoot` performs), web target only. Called once at composition so the
/// document element reports the page's actual language instead of index.html's
/// static `en` (#177 §5.1). Confined here — never in a shared component — so no
/// `web-sys`/`cfg` leaks into `AppRoot`. A reactive re-set on a live locale
/// switch rides with that later slice; the foundation has no switch UI yet.
#[cfg(feature = "web")]
fn apply_document_lang(services: &PlatformServices, platform: Option<&str>) {
    use jeliya_platform::PreferenceKey;
    let preferences = services.preferences();
    let text = preferences.get(&PreferenceKey::TextLocale);
    let formatting = preferences.get(&PreferenceKey::FormattingLocale);
    let state = crate::l10n::LocaleState::resolve(
        text.as_deref(),
        formatting.as_deref(),
        platform,
        platform,
    );
    if let Some(element) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = element.set_attribute("lang", state.text.tag());
    }
}

/// How many scripted mount reads the compositions carry — exactly the
/// `room.list` reply today. A new scripted read must bump this in lockstep,
/// or the driver stops delivering before the new read settles.
const SCRIPTED_MOUNT_READS: usize = 1;

/// Await each dispatch and settle it, `expected` times: the event-driven
/// pump both roots share.
async fn drive_scripted_replies(controller: &MockController, expected: usize) {
    for _ in 0..expected {
        controller.pending_call().await;
        while controller.deliver_next() {}
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
