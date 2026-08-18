//! The room shell (#178 §5.F): the room header + destination strip. Activity
//! (#179, [`super::activity::ActivityPane`]) and, from #180, the **People** and
//! **Agents** destinations ([`super::people::PeoplePane`] /
//! [`super::agents::AgentsPane`]) render real content; Files/Pipes (#181) are
//! still per-destination skeletons.
//!
//! A route naming an unreachable/departed room renders the recoverable state
//! ([`RoomUnavailable`]) with Rooms as the way out — never a blank panel (D7 /
//! contract rule 6).

use dioxus::prelude::*;
use jeliya_api::{RoomId, RoomRow, SubjectId, Timestamp};
use jeliya_client::ClientHandle;
use jeliya_platform::navigation::{RoomDest, Route};

use super::agents::AgentsPane;
use super::people::PeoplePane;
use super::{ActivityPane, NavLandmark};
use crate::l10n::use_strings;
use crate::shell::router::NavIntent;
use crate::shell::Shell;
use crate::PlatformServices;

/// A short, human-scannable disambiguator for a room id (the header's
/// short-id): the last segment after any `:` prefix, truncated.
fn short_id(room_id: &RoomId) -> String {
    let raw = room_id.as_str();
    let tail = raw.rsplit(':').next().unwrap_or(raw);
    tail.chars().take(8).collect()
}

/// Whether `dest` is the same room destination as `kind`, ignoring any selected
/// item (the strip highlights the People tab whether or not a member is open).
fn dest_kind_matches(dest: &RoomDest, kind: &RoomDest) -> bool {
    matches!(
        (dest, kind),
        (RoomDest::Activity, RoomDest::Activity)
            | (RoomDest::People { .. }, RoomDest::People { .. })
            | (RoomDest::Agents { .. }, RoomDest::Agents { .. })
            | (RoomDest::Files { .. }, RoomDest::Files { .. })
            | (RoomDest::Pipes { .. }, RoomDest::Pipes { .. })
    )
}

/// The room shell for a reachable room: header + destination strip + the
/// destination pane (People/Agents content, or a skeleton for the rest).
#[component]
pub fn RoomShell(
    room: RoomRow,
    dest: RoomDest,
    navigate: Callback<NavIntent>,
    handle: ClientHandle,
    services: PlatformServices,
    now: Timestamp,
    #[props(default)] self_id: Option<SubjectId>,
    /// The responsive shell (the Activity composer's keyboard fork; #178 §8).
    #[props(default = Shell::Wide)]
    shell: Shell,
) -> Element {
    let strings = use_strings();
    let nav_label = strings.room_nav_label().to_string();
    let skeleton = strings.room_dest_skeleton();
    let room_id = room.room_id.clone();
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
            RoomDest::People { item: None },
        ),
        (
            "room-dest-agents",
            strings.room_dest_agents(),
            RoomDest::Agents { item: None },
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
            // The destination pane.
            match dest.clone() {
                RoomDest::Activity => rsx! {
                    ActivityPane {
                        handle: handle.clone(),
                        services: services.clone(),
                        room_id: room_id.clone(),
                        capabilities: room.capabilities.clone(),
                        shell,
                        self_id: self_id.clone(),
                    }
                },
                RoomDest::People { item } => rsx! {
                    PeoplePane {
                        room: room.clone(),
                        item,
                        handle: handle.clone(),
                        services: services.clone(),
                        navigate,
                        now,
                        self_id: self_id.clone(),
                    }
                },
                RoomDest::Agents { item } => rsx! {
                    AgentsPane {
                        room: room.clone(),
                        item,
                        handle: handle.clone(),
                        services: services.clone(),
                        navigate,
                        self_id: self_id.clone(),
                    }
                },
                // Files/Pipes (#181) remain skeletons.
                _ => rsx! {
                    div { class: "room-pane-skeleton muted", id: "room-pane-skeleton", "{skeleton}" }
                },
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
        assert!(dest_kind_matches(
            &RoomDest::People {
                item: Some(SubjectId::new("m-1")),
            },
            &RoomDest::People { item: None },
        ));
        assert!(!dest_kind_matches(
            &RoomDest::People { item: None },
            &RoomDest::Agents { item: None },
        ));
    }

    #[test]
    fn short_id_takes_the_tail_after_a_colon() {
        assert_eq!(short_id(&RoomId::new("blake3:abcdefghmore")), "abcdefgh");
        assert_eq!(short_id(&RoomId::new("r-1")), "r-1");
    }
}
