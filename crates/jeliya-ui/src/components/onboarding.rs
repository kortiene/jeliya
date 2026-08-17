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
//!
//! Errors surface honestly (Finding 1): both the identity and room-creation
//! operations display an inline error region with the catalog message and the
//! raw `CallError` (mono, technical detail — §5.8).  The form remains usable so
//! the user can retry without reloading.  Focus is moved to the main element on
//! mount (Finding 2), following the dialog/BootScreen pattern that uses
//! `MountedEvent::set_focus`.

use dioxus::prelude::*;
use jeliya_api::{InviteRedeem, RoomCreate, RoomId, SubjectEnsure};
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
            IdentityStep {
                handle,
                services,
                on_done: move |_| on_advance.call(None),
            }
        },
        OnboardStep::Rooms => rsx! {
            RoomsStep {
                handle,
                on_created: move |room_id| on_advance.call(Some(room_id)),
            }
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
    let err_title = strings.err_onboarding_identity();
    let err_body = strings.err_onboarding_identity_body();
    let current = services
        .preferences()
        .get(&jeliya_platform::PreferenceKey::SelfLabel)
        .unwrap_or_default();
    // Dynamic hint association: the Field id is a String expression, so the
    // control's `aria-describedby` must construct `{id}-hint` inline (#177 gate).
    let self_label_base = "onboarding-self-label";

    let error = use_signal(|| None::<String>);

    let create_identity = move |_| {
        let handle = handle.clone();
        let on_done = on_done;
        let mut error = error;
        spawn(async move {
            match handle
                .call::<SubjectEnsure>(SubjectEnsure {}, Dedup::None)
                .await
            {
                Ok(_out) => on_done.call(()),
                Err(e) => {
                    // Surface the raw error string so the user gets the real
                    // code (Calling 1 — "Failures are failures" law).
                    error.set(Some(format!("{}: {e}", err_title)));
                }
            }
        });
    };

    rsx! {
        main {
            class: "onboarding",
            id: "onboarding-identity",
            tabindex: "-1",
            onmounted: move |evt: MountedEvent| {
                // Move focus to this step's main so keyboard users land on the
                // form immediately after a transition (Finding 2; app.rs pattern).
                spawn(async move {
                    let _ = evt.set_focus(true).await;
                });
            },
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
            if let Some(ref err) = error.peek().as_ref() {
                div { class: "error-note", id: "onboarding-error-identity", role: "alert",
                    // First line is the catalog-friendly title + the raw error.
                    // The raw error string gives the real code (the "law":
                    // "Errors surface their real code and a useful hint").
                    p { class: "error-message", "{err_body}" }
                    p { class: "mono", id: "onboarding-error-detail", "{err}" }
                }
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
    let err_title = strings.err_onboarding_room_create();
    let err_body = strings.err_onboarding_room_create_body();

    let mut name = use_signal(String::new);
    let mut ticket = use_signal(String::new);
    let error = use_signal(|| None::<String>);
    // Dynamic hint association for the ticket field (#177 form gate): the Field
    // id is a String expression, so `aria-describedby` constructs `{id}-hint`
    // inline.
    let ticket_base = "onboarding-ticket";

    // In-flight latches: a mutation button must never dispatch a second call
    // while the first is unanswered — room-name homonyms are valid, so a
    // double-click would otherwise create TWO rooms while onboarding advances
    // on whichever reply lands first. The latch releases only on error (on
    // success this surface unmounts as onboarding completes).
    let mut creating = use_signal(|| false);
    let mut joining = use_signal(|| false);

    let handle_create = handle.clone();
    let create_room = move |_| {
        let trimmed = name.peek().trim().to_owned();
        if trimmed.is_empty() || creating() {
            return;
        }
        creating.set(true);
        let handle = handle_create.clone();
        let on_created = on_created;
        let mut error = error;
        spawn(async move {
            match handle
                .call::<RoomCreate>(RoomCreate { name: trimmed }, Dedup::None)
                .await
            {
                Ok(out) => on_created.call(out.room_id),
                Err(e) => {
                    // Surface the raw error string so the user gets the real
                    // code (Finding 1 — "Failures are failures" law).
                    error.set(Some(format!("{}: {e}", err_title)));
                    creating.set(false);
                }
            }
        });
    };

    let join_room = move |_| {
        let capability = ticket.peek().trim().to_owned();
        if capability.is_empty() || joining() {
            return;
        }
        joining.set(true);
        let handle = handle.clone();
        let on_created = on_created;
        let mut error = error;
        spawn(async move {
            match handle
                .call::<InviteRedeem>(InviteRedeem { capability }, Dedup::None)
                .await
            {
                // A replay (`joined: false`) still names the room — the user's
                // membership exists either way, so both outcomes advance.
                Ok(out) => on_created.call(out.room_id),
                Err(e) => {
                    error.set(Some(format!("{}: {e}", err_title)));
                    joining.set(false);
                }
            }
        });
    };

    rsx! {
        main {
            class: "onboarding",
            id: "onboarding-rooms",
            tabindex: "-1",
            onmounted: move |evt: MountedEvent| {
                // Move focus to this step's main so keyboard users land on the
                // form immediately after a transition (Finding 2; app.rs pattern).
                spawn(async move {
                    let _ = evt.set_focus(true).await;
                });
            },
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
                    disabled: creating(),
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
                // The join action redeems the ticket's capability through
                // `invite.redeem`; both a fresh join and a replay advance to
                // the named room. The full retrying join flow is #179-adjacent
                // (spec Q3) — this dispatches the mutation honestly, with the
                // raw error code on failure.
                button {
                    class: "btn",
                    id: "onboarding-join-room",
                    disabled: ticket.read().trim().is_empty() || joining(),
                    onclick: join_room,
                    "{join}"
                }
            }
            if let Some(ref err) = error.peek().as_ref() {
                div { class: "error-note", id: "onboarding-error-rooms", role: "alert",
                    p { class: "error-message", "{err_body}" }
                    p { class: "mono", id: "onboarding-error-detail", "{err}" }
                }
            }
        }
    }
}
