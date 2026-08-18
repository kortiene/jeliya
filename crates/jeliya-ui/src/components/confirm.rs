//! The destructive/sensitive confirmation primitive (§7, D4).
//!
//! Built on [`Dialog`](super::dialog::Dialog), so `role="dialog"`, `aria-modal`,
//! focus containment, Escape, and safe **initial focus on the panel** (never a
//! destructive control) are inherited. On top of that `ConfirmDialog`:
//! - **repeats the room disambiguator** (the short id) in its body, homonym or
//!   not (contract rule 6);
//! - orders **Cancel before Confirm** in the DOM/tab order, so the first
//!   interactive control after the panel is the abandoning one (D4);
//! - is **single-submit** — the confirm control disables the moment it is
//!   pressed and stays disabled until the mutation settles, so a double-press or
//!   a re-render cannot issue the signed, irreversible mutation twice (D4/D5/R7).

use dioxus::prelude::*;

use super::dialog::Dialog;
use crate::l10n::use_strings;

/// A destructive-action confirmation.
///
/// `in_flight` is the parent's mutation guard (disabled while the mutation is
/// settling); `ConfirmDialog` additionally latches its own `submitting` signal
/// on the first press, so the guard holds even before the parent's signal
/// propagates on the next render. `on_confirm` fires **at most once** per open.
#[component]
pub fn ConfirmDialog(
    title: String,
    body: String,
    confirm_label: String,
    room_short: String,
    #[props(default)] in_flight: bool,
    on_confirm: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    let strings = use_strings();
    let cancel_label = strings.confirm_cancel();
    let room_label = strings.confirm_room_label();

    // The single-submit latch: set on the first press, never cleared here (the
    // dialog unmounts when the parent closes it on settle). The confirm control
    // is disabled while EITHER the latch or the parent guard is set, so neither
    // a double-click nor a re-render can dispatch twice.
    let mut submitting = use_signal(|| false);
    let disabled = submitting() || in_flight;

    rsx! {
        Dialog {
            title: title.clone(),
            close_label: cancel_label.to_string(),
            on_close: move |_| on_cancel.call(()),
            div { class: "confirm-body", id: "confirm-body",
                p { class: "confirm-text", "{body}" }
                // The room disambiguator, repeated even with no homonym (rule 6).
                p { class: "confirm-room muted",
                    "{room_label} "
                    span { class: "mono", id: "confirm-room-short", "{room_short}" }
                }
                // Cancel FIRST (the abandoning control), then Confirm — so tab
                // order after the auto-focused panel reaches Cancel before the
                // destructive control.
                div { class: "confirm-actions",
                    button {
                        r#type: "button",
                        class: "btn btn-sm",
                        id: "confirm-cancel",
                        onclick: move |_| on_cancel.call(()),
                        "{cancel_label}"
                    }
                    button {
                        r#type: "button",
                        class: "btn btn-sm btn-danger",
                        id: "confirm-accept",
                        disabled,
                        onclick: move |_| {
                            // Single-submit: latch and fire once. A second press
                            // (or a re-render) sees the latch and does nothing.
                            if !submitting() {
                                submitting.set(true);
                                on_confirm.call(());
                            }
                        },
                        "{confirm_label}"
                    }
                }
            }
        }
    }
}
