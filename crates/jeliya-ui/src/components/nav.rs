//! The navigation landmark primitive (§5.6).
//!
//! A named `nav` so landmark navigation can tell panes apart (the checklist
//! requires named landmarks where more than one of a role can exist). Routing
//! all navigation through this primitive is what lets the literal/structure scan
//! forbid an ad-hoc unnamed `nav` elsewhere (Decision-6).

use dioxus::prelude::*;

/// A named navigation landmark. `label` becomes the accessible name
/// (`aria-label`); `tabindex="-1"` makes it a skip-link focus target.
#[component]
pub fn NavLandmark(
    id: String,
    label: String,
    #[props(default)] class: String,
    children: Element,
) -> Element {
    rsx! {
        nav { class, id, "aria-label": "{label}", tabindex: "-1", {children} }
    }
}
