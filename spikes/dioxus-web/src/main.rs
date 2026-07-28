//! Disposable Dioxus/WASM vertical slice for issue #158.
//!
//! Covers exactly the five steps the spike is asked to prove — bootstrap, room
//! list, room open, message send, and one live `room.event` push — against a
//! real `jeliyad`. It is not a port of the Room Workbench, it does not
//! establish component structure, and it changes nothing in `ui/`.
//!
//! Every class name below is copied from the React client so that
//! `ui/src/styles.css` is exercised UNMODIFIED. Whether the existing design
//! system survives the renderer swap is one of the questions being answered.

mod client;
mod proto;

use dioxus::prelude::*;

use client::Client;
use proto::{DaemonStatus, RoomCreated, RoomList, RoomOpened, RoomSummary, TimelineEvent};

fn main() {
    dioxus::launch(App);
}

#[derive(PartialEq, Clone)]
enum Phase {
    Connecting,
    Ready,
    Failed(String),
}

/// The normative client-side merge rule (`docs/PROTOCOL.md`): dedup by
/// `event_id`, then insert by `ts` rather than by arrival, because a
/// reconnecting peer's backlog validates late and can carry a `ts` older than
/// what is already rendered. Ties break by arrival, so an equal-`ts` event
/// lands after its peers.
fn merge_event(timeline: &mut Vec<TimelineEvent>, event: TimelineEvent) {
    if timeline.iter().any(|e| e.event_id == event.event_id) {
        return;
    }
    let at = timeline.partition_point(|e| e.ts <= event.ts);
    timeline.insert(at, event);
}

#[component]
fn App() -> Element {
    let mut phase = use_signal(|| Phase::Connecting);
    let mut status = use_signal(|| None::<DaemonStatus>);
    let mut client = use_signal(|| None::<Client>);
    let mut rooms = use_signal(Vec::<RoomSummary>::new);
    let mut current = use_signal(|| None::<String>);
    let mut timeline = use_signal(Vec::<TimelineEvent>::new);
    let mut draft = use_signal(String::new);
    let mut notice = use_signal(|| None::<String>);
    let mut pushes = use_signal(|| 0usize);

    // Bootstrap, once. Fresh state is expected: a brand-new data dir has no
    // identity and no rooms, and the slice must come up from exactly that.
    use_future(move || async move {
        let conn = Client::connect(move |push| {
            // Pushes are broadcast for every open room on the daemon, so the
            // client filters by room itself — there is no subscribe operation.
            if current.read().as_deref() == Some(push.room_id.as_str()) {
                pushes += 1;
                merge_event(&mut timeline.write(), push.event);
            }
        })
        .await;

        let conn = match conn {
            Ok(c) => c,
            Err(e) => {
                phase.set(Phase::Failed(e));
                return;
            }
        };
        client.set(Some(conn.clone()));

        // No greeting frame exists, so a client MUST read daemon.status once
        // after connecting and range-check the protocol major before trusting
        // any other contract.
        let st: DaemonStatus = match conn.call("daemon.status", serde_json::json!({})).await {
            Ok(s) => s,
            Err(e) => {
                phase.set(Phase::Failed(format!("daemon.status: {e}")));
                return;
            }
        };
        if st.protocol != 1 {
            phase.set(Phase::Failed(format!(
                "daemon speaks protocol {}, this spike speaks 1",
                st.protocol
            )));
            return;
        }
        // `identity == null` is the only onboarding signal; `endpoint` is also
        // null whenever no room is open, so it says nothing about setup.
        if st.identity.is_none() {
            if let Err(e) = conn
                .call::<serde_json::Value>("identity.create", serde_json::json!({}))
                .await
            {
                phase.set(Phase::Failed(format!("identity.create: {e}")));
                return;
            }
        }
        status.set(Some(st));

        // A fresh data dir lists no rooms (an empty result, not an error), so
        // make one for the slice to open. Disposable, like the data dir.
        let mut list: RoomList = match conn.call("room.list", serde_json::json!({})).await {
            Ok(l) => l,
            Err(e) => {
                phase.set(Phase::Failed(format!("room.list: {e}")));
                return;
            }
        };
        if list.rooms.is_empty() {
            if let Err(e) = conn
                .call::<RoomCreated>("room.create", serde_json::json!({ "name": "Spike room" }))
                .await
            {
                phase.set(Phase::Failed(format!("room.create: {e}")));
                return;
            }
            list = match conn.call("room.list", serde_json::json!({})).await {
                Ok(l) => l,
                Err(e) => {
                    phase.set(Phase::Failed(format!("room.list (after create): {e}")));
                    return;
                }
            };
        }
        rooms.set(list.rooms);
        phase.set(Phase::Ready);
    });

    // `room.open` is mandatory before `message.send` and is what starts this
    // room's pushes; it also succeeds locally regardless of peer reachability.
    let open_room = move |room_id: String| async move {
        let Some(conn) = client.read().clone() else {
            return;
        };
        notice.set(None);
        match conn
            .call::<RoomOpened>("room.open", serde_json::json!({ "room_id": room_id }))
            .await
        {
            Ok(opened) => {
                current.set(Some(room_id));
                let mut next = Vec::new();
                for event in opened.timeline {
                    merge_event(&mut next, event);
                }
                timeline.set(next);
            }
            Err(e) => notice.set(Some(format!("room.open: {e}"))),
        }
    };

    let send = move |_| async move {
        let (Some(conn), Some(room_id)) = (client.read().clone(), current.read().clone()) else {
            return;
        };
        let body = draft.read().trim().to_string();
        if body.is_empty() {
            return;
        }
        // No optimistic echo. The message appears when the daemon's own
        // `room.event` arrives — which may land BEFORE this response resolves,
        // since both travel the same broadcast path. `merge_event` dedups by
        // `event_id`, so either order renders once.
        match conn
            .call::<serde_json::Value>(
                "message.send",
                serde_json::json!({ "room_id": room_id, "body": body }),
            )
            .await
        {
            Ok(_) => draft.set(String::new()),
            Err(e) => notice.set(Some(format!("message.send: {e}"))),
        }
    };

    match phase() {
        Phase::Connecting => rsx! {
            main { class: "boot-screen", id: "spike-boot",
                h1 { "Jeliya" }
                p { class: "boot-target mono", "connecting to the local daemon…" }
            }
        },
        Phase::Failed(reason) => rsx! {
            main { class: "boot-screen", id: "spike-boot",
                div { class: "error-note",
                    p { class: "error-title", "Could not start" }
                    p { class: "error-code mono", id: "spike-error", "{reason}" }
                }
            }
        },
        Phase::Ready => rsx! {
            // The reused stylesheet is a one-pane-at-a-time shell below
            // 899.98px: `.app .sidebar` and `.app .center` are `display: none`
            // there, and only a root pane state reveals one. A root of plain
            // `app` renders a blank screen on any compact viewport — including
            // a phone system WebView, which is a target platform. React sets
            // `app pane-${pane}`; so does this.
            div {
                class: if current.read().is_some() { "app pane-room" } else { "app pane-rooms" },
                id: "spike-app",
                nav { class: "sidebar",
                    for room in rooms().into_iter() {
                        {
                            let id = room.room_id.clone();
                            let selected = current.read().as_deref() == Some(id.as_str());
                            let label = room.name.clone().unwrap_or_else(|| "Untitled room".into());
                            rsx! {
                                div {
                                    key: "{room.room_id}",
                                    class: if selected { "room-item selected" } else { "room-item" },
                                    button {
                                        r#type: "button",
                                        class: "room-select",
                                        "data-room": "{room.room_id}",
                                        onclick: move |_| {
                                            let id = id.clone();
                                            async move { open_room(id).await }
                                        },
                                        span { class: "room-info",
                                            span { class: "room-name-line",
                                                span { class: "room-name", "{label}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                section { class: "center",
                    if let Some(message) = notice() {
                        div { class: "error-note", id: "spike-notice", "{message}" }
                    }
                    if current.read().is_some() {
                        div { class: "timeline", id: "spike-timeline", "data-pushes": "{pushes}",
                            for event in timeline().into_iter().filter(|e| e.kind == "message") {
                                div {
                                    key: "{event.event_id}",
                                    class: "msg-row",
                                    "data-event": "{event.event_id}",
                                    div { class: "msg-col",
                                        div { class: "msg-bubble",
                                            "{event.body.clone().unwrap_or_default()}"
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "composer",
                            div { class: "composer-bar",
                                // A `textarea`, because the stylesheet styles
                                // the editor as `.composer-bar textarea` and
                                // has no rule for a class on an `input`. React
                                // renders a textarea for the same reason.
                                textarea {
                                    id: "composer-input",
                                    rows: "1",
                                    value: "{draft}",
                                    placeholder: "Message",
                                    oninput: move |e| draft.set(e.value()),
                                }
                                button {
                                    r#type: "button",
                                    class: "composer-send",
                                    id: "spike-send",
                                    onclick: send,
                                    "Send"
                                }
                            }
                        }
                    } else {
                        div { class: "center-empty",
                            p { class: "center-empty-title", "Choose a room" }
                        }
                    }
                }

                if let Some(st) = status() {
                    p { class: "boot-target mono", id: "spike-status",
                        "jeliyad {st.version} · protocol {st.protocol}"
                    }
                }
            }
        },
    }
}
