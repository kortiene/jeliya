//! The global-destination navigation (#178 §5.F / §8): Rooms / Agent Fleet /
//! Settings, rendered through the [`NavLandmark`](crate::components::NavLandmark)
//! primitive (Decision-6), with the active destination derived from the current
//! [`Route`].
//!
//! Placement is the one Rust element-identity fork (D6): a persistent **rail**
//! on wide/medium, a compact **bottom bar** carrying only global destinations.
//! The choice is driven by the injected [`Shell`] value, never a `cfg` — and the
//! two are *different elements* (rendering both and hiding one would put two nav
//! landmarks in the a11y tree).

use dioxus::prelude::*;
use jeliya_platform::navigation::Route;

use crate::components::NavLandmark;
use crate::l10n::use_strings;
use crate::shell::router::NavIntent;
use crate::shell::Shell;

/// Whether `route` is under the Rooms global destination (the list itself, or
/// any room-scoped route). The Rooms destination owns every `/rooms…` path.
fn rooms_is_active(route: &Route) -> bool {
    matches!(route, Route::Rooms | Route::Room { .. })
}

/// The global-destination navigation strip. `route` is the current route (for
/// the active-destination highlight), `navigate` performs a pushing navigation,
/// and `shell` selects rail vs. bottom-bar placement.
#[component]
pub fn GlobalNav(route: Route, navigate: Callback<NavIntent>, shell: Shell) -> Element {
    let strings = use_strings();
    let label = strings.nav_global_label().to_string();
    let rooms = strings.dest_rooms();
    let fleet = strings.dest_fleet();
    let settings = strings.dest_settings();

    // The single element-identity fork: the compact bottom bar vs. the
    // wide/medium rail. A class distinguishes them for the canonical stylesheet;
    // both are the SAME `NavLandmark` element so there is one nav landmark, never
    // two (the a11y-tree invariant §8).
    let class = if shell.uses_bottom_bar() {
        "global-nav global-nav-bottom"
    } else {
        "global-nav global-nav-rail"
    };

    let rooms_active = rooms_is_active(&route);
    let fleet_active = matches!(route, Route::Fleet);
    let settings_active = matches!(route, Route::Settings);

    rsx! {
        NavLandmark { class: class.to_string(), id: "global-nav".to_string(), label,
            GlobalNavItem {
                id: "nav-rooms".to_string(),
                label: rooms.to_string(),
                active: rooms_active,
                target: Route::Rooms,
                navigate,
            }
            GlobalNavItem {
                id: "nav-fleet".to_string(),
                label: fleet.to_string(),
                active: fleet_active,
                target: Route::Fleet,
                navigate,
            }
            GlobalNavItem {
                id: "nav-settings".to_string(),
                label: settings.to_string(),
                active: settings_active,
                target: Route::Settings,
                navigate,
            }
        }
    }
}

/// One global-destination button. `aria-current="page"` marks the active
/// destination for assistive tech (a state, not colour-only).
#[component]
fn GlobalNavItem(
    id: String,
    label: String,
    active: bool,
    target: Route,
    navigate: Callback<NavIntent>,
) -> Element {
    let class = if active {
        "global-nav-item selected"
    } else {
        "global-nav-item"
    };
    let current = if active { "page" } else { "false" };
    rsx! {
        button {
            class: "{class}",
            id: "{id}",
            "aria-current": "{current}",
            onclick: move |_| navigate.call(NavIntent::push(target.clone())),
            "{label}"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_platform::navigation::RoomDest;

    #[test]
    fn rooms_destination_owns_every_room_scoped_route() {
        assert!(rooms_is_active(&Route::Rooms));
        assert!(rooms_is_active(&Route::Room {
            room_id: jeliya_api::RoomId::new("r-1"),
            dest: RoomDest::People,
        }));
        assert!(!rooms_is_active(&Route::Fleet));
        assert!(!rooms_is_active(&Route::Settings));
    }
}
