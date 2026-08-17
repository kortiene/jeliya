//! The Pipes room-destination pane (#181 §5.D): the pipe list with truthful
//! Connected/Open state and publisher reachability, an Expose action (loopback
//! target + required audience), and per-pipe Connect / Release / Revoke.
//! Replaces the #178 skeleton at `/rooms/:roomId/pipes`.
//!
//! Decisions live in the host-tested [`crate::pipes`] orchestration. `link`
//! (publisher reachability) and `connected` (local connection) are rendered as
//! separate facts (spec D2); a target is validated loopback-only client-side
//! before any round trip (spec D6); `pipe_unreachable` / `pipe_unknown` /
//! `pipe_revoked` render distinctly (§6/#94). Booting is unknown, not zero (D8).

use dioxus::prelude::*;
use jeliya_api::{
    Audience, Cursor, Direction, Link, LinkReason, Page, PipeId, PipeList, RoomId, SubjectId,
    Target,
};
use jeliya_client::{ClientHandle, Dedup};
use jeliya_platform::CancelToken;

use crate::components::Field;
use crate::files::{errors::FlowStop, FlowFailure, Settled};
use crate::l10n::{use_strings, ErrorDisplay};
use crate::pipes::{run_connect, run_publish, run_release, run_revoke, PipeListModel, PipeRowView};
use crate::PlatformServices;

fn first_page() -> Page {
    Page {
        cursor: Cursor::Start,
        direction: Direction::Forward,
        limit: 100,
    }
}

/// The Pipes pane.
#[component]
pub fn PipesPane(
    handle: ClientHandle,
    #[props(default)] _services: Option<PlatformServices>,
    room_id: RoomId,
    /// The deep-linked selected pipe, if any (`RoomDest::Pipes { item }`).
    #[props(default)]
    selected: Option<PipeId>,
    /// The local subject, when known, so own pipes can be offered Revoke. `None`
    /// (the seam gap) never guesses — Revoke is simply not shown (spec D2).
    #[props(default)]
    own_subject: Option<SubjectId>,
) -> Element {
    let strings = use_strings();

    let mut reload = use_signal(|| 0_u32);
    let notice = use_signal(|| None::<String>);
    // Connection ids captured from successful connects, so Release (which is by
    // connection, not pipe — spec §5.C) can name them.
    let connections = use_signal(Vec::<(String, String)>::new);

    let list = {
        let handle = handle.clone();
        let room_id = room_id.clone();
        use_resource(move || {
            let handle = handle.clone();
            let room_id = room_id.clone();
            let _ = reload();
            async move {
                match handle
                    .call::<PipeList>(
                        PipeList {
                            room_id,
                            page: first_page(),
                        },
                        Dedup::None,
                    )
                    .await
                {
                    Ok(out) => Ok(PipeListModel::from_out(out)),
                    Err(error) => Err(FlowStop::from_call(&error)),
                }
            }
        })
    };

    rsx! {
        section { class: "pipes-pane", id: "pipes-pane",
            header { class: "pane-header",
                h2 { class: "pane-title", "{strings.pipes_heading()}" }
            }
            ExposeForm {
                handle: handle.clone(),
                room_id: room_id.clone(),
                notice,
                reload,
            }
            if let Some(message) = notice() {
                div { class: "error-note", id: "pipes-notice", "{message}" }
            }
            {
                match &*list.read_unchecked() {
                    None => rsx! {
                        div { class: "muted", id: "pipes-loading", "{strings.pipes_loading()}" }
                    },
                    Some(Err(stop)) => {
                        let failure = match stop {
                            FlowStop::Failed(f) => f.clone(),
                            FlowStop::Cancelled => FlowFailure::Transport,
                        };
                        let message = ErrorDisplay::flow_failure(strings, crate::l10n::use_formats(), &failure).message;
                        rsx! {
                            div { class: "error-note", id: "pipes-failed",
                                p { "{message}" }
                                button {
                                    class: "btn btn-sm",
                                    id: "pipes-retry",
                                    onclick: move |_| reload.set(reload() + 1),
                                    "{strings.files_retry_action()}"
                                }
                            }
                        }
                    }
                    Some(Ok(model)) if model.rows.is_empty() => rsx! {
                        div { class: "muted", id: "pipes-empty", "{strings.pipes_empty()}" }
                    },
                    Some(Ok(model)) => {
                        let rows = model.rows.clone();
                        rsx! {
                            ul { class: "pipes-list", id: "pipes-list",
                                for row in rows {
                                    PipeRow {
                                        key: "{row.pipe_id}",
                                        handle: handle.clone(),
                                        room_id: room_id.clone(),
                                        row: row.clone(),
                                        selected: selected.as_ref() == Some(&row.pipe_id),
                                        is_own: row.is_own(own_subject.as_ref()),
                                        notice,
                                        reload,
                                        connections,
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

/// The Expose form: a loopback host/port and a required audience. The target is
/// validated client-side (loopback only) before any round trip (spec D6).
#[component]
fn ExposeForm(
    handle: ClientHandle,
    room_id: RoomId,
    notice: Signal<Option<String>>,
    reload: Signal<u32>,
) -> Element {
    let strings = use_strings();
    let formats = crate::l10n::use_formats();
    let mut host = use_signal(|| "127.0.0.1".to_string());
    let mut port = use_signal(String::new);

    let on_expose = move |_: ()| {
        let mut notice = notice;
        let mut reload = reload;
        let Ok(port_num) = port().trim().parse::<u64>() else {
            notice.set(Some(strings.err_pipe_target_refused().to_string()));
            return;
        };
        let handle = handle.clone();
        let room_id = room_id.clone();
        let target = Target {
            host: host().trim().to_string(),
            port: port_num,
        };
        let ct = CancelToken::new();
        spawn(async move {
            // Audience is required and stated explicitly (never defaulted). The
            // form ships the room audience; named-subject audiences are a
            // follow-up affordance.
            let outcome = run_publish(&handle, room_id, target, Audience::Room, &ct).await;
            match outcome {
                Settled::Ok(_) => {
                    notice.set(None);
                    reload.set(reload() + 1);
                }
                Settled::Failed(failure) => {
                    notice.set(Some(
                        ErrorDisplay::flow_failure(strings, formats, &failure).message,
                    ));
                }
                Settled::Cancelled => {}
            }
        });
    };

    rsx! {
        form {
            class: "expose-form",
            id: "expose-form",
            onsubmit: move |evt: FormEvent| {
                evt.prevent_default();
                on_expose(());
            },
            // Both inputs live inside one labelled Field (Decision-6): the field
            // names the loopback host/port group for assistive tech, and the hint
            // is associated with both controls.
            Field {
                id: "expose-host".to_string(),
                label: strings.pipes_target_label().to_string(),
                input {
                    class: "input",
                    id: "expose-host",
                    value: "{host}",
                    oninput: move |evt: FormEvent| host.set(evt.value()),
                }
            }
            Field {
                id: "expose-port".to_string(),
                label: strings.pipes_port_label().to_string(),
                input {
                    class: "input",
                    id: "expose-port",
                    r#type: "number",
                    value: "{port}",
                    oninput: move |evt: FormEvent| port.set(evt.value()),
                }
            }
            span { class: "muted",
                "{strings.pipes_audience_label()} — {strings.pipes_audience_room()}"
            }
            button { class: "btn btn-primary btn-sm", r#type: "submit", id: "expose-submit",
                "{strings.pipes_expose_action()}"
            }
        }
    }
}

/// A localized reachability label for a publisher's link (spec D2).
fn reach_label(strings: &dyn crate::l10n::Catalog, link: &Link) -> String {
    match link {
        Link::Direct { .. } => strings.pipes_reach_direct().to_string(),
        Link::Relay { .. } => strings.pipes_reach_relay().to_string(),
        Link::NotConnected { reason } => {
            let why = match reason {
                LinkReason::NeverDialed => strings.status_stopped(),
                LinkReason::DialFailed | LinkReason::NoRoute => strings.status_failed(),
                LinkReason::Closed => strings.status_interrupted(),
            };
            format!("{} ({why})", strings.pipes_reach_unavailable())
        }
    }
}

/// One pipe row: Connected/Open, publisher reachability, and the lifecycle
/// actions (Connect / Release / Revoke).
#[component]
fn PipeRow(
    handle: ClientHandle,
    room_id: RoomId,
    row: PipeRowView,
    selected: bool,
    is_own: bool,
    notice: Signal<Option<String>>,
    reload: Signal<u32>,
    connections: Signal<Vec<(String, String)>>,
) -> Element {
    let strings = use_strings();
    let formats = crate::l10n::use_formats();

    let state = if row.connected {
        strings.pipes_state_connected()
    } else {
        strings.pipes_state_open()
    };
    let reach = reach_label(strings, &row.link);
    let pipe_key = row.pipe_id.as_str().to_string();
    let connection_id = connections()
        .iter()
        .find(|(pipe, _)| pipe == &pipe_key)
        .map(|(_, conn)| conn.clone());

    let on_connect = {
        let handle = handle.clone();
        let room_id = room_id.clone();
        let pipe_id = row.pipe_id.clone();
        let pipe_key = pipe_key.clone();
        move |_| {
            let handle = handle.clone();
            let room_id = room_id.clone();
            let pipe_id = pipe_id.clone();
            let pipe_key = pipe_key.clone();
            let mut notice = notice;
            let mut connections = connections;
            let mut reload = reload;
            let ct = CancelToken::new();
            spawn(async move {
                match run_connect(&handle, room_id, pipe_id, &ct).await {
                    Settled::Ok(done) => {
                        connections.write().push((pipe_key, done.connection_id));
                        notice.set(None);
                        reload.set(reload() + 1);
                    }
                    Settled::Failed(failure) => notice.set(Some(
                        ErrorDisplay::flow_failure(strings, formats, &failure).message,
                    )),
                    Settled::Cancelled => {}
                }
            });
        }
    };

    let on_release = {
        let handle = handle.clone();
        let conn = connection_id.clone();
        let pipe_key = pipe_key.clone();
        move |_| {
            let Some(conn) = conn.clone() else { return };
            let handle = handle.clone();
            let pipe_key = pipe_key.clone();
            let mut notice = notice;
            let mut connections = connections;
            let mut reload = reload;
            let ct = CancelToken::new();
            spawn(async move {
                match run_release(&handle, conn, &ct).await {
                    Settled::Ok(()) => {
                        connections.write().retain(|(pipe, _)| pipe != &pipe_key);
                        reload.set(reload() + 1);
                    }
                    Settled::Failed(failure) => notice.set(Some(
                        ErrorDisplay::flow_failure(strings, formats, &failure).message,
                    )),
                    Settled::Cancelled => {}
                }
            });
        }
    };

    let on_revoke = {
        let handle = handle.clone();
        let room_id = room_id.clone();
        let pipe_id = row.pipe_id.clone();
        move |_| {
            let handle = handle.clone();
            let room_id = room_id.clone();
            let pipe_id = pipe_id.clone();
            let mut notice = notice;
            let mut reload = reload;
            let ct = CancelToken::new();
            spawn(async move {
                match run_revoke(&handle, room_id, pipe_id, &ct).await {
                    Settled::Ok(()) => reload.set(reload() + 1),
                    Settled::Failed(failure) => notice.set(Some(
                        ErrorDisplay::flow_failure(strings, formats, &failure).message,
                    )),
                    Settled::Cancelled => {}
                }
            });
        }
    };

    let class = if selected {
        "pipe-row selected"
    } else {
        "pipe-row"
    };

    rsx! {
        li { class: "{class}", id: "pipe-row-{row.pipe_id}",
            div { class: "pipe-main",
                span { class: "pipe-state", "{state}" }
                span { class: "pipe-reach muted", "{reach}" }
            }
            div { class: "pipe-meta muted",
                span { class: "pipe-published-by", "{strings.pipes_published_by_label()}: " }
                span { class: "pipe-owner mono", "{row.published_by}" }
            }
            div { class: "pipe-actions",
                if connection_id.is_some() {
                    button { class: "btn btn-sm", id: "pipe-release-{row.pipe_id}", onclick: on_release,
                        "{strings.pipes_release_action()}"
                    }
                } else {
                    button { class: "btn btn-sm", id: "pipe-connect-{row.pipe_id}", onclick: on_connect,
                        "{strings.pipes_connect_action()}"
                    }
                }
                if is_own {
                    button { class: "btn btn-sm btn-danger", id: "pipe-revoke-{row.pipe_id}", onclick: on_revoke,
                        "{strings.pipes_revoke_action()}"
                    }
                }
            }
        }
    }
}
