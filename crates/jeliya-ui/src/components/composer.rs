//! The room composer (#179 §8): autosize, platform-correct keyboard, per-room
//! drafts, an attachment handoff, and the evidence-backed send/retry lifecycle.
//!
//! The composer owns the local text and the draft persistence; the send state
//! machine (§6) lives in the pure [`crate::room::send`] module and the shared
//! `sends` signal it mutates. Every user-visible string comes from the catalog
//! (#177). DOM measurement (autosize) rides Dioxus's renderer-agnostic mounted
//! element API — no `web-sys`, no `cfg` (Decision-3/-6).

use dioxus::html::{Key, Modifiers, ModifiersInteraction};
use dioxus::prelude::*;
use jeliya_api::{MessageSend, OpId, RoomId, Timestamp};
use jeliya_client::{ClientHandle, Dedup};
use jeliya_platform::PreferenceKey;

use crate::components::Field;
use crate::l10n::use_strings;
use crate::room::send::{classify_error, op_id_for, phase_for_reply, SendId};
use crate::room::RoomActivityState;
use crate::shell::Shell;
use crate::PlatformServices;

/// The maximum autosize height (px) before the textarea scrolls internally — a
/// grown composer never eats the timeline.
const AUTOSIZE_CAP: f64 = 160.0;

/// The maximum composed-message length in bytes. Longer input is clamped and
/// surfaced as an inline error (#179 §8, D3).
const MAX_MESSAGE_LEN: usize = 4000;

/// Dispatch one `message.send` under its stable `op_id` and settle the addressed
/// pending entry's phase from the seam's evidence alone (D3): a reply → `Syncing`,
/// a `DecodeReply` `Definitely` → `Syncing{None}`, any other error → the
/// classified `Failed`/`Syncing`. Shared by the composer's first send and a
/// timeline row's Retry, so both settle identically and — reusing the SAME
/// `op_id` (D4) — the daemon returns the original `event_id` with no second
/// effect on this connection. Returns `true` iff the call failed, so the
/// first-send path can restore its draft; a retry keeps the body in the entry
/// and has nothing to restore.
pub(crate) async fn run_send(
    handle: ClientHandle,
    mut activity: Signal<RoomActivityState>,
    room_id: RoomId,
    client_id: SendId,
    body: String,
    op_id: OpId,
) -> bool {
    let result = handle
        .call::<MessageSend>(MessageSend { room_id, body }, Dedup::Key(op_id))
        .await;
    match result {
        Ok(out) => {
            activity
                .write()
                .sends
                .set_phase(client_id, phase_for_reply(&out));
            false
        }
        Err(error) => {
            activity
                .write()
                .sends
                .set_phase(client_id, classify_error(&error));
            true
        }
    }
}

/// The composer, or — when the room lacks the `MessageSend` capability (a
/// departed room, D8) — the read-only notice that replaces it, keeping the
/// signed timeline visible while suppressing the composer as a typed capability.
#[component]
pub fn Composer(
    handle: ClientHandle,
    services: PlatformServices,
    room_id: RoomId,
    can_send: bool,
    shell: Shell,
    base_at: Timestamp,
    activity: Signal<RoomActivityState>,
) -> Element {
    let mut activity = activity;
    let strings = use_strings();

    if !can_send {
        // The capability is absent (invariant-5 floor): no disabled textarea,
        // a plain statement of the read-only fact (#91 owns the full archive).
        let departed = strings.activity_departed();
        return rsx! {
            div { class: "composer-readonly muted", id: "composer-readonly", "{departed}" }
        };
    }

    let draft_key = PreferenceKey::Draft {
        room_id: room_id.clone(),
    };
    // Seed the text from the persisted draft ONCE (survives a route change; the
    // browser store is session-scoped, so the composer implies no more).
    let initial_draft = services.preferences().get(&draft_key).unwrap_or_default();
    let mut text = use_signal(|| initial_draft);
    let mut height = use_signal(|| None::<f64>);
    let mut element = use_signal(|| None::<MountedEvent>);
    let mut overflow = use_signal(|| None::<usize>);

    let field_label = strings.composer_label();
    let placeholder = strings.composer_placeholder();
    let send_label = strings.composer_send();
    let attach_label = strings.composer_attach();
    let attach_unavailable = strings.composer_attach_unavailable();
    // The Enter-to-send hint is TRUE only on non-compact; withheld on compact
    // where Enter inserts a newline (the soft-keyboard behavior). `uses_bottom_bar`
    // is true exactly for the compact shell.
    let enter_hint = (!shell.uses_bottom_bar()).then(|| strings.composer_enter_hint());

    // Measure the textarea and apply min(scrollHeight, cap). On a hidden pane
    // the measurement is 0; keep the current height and let a later reveal/resize
    // re-measure (the React fallback, §8).
    let measure = move || {
        if let Some(evt) = element() {
            spawn(async move {
                if let Ok(size) = evt.get_scroll_size().await {
                    if size.height > 0.0 {
                        height.set(Some(size.height.min(AUTOSIZE_CAP)));
                    }
                }
            });
        }
    };

    // The send action (§6): trim, guard empty/disabled, push a Pending entry,
    // optimistically clear the draft, dispatch under the stable op_id, and settle
    // the phase from the seam's evidence. On failure restore the draft ONLY if
    // the user has not typed a new one (never clobber fresh input, §11).
    let dispatch_send = {
        let handle = handle.clone();
        let services = services.clone();
        let room_id = room_id.clone();
        let draft_key = draft_key.clone();
        move || {
            let body = text().trim().to_string();
            if body.is_empty() {
                return;
            }
            let client_id = activity.write().sends.begin(body.clone(), base_at);
            let op_id = op_id_for(client_id);
            text.set(String::new());
            overflow.set(None);
            height.set(None);
            services.preferences().remove(&draft_key);

            let handle = handle.clone();
            let services = services.clone();
            let room_id = room_id.clone();
            let draft_key = draft_key.clone();
            let restore_body = body.clone();
            spawn(async move {
                let failed = run_send(handle, activity, room_id, client_id, body, op_id).await;
                // Restore the draft only on failure, and only if the composer is
                // still empty (never clobber a new message the user has started).
                if failed && text.read().trim().is_empty() {
                    text.set(restore_body.clone());
                    services.preferences().set(draft_key, &restore_body);
                }
            });
        }
    };

    let on_input = {
        let services = services.clone();
        let draft_key = draft_key.clone();
        move |evt: FormEvent| {
            let value = evt.value();
            if value.len() > MAX_MESSAGE_LEN {
                // floor_char_boundary: never split a multibyte char (byte
                // slicing would panic).
                let mut end = MAX_MESSAGE_LEN;
                while !value.is_char_boundary(end) {
                    end -= 1;
                }
                text.set(value[..end].to_string());
                overflow.set(Some(value.len() - end));
            } else {
                text.set(value.clone());
                overflow.set(None);
            }
            // Persist (empty clears); measure to autosize.
            if value.is_empty() {
                services.preferences().remove(&draft_key);
            } else {
                let clamped = if value.len() > MAX_MESSAGE_LEN {
                    value[..MAX_MESSAGE_LEN].to_string()
                } else {
                    value.clone()
                };
                services.preferences().set(draft_key.clone(), &clamped);
            }
            measure();
        }
    };

    let on_keydown = {
        let mut dispatch = dispatch_send.clone();
        move |evt: KeyboardEvent| {
            // Non-compact: Enter sends, Shift+Enter is a newline. Compact: Enter
            // is always a newline (the explicit button is the only send). This
            // reads the injected `Shell`, never a `cfg`/user-agent sniff (§8).
            let shift = evt.modifiers().contains(Modifiers::SHIFT);
            if evt.key() == Key::Enter && !shift && !shell.uses_bottom_bar() {
                evt.prevent_default();
                dispatch();
            }
        }
    };

    let mut send_click = dispatch_send.clone();
    let style = match height() {
        Some(h) => format!("height: {h}px"),
        None => String::new(),
    };

    rsx! {
        form {
            class: "composer",
            id: "composer",
            onsubmit: move |evt| evt.prevent_default(),
            div { class: "composer-row",
                // Attachment handoff is wired to the Files capability seam but
                // degrades to an honest disabled affordance until #181. A failed
                // or absent attach never touches the composed text (§11).
                button {
                    r#type: "button",
                    class: "btn btn-ghost btn-sm composer-attach",
                    id: "composer-attach",
                    disabled: true,
                    "aria-label": "{attach_label}",
                    title: "{attach_unavailable}",
                    "aria-disabled": "true",
                    "+"
                }
                // The control is wrapped by the `Field` primitive for label
                // association (Decision-6): the visible/accessible name is the
                // catalog "Message" label, `for`-linked to `#composer-input`.
                Field { id: "composer-input".to_string(), label: field_label.to_string(),
                    textarea {
                        class: "composer-input",
                        id: "composer-input",
                        rows: "1",
                        style: "{style}",
                        placeholder: "{placeholder}",
                        value: "{text}",
                        oninput: on_input,
                        onkeydown: on_keydown,
                        onmounted: move |evt: MountedEvent| {
                            element.set(Some(evt));
                            measure();
                        },
                        onresize: move |_| measure(),
                    }
                }
                button {
                    r#type: "button",
                    class: "btn btn-primary btn-sm composer-send",
                    id: "composer-send",
                    onclick: move |_| send_click(),
                    "{send_label}"
                }
            }
            if let Some(hint) = enter_hint {
                div { class: "composer-hint muted", id: "composer-hint", "{hint}" }
            }
            if overflow.peek().is_some() {
                div {
                    class: "composer-error muted",
                    id: "composer-error",
                    role: "alert",
                    "{strings.composer_too_long()}",
                }
            }
        }
    }
}
