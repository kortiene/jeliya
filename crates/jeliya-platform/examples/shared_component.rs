//! The verification component for #174 (AC-5/§7).
//!
//! A minimal shared Dioxus component that reaches files, URLs, clipboard,
//! navigation, and window actions **only** through an injected
//! [`PlatformServices`], compiled against the deterministic fakes for **both**
//! `wasm32-unknown-unknown` and a native target, with **no per-component `cfg`
//! logic** — this file contains no `cfg` token at all. Which shape backs the
//! services (browser / desktop / android) is a composition choice; the
//! component renders capability facts identically regardless.
//!
//! It is built (never run) by CI on both targets:
//!
//! ```text
//! cargo build --locked -p jeliya-platform --example shared_component --features example
//! cargo build --locked -p jeliya-platform --example shared_component --features example \
//!     --target wasm32-unknown-unknown
//! ```
//!
//! The example is gated by `required-features = ["example"]` (which enables
//! `fake` plus the optional `dioxus`), so the heavy Dioxus tree only compiles
//! in these explicit steps. Dioxus is used only for RSX and `#[component]`; no
//! renderer runs.

use dioxus::prelude::*;
use jeliya_platform::fake;
use jeliya_platform::{PlatformServices, Route, SafeExternalUrl};

fn main() {
    // Build-only verification: construct the shared component against each
    // fake shape and build the virtual DOM once. No renderer runs; the point is
    // that the same component type-checks and links against all three shapes on
    // this target, with no `cfg`.
    for services in [fake::browser().0, fake::desktop().0, fake::android().0] {
        let mut dom =
            VirtualDom::new_with_props(CapabilityPanel, CapabilityPanelProps { services });
        dom.rebuild_in_place();
    }
}

/// A minimal shared component: it reads a few platform facts and drives a few
/// actions through the injected [`PlatformServices`], with no `cfg` anywhere.
///
/// The same source compiles for native and wasm; the component never names a
/// target and never reaches platform authority except through `services`.
#[component]
fn CapabilityPanel(services: PlatformServices) -> Element {
    // Read platform facts through the injected seam — never a `cfg(target_…)`.
    let durability = format!("{:?}", services.preferences().durability());
    let window_available = services.window().availability().is_available();
    let route = services.navigation().route();
    let route_label = match route {
        Route::Root => "root".to_string(),
        Route::Room { room_id } => format!("room {room_id}"),
        Route::Settings => "settings".to_string(),
    };

    // Drive a couple of actions through the seam. Failures are surfaced, never
    // swallowed — a component branches on the outcome, not on the platform.
    let clipboard_note = match services.clipboard().write_text("diagnostics") {
        Ok(()) => "copied".to_string(),
        Err(error) => format!("copy failed: {error}"),
    };
    let launch_note = match SafeExternalUrl::parse("https://jeliya.example") {
        Ok(url) => match services.url_launcher().open_external(url) {
            Ok(()) => "opened".to_string(),
            Err(error) => format!("open failed: {error}"),
        },
        Err(_) => "rejected".to_string(),
    };

    rsx! {
        section {
            p { "durability: {durability}" }
            p { "window actions: {window_available}" }
            p { "route: {route_label}" }
            p { "clipboard: {clipboard_note}" }
            p { "launch: {launch_note}" }
        }
    }
}
