//! The Agents & Runs (room) destination (§7.2): the agents *in this room*
//! (`fleet.list` filtered by `room_id`, D9), each with liveness and latest
//! status as **two separate facts** (D10), and a selected agent's bounded run
//! history (`status.history`). This destination authors nothing (spawning is a
//! non-goal).

use dioxus::prelude::*;
use jeliya_api::{FleetList, Page, RoomRow, StatusHistory, SubjectId};
use jeliya_client::ClientHandle;
use jeliya_platform::navigation::{RoomDest, Route};
use jeliya_platform::PreferenceKey;

use super::read::ready_call;
use super::Heading;
use crate::l10n::{use_formats, use_strings};
use crate::shell::router::NavIntent;
use crate::status;
use crate::view::agents::{fold_room_agents, fold_run_history, AgentsView, RunHistoryView};
use crate::view::alias::AliasMap;
use crate::view::fleet::FleetRowView;
use crate::view::load::LoadState;
use crate::PlatformServices;

fn first_page() -> Page {
    Page {
        cursor: jeliya_api::Cursor::Start,
        direction: jeliya_api::Direction::Backward,
        limit: 50,
    }
}

fn load_error<T>(err: &jeliya_client::CallError) -> LoadState<T> {
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

/// The Agents & Runs destination pane.
#[component]
pub fn AgentsPane(
    room: RoomRow,
    #[props(default)] item: Option<SubjectId>,
    handle: ClientHandle,
    services: PlatformServices,
    navigate: Callback<NavIntent>,
    #[props(default)] self_id: Option<SubjectId>,
) -> Element {
    let strings = use_strings();
    let room_id = room.room_id.clone();

    let agents = use_signal(|| LoadState::<AgentsView>::Loading);
    {
        let handle = handle.clone();
        let room_id = room_id.clone();
        let mut agents = agents;
        use_future(move || {
            let handle = handle.clone();
            let room_id = room_id.clone();
            async move {
                agents.set(LoadState::Loading);
                match ready_call(&handle, FleetList {}).await {
                    Ok(fleet) => {
                        let view = fold_room_agents(&fleet, &room_id);
                        agents.set(if view.is_empty() {
                            LoadState::Empty
                        } else {
                            LoadState::Loaded(view)
                        });
                    }
                    Err(err) => agents.set(load_error(&err)),
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
    let heading = strings.agents_heading();

    rsx! {
        section { class: "agents-pane", id: "agents-pane",
            Heading { level: 2, class: "agents-title".to_string(), "{heading}" }
            match &*agents.read() {
                LoadState::Loading => rsx! { p { class: "muted", id: "agents-loading", "{strings.state_loading()}" } },
                LoadState::Empty => rsx! { p { class: "muted", id: "agents-empty", "{strings.agents_empty()}" } },
                LoadState::Offline => rsx! { p { class: "muted", id: "agents-offline", "{strings.state_offline()}" } },
                LoadState::Unauthorized => rsx! { p { class: "muted", id: "agents-unauth", "{strings.state_unauthorized()}" } },
                LoadState::Failed(_) => rsx! { p { class: "error-note", id: "agents-failed", "{strings.state_failed_body()}" } },
                LoadState::Loaded(view) | LoadState::Stale(view) => rsx! {
                    ul { class: "agents-list", id: "agents-list",
                        for row in view.rows.iter() {
                            AgentRow {
                                key: "{row.subject_id}",
                                row: row.clone(),
                                aliases: aliases.clone(),
                                self_id: self_id.clone(),
                                selected: item.as_ref() == Some(&row.subject_id),
                                room_id: room_id.clone(),
                                handle: handle.clone(),
                                navigate,
                            }
                        }
                    }
                },
            }
        }
    }
}

/// One agent row, with the run-history disclosure.
#[component]
fn AgentRow(
    row: FleetRowView,
    aliases: AliasMap,
    self_id: Option<SubjectId>,
    selected: bool,
    room_id: jeliya_api::RoomId,
    handle: ClientHandle,
    navigate: Callback<NavIntent>,
) -> Element {
    let strings = use_strings();
    let formats = use_formats();
    let resolved = aliases.resolve(&row.subject_id, self_id.as_ref());
    let name = status::resolved_name(strings, &resolved);
    // Liveness and latest status are TWO separately named facts (D10).
    let liveness = status::liveness_label(strings, row.liveness);
    let latest = status::latest_status_label(strings, &row.latest);
    let last_seen = status::last_seen_label(strings, formats, &row.last_seen);
    let subject = row.subject_id.clone();

    // The run history is read on demand when this agent is the selected item.
    let history = use_signal(|| None::<LoadState<RunHistoryView>>);
    {
        let handle = handle.clone();
        let room_id = room_id.clone();
        let subject = subject.clone();
        let mut history = history;
        use_future(move || {
            let open = selected;
            let handle = handle.clone();
            let room_id = room_id.clone();
            let subject = subject.clone();
            async move {
                if !open {
                    history.set(None);
                    return;
                }
                history.set(Some(LoadState::Loading));
                match ready_call(
                    &handle,
                    StatusHistory {
                        room_id,
                        subject_id: subject,
                        page: first_page(),
                    },
                )
                .await
                {
                    Ok(out) => {
                        let view = fold_run_history(&out);
                        history.set(Some(if view.is_empty() {
                            LoadState::Empty
                        } else {
                            LoadState::Loaded(view)
                        }));
                    }
                    Err(err) => history.set(Some(load_error(&err))),
                }
            }
        });
    }

    let class = if selected {
        "agent-row selected"
    } else {
        "agent-row"
    };
    rsx! {
        li { class: "{class}", "data-subject": "{row.subject_id}",
            button {
                r#type: "button",
                class: "agent-select",
                "aria-expanded": "{selected}",
                onclick: {
                    let room_id = room_id.clone();
                    let subject = subject.clone();
                    move |_| {
                        // Toggle the run-history deep link (stable, Back-closeable).
                        let dest = if selected {
                            RoomDest::Agents { item: None }
                        } else {
                            RoomDest::Agents { item: Some(subject.clone()) }
                        };
                        navigate.call(NavIntent::push(Route::Room { room_id: room_id.clone(), dest }));
                    }
                },
                span { class: "agent-name", "{name}" }
                span { class: "agent-liveness", "{strings.agents_liveness_label()} {liveness}" }
                span { class: "agent-status", "{strings.agents_status_label()} {latest}" }
                span { class: "agent-last-seen muted", "{strings.agents_last_seen_label()} {last_seen}" }
                span { class: "agent-run-open muted", " {strings.run_history_open()}" }
            }
            if selected {
                if let Some(state) = history.read().clone() {
                    RunHistory { state }
                }
            }
        }
    }
}

/// The run-history disclosure for a selected agent.
#[component]
fn RunHistory(state: LoadState<RunHistoryView>) -> Element {
    let strings = use_strings();
    let formats = use_formats();
    rsx! {
        section { class: "run-history", id: "run-history",
            Heading { level: 4, class: "run-history-title".to_string(), "{strings.run_history_heading()}" }
            match &state {
                LoadState::Loading => rsx! { p { class: "muted", "{strings.state_loading()}" } },
                LoadState::Empty => rsx! { p { class: "muted", id: "run-history-empty", "{strings.run_history_empty()}" } },
                LoadState::Offline => rsx! { p { class: "muted", "{strings.state_offline()}" } },
                LoadState::Unauthorized => rsx! { p { class: "muted", "{strings.state_unauthorized()}" } },
                LoadState::Failed(_) => rsx! { p { class: "error-note", "{strings.state_failed_body()}" } },
                LoadState::Loaded(view) | LoadState::Stale(view) => rsx! {
                    ul { class: "run-list",
                        for (i , entry) in view.entries.iter().enumerate() {
                            li { key: "{i}", class: "run-entry",
                                span { class: "run-at muted", "{status::date_of(formats, entry.at)}" }
                                span { class: "run-label", "{status::status_label_word(strings, entry.label)}" }
                                span { class: "run-progress muted",
                                    "{strings.run_progress_label()} {progress_text(strings, &entry.progress)}"
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

/// Render a status progress as text (a locale-formatted percentage, or a dash
/// when absent).
fn progress_text(strings: &dyn crate::l10n::Catalog, progress: &jeliya_api::Progress) -> String {
    match progress {
        jeliya_api::Progress::Reported { percent } => strings.format_percent(&percent.to_string()),
        jeliya_api::Progress::Absent => "—".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l10n::{En, Fr};
    use jeliya_api::Progress;

    // ---- progress_text: two exhaustive arms, both locales -----------------

    #[test]
    fn progress_text_reported_is_nonempty_in_both_locales() {
        for cat in [
            &En as &dyn crate::l10n::Catalog,
            &Fr as &dyn crate::l10n::Catalog,
        ] {
            let s = progress_text(cat, &Progress::Reported { percent: 42 });
            assert!(
                !s.is_empty(),
                "Reported progress must render a non-empty string"
            );
            // The number must appear somewhere, locale formatting or not.
            assert!(s.contains("42"), "the percent value must be present: {s:?}");
        }
    }

    #[test]
    fn progress_text_absent_is_the_dash_placeholder_in_both_locales() {
        for cat in [
            &En as &dyn crate::l10n::Catalog,
            &Fr as &dyn crate::l10n::Catalog,
        ] {
            assert_eq!(
                progress_text(cat, &Progress::Absent),
                "—",
                "absent progress must render the em-dash placeholder"
            );
        }
    }

    #[test]
    fn progress_text_at_boundary_percents_is_nonempty() {
        // 0% and 100% are both valid reported values; neither must render empty.
        let cat = &En as &dyn crate::l10n::Catalog;
        assert!(!progress_text(cat, &Progress::Reported { percent: 0 }).is_empty());
        assert!(!progress_text(cat, &Progress::Reported { percent: 100 }).is_empty());
    }
}
