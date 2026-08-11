//! Shared, renderer-agnostic RSX components.
//!
//! **No platform business-logic `cfg` forks live here** (architecture
//! Decision 3 / #174). A component that needs a platform capability takes it as
//! a prop or an injected service; it never branches on `cfg`. Target
//! differences live only in [`crate::compose`] and the per-target `bin`. As a
//! result this whole module compiles identically for native and `wasm32`.
//!
//! Every class name is the one the shared stylesheet (`ui/src/styles.css`,
//! consumed canonically per §7) already styles, so the design system drives
//! this markup unchanged — the property the #158 spike measured byte-identical.

use dioxus::prelude::*;
use jeliya_api::RoomRow;

/// The full-viewport status cover shown when the shell is not mounted:
/// initial activation ("connecting…") and the stop/failure states, each with
/// its own honest label — never a generic "connecting" over a terminal state.
/// Any recorded notice renders INSIDE the cover: `.boot-screen` fills the
/// viewport (`height: var(--vh-full)`), so a sibling would start below the
/// fold and the one diagnostic that explains a failure state would need a
/// scroll nobody knows to perform.
#[component]
pub fn BootScreen(target: String, notice: Option<String>) -> Element {
    rsx! {
        main { class: "boot-screen", id: "boot-screen",
            h1 { "Jeliya" }
            p { class: "boot-target mono", "{target}" }
            if let Some(notice) = notice.as_ref() {
                div { class: "error-note", id: "notice", "{notice}" }
            }
        }
    }
}

/// One room row in the sidebar. Renders the room's label and id using the
/// shared `.room-item` styling; selection and actions are later slices.
#[component]
pub fn RoomListItem(room: RoomRow, selected: bool) -> Element {
    let class = if selected {
        "room-item selected"
    } else {
        "room-item"
    };
    let label = if room.name.trim().is_empty() {
        "Untitled room".to_string()
    } else {
        room.name.clone()
    };
    rsx! {
        div { class: "{class}", "data-room": "{room.room_id}",
            // `.room-select` carries the row's padding, flex growth, and
            // alignment in the canonical stylesheet; `.room-item` is only
            // the border/hover container. Mirrors the React shell's
            // `button.room-select`; selection wiring is a later slice — the
            // element exists here for the design-system layout.
            button { class: "room-select",
                span { class: "room-info",
                    span { class: "room-name-line",
                        span { class: "room-name", "{label}" }
                    }
                }
            }
        }
    }
}

/// The empty-center placeholder shown when no room is open.
#[component]
pub fn EmptyCenter() -> Element {
    rsx! {
        div { class: "center-empty", id: "center-empty",
            p { class: "center-empty-title", "Choose a room" }
        }
    }
}

/// A small footer reporting the observable client lifecycle, so the shell is
/// honest about connection state without polling.
#[component]
pub fn StatusFooter(lifecycle: String) -> Element {
    rsx! {
        p { class: "boot-target mono", id: "status-footer", "client · {lifecycle}" }
    }
}
