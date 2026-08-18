//! The Agent Fleet destination skeleton (#178 §5.F).
//!
//! The `/fleet` destination exists and routes; its actionable-attention
//! **content** is #180. This renders only the landmarked heading and a truthful
//! "not available yet" placeholder — never a blank panel (D7).

use dioxus::prelude::*;

use crate::components::Heading;
use crate::l10n::use_strings;

/// The Agent Fleet destination pane (skeleton).
#[component]
pub fn FleetPane() -> Element {
    let strings = use_strings();
    let heading = strings.fleet_heading();
    let loading = strings.fleet_loading();
    rsx! {
        section { class: "fleet-pane", id: "fleet-pane",
            Heading { level: 2, class: "fleet-title".to_string(), "{heading}" }
            p { class: "muted", "{loading}" }
        }
    }
}
