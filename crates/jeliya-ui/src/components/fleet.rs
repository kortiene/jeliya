//! The Agent Fleet (global) destination (§7.3): `fleet.list` across every
//! authorized room, grouped by **attention severity**, with the **Live** filter,
//! **polled only while the destination is active and the document visible,
//! resuming once** (D7, AC-5).
//!
//! "Active" is structural: leaving `/fleet` unmounts this pane (the route *is*
//! the state), stopping the loop; so the poll controller only needs the document
//! **visibility**, folded from the `Lifecycle` bus. Becoming visible triggers
//! exactly one resume-fetch (the pure [`FleetPoll`](crate::view::poll::FleetPoll)
//! machine, unit-tested in `view/poll.rs`, governs this); backgrounding stops it.

use dioxus::prelude::*;
use futures::StreamExt;
use jeliya_api::FleetList;
use jeliya_client::ClientHandle;
use jeliya_platform::lifecycle::{LifecycleDelivery, LifecycleEvent};
use jeliya_platform::navigation::{RoomDest, Route};
use jeliya_platform::PreferenceKey;

use super::read::ready_call;
use super::{Heading, StatusIndicator};
use crate::l10n::{use_formats, use_strings};
use crate::shell::router::NavIntent;
use crate::status;
use crate::view::alias::AliasMap;
use crate::view::fleet::{fold_fleet, AttentionGroup, FleetFilter, FleetRowView, FleetView};
use crate::view::load::LoadState;
use crate::view::poll::{FleetPoll, PollAction};
use crate::PlatformServices;

/// The Agent Fleet destination pane.
#[component]
pub fn FleetPane(
    handle: ClientHandle,
    services: PlatformServices,
    navigate: Callback<NavIntent>,
    #[props(default)] self_id: Option<jeliya_api::SubjectId>,
) -> Element {
    let strings = use_strings();
    let fleet = use_signal(|| LoadState::<FleetView>::Loading);
    let filter = use_signal(|| FleetFilter::All);
    // Document visibility, folded from the lifecycle bus (default visible).
    let visible = use_signal(|| true);

    // Fold the lifecycle bus into `visible`.
    {
        let services = services.clone();
        let mut visible = visible;
        use_future(move || {
            let services = services.clone();
            async move {
                let mut sub = services.lifecycle().subscribe();
                while let Some(delivery) = sub.next().await {
                    if let LifecycleDelivery::Event(event) = delivery {
                        match event {
                            LifecycleEvent::Resumed => visible.set(true),
                            LifecycleEvent::Backgrounded { .. } => visible.set(false),
                            _ => {}
                        }
                    }
                }
            }
        });
    }

    // The poll: mounting means active (the route is Fleet); becoming visible
    // resumes exactly once. Re-runs when `visible` changes — a background→
    // foreground transition is one resume-fetch, backgrounding stops the loop.
    {
        let handle = handle.clone();
        let mut fleet = fleet;
        use_future(move || {
            let is_visible = visible();
            let handle = handle.clone();
            // The pure controller decides the action; mounting is the active
            // entry (was_running defaults false on this fresh machine).
            let mut machine = FleetPoll::new();
            let action = machine.update(true, is_visible);
            async move {
                match action {
                    PollAction::FetchOnceThenPoll | PollAction::Poll => {
                        fleet.set(LoadState::Loading);
                        match ready_call(&handle, FleetList {}).await {
                            Ok(out) => {
                                let view = fold_fleet(&out);
                                fleet.set(if view.is_empty() {
                                    LoadState::Empty
                                } else {
                                    LoadState::Loaded(view)
                                });
                            }
                            Err(err) => fleet.set(fleet_error(&err)),
                        }
                    }
                    PollAction::Stop => {}
                }
            }
        });
    }

    let aliases = AliasMap::parse(
        services
            .preferences()
            .get(&PreferenceKey::Aliases)
            .as_deref(),
    );
    let heading = strings.fleet_heading();
    let active_filter = filter();

    rsx! {
        section { class: "fleet-pane", id: "fleet-pane",
            Heading { level: 2, class: "fleet-title".to_string(), "{heading}" }

            // The Live filter (spans Working + Online).
            FilterToggle { filter, active: active_filter }

            match &*fleet.read() {
                LoadState::Loading => rsx! { p { class: "muted", id: "fleet-loading", "{strings.state_loading()}" } },
                LoadState::Empty => rsx! { p { class: "muted", id: "fleet-empty", "{strings.fleet_empty()}" } },
                LoadState::Offline => rsx! { p { class: "muted", id: "fleet-offline", "{strings.state_offline()}" } },
                LoadState::Unauthorized => rsx! { p { class: "muted", id: "fleet-unauth", "{strings.state_unauthorized()}" } },
                LoadState::Failed(_) => rsx! { p { class: "error-note", id: "fleet-failed", "{strings.state_failed_body()}" } },
                LoadState::Loaded(view) | LoadState::Stale(view) => {
                    let groups = view.grouped(active_filter);
                    if groups.is_empty() {
                        rsx! { p { class: "muted", id: "fleet-empty-filtered", "{strings.fleet_empty()}" } }
                    } else {
                        rsx! {
                            for (group , rows) in groups.iter() {
                                section { key: "{group:?}", class: "fleet-group",
                                    Heading {
                                        level: 3,
                                        class: "fleet-group-title".to_string(),
                                        "{attention_heading(strings, *group)}"
                                    }
                                    ul { class: "fleet-list",
                                        for row in rows.iter() {
                                            FleetRow {
                                                key: "{row.subject_id}-{row.room_id}",
                                                row: (*row).clone(),
                                                aliases: aliases.clone(),
                                                self_id: self_id.clone(),
                                                navigate,
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The attention group heading copy.
fn attention_heading(strings: &dyn crate::l10n::Catalog, group: AttentionGroup) -> &'static str {
    match group {
        AttentionGroup::NeedsPerson => strings.fleet_attention_needs_person(),
        AttentionGroup::Failed => strings.fleet_attention_failed(),
        AttentionGroup::Ok => strings.fleet_attention_ok(),
    }
}

/// Map a fleet read error to the truthful state.
fn fleet_error(err: &jeliya_client::CallError) -> LoadState<FleetView> {
    use crate::view::load::ReadOutcome;
    match ReadOutcome::classify(err, matches!(err, jeliya_client::CallError::Wire(_))) {
        ReadOutcome::Offline => LoadState::Offline,
        ReadOutcome::Unauthorized => LoadState::Unauthorized,
        ReadOutcome::Failed => LoadState::Failed(crate::l10n::FriendlyError {
            title: String::new(),
            message: String::new(),
        }),
    }
}

/// The All / Live filter toggle.
#[component]
fn FilterToggle(filter: Signal<FleetFilter>, active: FleetFilter) -> Element {
    let strings = use_strings();
    let mut filter = filter;
    let label = strings.fleet_filter_label();
    rsx! {
        div {
            class: "fleet-filter",
            role: "group",
            "aria-label": "{label}",
            button {
                r#type: "button",
                class: "btn btn-sm",
                id: "fleet-filter-all",
                "aria-pressed": "{active == FleetFilter::All}",
                onclick: move |_| filter.set(FleetFilter::All),
                "{strings.fleet_filter_all()}"
            }
            button {
                r#type: "button",
                class: "btn btn-sm",
                id: "fleet-filter-live",
                "aria-pressed": "{active == FleetFilter::Live}",
                onclick: move |_| filter.set(FleetFilter::Live),
                "{strings.fleet_filter_live()}"
            }
        }
    }
}

/// One fleet row: liveness and latest status as two separate facts (D10), with
/// the room disambiguator; selecting it deep-links to that room's Agents view.
#[component]
fn FleetRow(
    row: FleetRowView,
    aliases: AliasMap,
    self_id: Option<jeliya_api::SubjectId>,
    navigate: Callback<NavIntent>,
) -> Element {
    let strings = use_strings();
    let formats = use_formats();
    let resolved = aliases.resolve(&row.subject_id, self_id.as_ref());
    let name = status::resolved_name(strings, &resolved);
    let liveness = status::liveness_label(strings, row.liveness);
    let latest = status::latest_status_label(strings, &row.latest);
    let last_seen = status::last_seen_label(strings, formats, &row.last_seen);
    let room_short = short_room(&row.room_id);
    let tone = status::liveness_tone(row.liveness);
    let room_id = row.room_id.clone();

    rsx! {
        li { class: "fleet-row", "data-subject": "{row.subject_id}",
            button {
                r#type: "button",
                class: "fleet-select",
                onclick: move |_| {
                    navigate
                        .call(
                            NavIntent::push(Route::Room {
                                room_id: room_id.clone(),
                                dest: RoomDest::Agents { item: None },
                            }),
                        )
                },
                span { class: "fleet-name", "{name}" }
                span { class: "fleet-room muted", "{strings.fleet_room_label()} ",
                    span { class: "mono", "{room_short}" }
                }
                // Liveness (dot + label) and latest status are DISTINCT names.
                span { class: "fleet-liveness",
                    StatusIndicator { tone, label: liveness.to_string() }
                }
                span { class: "fleet-status", "{strings.agents_status_label()} {latest}" }
                span { class: "fleet-last-seen muted", "{strings.agents_last_seen_label()} {last_seen}" }
            }
        }
    }
}

/// The room's short-id disambiguator (homonym-safe — §11).
fn short_room(room_id: &jeliya_api::RoomId) -> String {
    let raw = room_id.as_str();
    let tail = raw.rsplit(':').next().unwrap_or(raw);
    tail.chars().take(8).collect()
}
