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
//! The mock is driven to `Ready` and its scripted mount reads are settled with
//! a bounded, cooperative pump (no wall clock, no busy loop). The real browser
//! transport (`WsWeb`, #168) replaces [`web_composition`]'s mock with the live
//! adapter behind the same handle; nothing else in this crate changes.

use std::future::poll_fn;
use std::rc::Rc;
use std::task::Poll;

use dioxus::prelude::*;
use jeliya_api::{RoomList, RoomListOut};
use jeliya_client::mock::{MockController, MockScript, Program};
use jeliya_client::{ClientHandle, State};

use crate::app::AppRoot;
use crate::services::PlatformServices;

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
        services: PlatformServices::web_default(),
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
            // the shell leaves the boot state, then settle the eager-dispatched
            // mount reads over a bounded number of cooperative passes. There is
            // no live push source in this foundation slice, so this terminates.
            composition.handle.start();
            composition.controller.set_state(State::Ready);
            for _ in 0..8 {
                while composition.controller.deliver_next() {}
                yield_once().await;
            }
        }
    });

    rsx! {
        AppRoot { handle, services }
    }
}

/// Yield control back to the executor exactly once, then resume. Lets the
/// cooperatively-scheduled `AppRoot` future dispatch its reads between the
/// driver's settle passes, with no wall clock and no busy loop.
async fn yield_once() {
    let mut yielded = false;
    poll_fn(move |cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}
