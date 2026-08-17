//! The room-shell skeleton (#178 §5.F): the room header (room context, always
//! visible on every room-scoped surface) plus the room-destination navigation
//! strip. Pane **content** (Timeline/People/Files/Pipes) is #179–#181; this
//! renders the routing frame and a per-destination skeleton placeholder.
//!
//! A route naming an unreachable/departed room renders the recoverable state
//! ([`RoomUnavailable`]) with Rooms as the way out — never a blank panel (D7 /
//! contract rule 6).

use dioxus::prelude::*;
use jeliya_api::RoomId;
use jeliya_client::ClientHandle;
use jeliya_platform::navigation::{RoomDest, Route};

use crate::components::{FilesPane, NavLandmark, PipesPane};
use crate::l10n::use_strings;
use crate::shell::router::NavIntent;
use crate::PlatformServices;

/// A short, human-scannable disambiguator for a room id (the header's
/// short-id): the last segment after any `:` prefix, truncated. Never the room
/// name (that is content, #179); this is the always-visible id context.
fn short_id(room_id: &RoomId) -> String {
    let raw = room_id.as_str();
    let tail = raw.rsplit(':').next().unwrap_or(raw);
    tail.chars().take(8).collect()
}

/// Whether `dest` is the same room destination as `kind`, ignoring any selected
/// item (the strip highlights the Files tab whether or not a file is open).
fn dest_kind_matches(dest: &RoomDest, kind: &RoomDest) -> bool {
    matches!(
        (dest, kind),
        (RoomDest::Activity, RoomDest::Activity)
            | (RoomDest::People, RoomDest::People)
            | (RoomDest::Agents, RoomDest::Agents)
            | (RoomDest::Files { .. }, RoomDest::Files { .. })
            | (RoomDest::Pipes { .. }, RoomDest::Pipes { .. })
    )
}

/// The room shell for a reachable room: header + destination strip + the
/// per-destination pane. Files/Pipes are the #181 content panes; Activity/
/// People/Agents remain skeletons (#179/#180).
#[component]
pub fn RoomShell(
    room_id: RoomId,
    dest: RoomDest,
    navigate: Callback<NavIntent>,
    /// The client seam for the Files/Pipes panes' reads and flows.
    handle: ClientHandle,
    /// The platform-authority seam for the Files pane's pick/stage/export.
    services: PlatformServices,
    /// The served `max_shared_file_bytes` (spec D3/R1), threaded to the Files
    /// pane's preflight. `None` until the seam surfaces it (#171/#270).
    #[props(default)]
    max_shared_file_bytes: Option<u64>,
    /// Whether this room is a read-only archive (suppress share/fetch — spec D8).
    #[props(default)]
    read_only: bool,
) -> Element {
    let strings = use_strings();
    let nav_label = strings.room_nav_label().to_string();
    let skeleton = strings.room_dest_skeleton();
    let disambiguator = short_id(&room_id);

    let items = [
        (
            "room-dest-activity",
            strings.room_dest_activity(),
            RoomDest::Activity,
        ),
        (
            "room-dest-people",
            strings.room_dest_people(),
            RoomDest::People,
        ),
        (
            "room-dest-agents",
            strings.room_dest_agents(),
            RoomDest::Agents,
        ),
        (
            "room-dest-files",
            strings.room_dest_files(),
            RoomDest::Files { item: None },
        ),
        (
            "room-dest-pipes",
            strings.room_dest_pipes(),
            RoomDest::Pipes { item: None },
        ),
    ];

    rsx! {
        section { class: "room-shell", id: "room-shell",
            // The room header — room context, always visible. The short id is the
            // disambiguator until the room name lands (#179); it is a data
            // element (mono), not translated copy.
            header { class: "room-header", id: "room-header",
                span { class: "room-shortid mono", "{disambiguator}" }
            }
            NavLandmark {
                role_tablist: true,
                class: "room-nav".to_string(),
                id: "room-nav".to_string(),
                label: nav_label,
                for (id , label , target) in items {
                    RoomDestItem {
                        key: "{id}",
                        id: id.to_string(),
                        label: label.to_string(),
                        active: dest_kind_matches(&dest, &target),
                        room_id: room_id.clone(),
                        target,
                        navigate,
                    }
                }
            }
            // The per-destination pane. Files/Pipes are the #181 content panes;
            // Activity/People/Agents keep the #178 skeleton until #179/#180.
            {
                match &dest {
                    RoomDest::Files { item } => {
                        let selected = item.clone();
                        rsx! {
                            FilesPane {
                                handle: handle.clone(),
                                services: services.clone(),
                                room_id: room_id.clone(),
                                selected,
                                max_shared_file_bytes,
                                read_only,
                            }
                        }
                    }
                    RoomDest::Pipes { item } => {
                        let selected = item.clone();
                        rsx! {
                            PipesPane {
                                handle: handle.clone(),
                                services: Some(services.clone()),
                                room_id: room_id.clone(),
                                selected,
                                read_only,
                            }
                        }
                    }
                    _ => rsx! {
                        div { class: "room-pane-skeleton muted", id: "room-pane-skeleton", "{skeleton}" }
                    },
                }
            }
        }
    }
}

/// One room-destination tab. Navigating carries the room id and the destination,
/// so a tab is a real route change, not local state.
#[component]
fn RoomDestItem(
    id: String,
    label: String,
    active: bool,
    room_id: RoomId,
    target: RoomDest,
    navigate: Callback<NavIntent>,
) -> Element {
    let class = if active {
        "room-dest-item selected"
    } else {
        "room-dest-item"
    };
    let selected = if active { "true" } else { "false" };
    rsx! {
        button {
            role: "tab",
            "aria-selected": "{selected}",
            class: "{class}",
            id: "{id}",
            onclick: move |_| {
                navigate
                    .call(
                        NavIntent::push(Route::Room {
                            room_id: room_id.clone(),
                            dest: target.clone(),
                        }),
                    )
            },
            "{label}"
        }
    }
}

/// The recoverable state for a route naming an unreachable/departed room: the
/// plain fact plus a way back to Rooms — never a blank panel (contract rule 6).
#[component]
pub fn RoomUnavailable(navigate: Callback<NavIntent>) -> Element {
    let strings = use_strings();
    let message = strings.room_unavailable();
    let rooms = strings.dest_rooms();
    rsx! {
        section { class: "room-unavailable", id: "room-unavailable",
            p { class: "muted", "{message}" }
            button {
                class: "btn btn-sm",
                id: "room-unavailable-rooms",
                onclick: move |_| navigate.call(NavIntent::push(Route::Rooms)),
                "{rooms}"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dest_kind_ignores_selected_item() {
        assert!(dest_kind_matches(
            &RoomDest::Files {
                item: Some(jeliya_api::FileId::new("f-1")),
            },
            &RoomDest::Files { item: None },
        ));
        assert!(!dest_kind_matches(&RoomDest::People, &RoomDest::Agents));
    }

    #[test]
    fn short_id_takes_the_tail_after_a_colon() {
        assert_eq!(short_id(&RoomId::new("blake3:abcdefghmore")), "abcdefgh");
        assert_eq!(short_id(&RoomId::new("r-1")), "r-1");
    }
}
