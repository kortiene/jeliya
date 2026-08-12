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
            composition.controller.set_state(State::Ready);
            drive_scripted_replies(&composition.controller, SCRIPTED_MOUNT_READS).await;
        }
    });

    rsx! {
        AppRoot { handle, services }
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
        AppRoot { handle, services }
    }
}
