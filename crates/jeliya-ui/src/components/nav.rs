//! The navigation landmark primitive (§5.6).
//!
//! A named `nav` so landmark navigation can tell panes apart (the checklist
//! requires named landmarks where more than one of a role can exist). Routing
//! all navigation through this primitive is what lets the literal/structure scan
//! forbid an ad-hoc unnamed `nav` elsewhere (Decision-6).
//!
//! When `role_tablist` is true the children are wrapped in a `<div role="tablist">`
//! so the strip can carry ARIA tab roles (WAI-ARIA tabs pattern).

use dioxus::prelude::*;

/// A named navigation landmark. `label` becomes the accessible name
/// (`aria-label`); `tabindex="-1"` makes it a skip-link focus target.
///
/// When `role_tablist` is true the children are rendered inside a
/// `<div role="tablist">` so the strip can carry ARIA tab roles
/// (WAI-ARIA tabs pattern) — Finding 3.
#[component]
pub fn NavLandmark(
    id: String,
    label: String,
    #[props(default)] class: String,
    #[props(default)] role_tablist: bool,
    children: Element,
) -> Element {
    if role_tablist {
        rsx! {
            nav { id, "aria-label": "{label}", tabindex: "-1",
                div { role: "tablist", class, {children} }
            }
        }
    } else {
        rsx! {
            nav { class, id, "aria-label": "{label}", tabindex: "-1", {children} }
        }
    }
}
