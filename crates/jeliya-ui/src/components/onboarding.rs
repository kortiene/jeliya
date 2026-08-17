//! First-run onboarding (#178 §5.E): create-or-connect an identity, then
//! create-or-join the first room. The onboarding terminus is a room the user
//! holds.
//!
//! Ported from the retiring `ui/src/components/Onboarding.tsx`, re-expressed
//! against v2 ops and the #177 catalog (no literal copy):
//! - **Identity step** — an optional self-label field beside a "create identity"
//!   action calling [`SubjectEnsure`]. `created: false` (someone/another tab
//!   already created it) simply advances.
//! - **Rooms step** — create a room ([`RoomCreate`], non-whitespace name
//!   required) is the guaranteed terminus; the invite-ticket field is present and
//!   validated, but the full retrying join flow is #179-adjacent (spec Q3).

use dioxus::prelude::*;
use jeliya_api::{RoomCreate, RoomId, SubjectEnsure};
use jeliya_client::{ClientHandle, Dedup};

use crate::components::Field;
use crate::l10n::use_strings;
use crate::shell::bootstrap::OnboardStep;
use crate::PlatformServices;

/// The onboarding surface for `step`.
///
/// `on_advance` is called when a step completes: `None` after the identity step
/// (advance to the rooms step), `Some(room_id)` after the first room is created
/// (advance to the shell, opening that room).
#[component]
pub fn Onboarding(
    handle: ClientHandle,
    services: PlatformServices,
    step: OnboardStep,
    on_advance: EventHandler<Option<RoomId>>,
) -> Element {
    match step {
        OnboardStep::Identity => rsx! {
            IdentityStep { handle, services, on_done: move |_| on_advance.call(None) }
        },
        OnboardStep::Rooms => rsx! {
            RoomsStep { handle, on_created: move |room_id| on_advance.call(Some(room_id)) }
        },
    }
}

/// The identity step: wordmark, an optional self-label field, and a "create
/// identity" action.
#[component]
fn IdentityStep(
    handle: ClientHandle,
    services: PlatformServices,
    on_done: EventHandler<()>,
) -> Element {
    let strings = use_strings();
    let app_name = strings.app_name();
    let title = strings.onboarding_identity_title();
    let body = strings.onboarding_identity_body();
    let create = strings.onboarding_create_identity();
    let self_label = strings.self_label_label().to_string();
    let self_help = strings.self_label_help().to_string();
    let current = services
        .preferences()
        .get(&jeliya_platform::PreferenceKey::SelfLabel)
        .unwrap_or_default();
    // Dynamic hint association: the Field id is a String expression, so the
    // control's `aria-describedby` must construct `{id}-hint` inline (#177 gate).
    let self_label_base = "onboarding-self-label";

    let create_identity = move |_| {
        let handle = handle.clone();
        let on_done = on_done;
        spawn(async move {
            // Idempotent: `created: false` (another tab already created it) just
            // advances, exactly like the retiring IdentityStep's identity_exists
            // (the daemon returns the existing subject). A refusal is left for the
            // caller's diagnostics; onboarding does not loop.
            if handle
                .call::<SubjectEnsure>(SubjectEnsure {}, Dedup::None)
                .await
                .is_ok()
            {
                on_done.call(());
            }
        });
    };

    rsx! {
        main { class: "onboarding", id: "onboarding-identity", tabindex: "-1",
            h1 { class: "onboarding-wordmark", "{app_name}" }
            h2 { class: "onboarding-title", "{title}" }
            p { class: "onboarding-body", "{body}" }
            Field {
                id: self_label_base.to_string(),
                label: self_label,
                hint: self_help.clone(),
                input {
                    class: "input",
                    id: self_label_base.to_string(),
                    aria_describedby: format!("{self_label_base}-hint"),
                    value: "{current}",
                    oninput: {
                        let services = services.clone();
                        move |evt: FormEvent| {
                            let value: String = evt.value().trim().chars().take(40).collect();
                            if value.is_empty() {
                                services.preferences().remove(&jeliya_platform::PreferenceKey::SelfLabel);
                            } else {
                                services.preferences().set(jeliya_platform::PreferenceKey::SelfLabel, &value);
                            }
                        }
                    },
                }
            }
            button {
                class: "btn btn-primary",
                id: "onboarding-create-identity",
                onclick: create_identity,
                "{create}"
            }
        }
    }
}

/// The rooms step: create a room (the terminus) and a present, validated
/// invite-ticket field (join flow deferred, spec Q3).
#[component]
fn RoomsStep(handle: ClientHandle, on_created: EventHandler<RoomId>) -> Element {
    let strings = use_strings();
    let title = strings.onboarding_rooms_title();
    let create = strings.onboarding_create_room();
    let room_name_label = strings.room_name_label().to_string();
    let join = strings.onboarding_join_room();
    let ticket_label = strings.ticket_label().to_string();
    let ticket_help = strings.ticket_help().to_string();

    let mut name = use_signal(String::new);
    let mut ticket = use_signal(String::new);
    // Dynamic hint association for the ticket field (#177 form gate): the Field
    // id is a String expression, so `aria-describedby` constructs `{id}-hint`
    // inline.
    let ticket_base = "onboarding-ticket";

    let create_room = move |_| {
        let trimmed = name.peek().trim().to_owned();
        if trimmed.is_empty() {
            return;
        }
        let handle = handle.clone();
        let on_created = on_created;
        spawn(async move {
            if let Ok(out) = handle
                .call::<RoomCreate>(RoomCreate { name: trimmed }, Dedup::None)
                .await
            {
                on_created.call(out.room_id);
            }
        });
    };

    rsx! {
        main { class: "onboarding", id: "onboarding-rooms", tabindex: "-1",
            h1 { class: "onboarding-title", "{title}" }
            div { class: "onboarding-task", id: "onboarding-create-room-task",
                Field { id: "onboarding-room-name".to_string(), label: room_name_label,
                    input {
                        class: "input",
                        id: "onboarding-room-name",
                        value: "{name}",
                        oninput: move |evt| name.set(evt.value()),
                    }
                }
                button {
                    class: "btn btn-primary",
                    id: "onboarding-create-room",
                    onclick: create_room,
                    "{create}"
                }
            }
            div { class: "onboarding-task", id: "onboarding-join-room-task",
                Field {
                    id: ticket_base.to_string(),
                    label: ticket_label,
                    hint: ticket_help.clone(),
                    input {
                        class: "input",
                        id: ticket_base.to_string(),
                        aria_describedby: format!("{ticket_base}-hint"),
                        value: "{ticket}",
                        oninput: move |evt| ticket.set(evt.value()),
                    }
                }
                // The join action is present; the full retrying join flow is
                // #179-adjacent (spec Q3). The button validates a non-empty
                // ticket but does not yet dispatch room.join.
                button {
                    class: "btn",
                    id: "onboarding-join-room",
                    disabled: ticket.read().trim().is_empty(),
                    "{join}"
                }
            }
        }
    }
}
