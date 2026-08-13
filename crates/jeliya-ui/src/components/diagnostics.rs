//! The Diagnostics dialog (§5.8) — the correct home for raw lifecycle state and
//! the raw failure detail (`"room.list: {error:?}"` leaves primary copy).
//!
//! Built on [`Dialog`](super::dialog::Dialog), so it is the foundation's real
//! exercise of the modal primitive (a landmark axe and keyboard rules can mean
//! something against). The detail is already secret-scrubbed at its source
//! ([`crate::l10n::ErrorDisplay::diagnostic_detail`]); nothing token-shaped
//! reaches these DOM nodes, and the labels are catalog copy while the values are
//! raw developer-facing English.

use dioxus::prelude::*;

use super::dialog::Dialog;
use crate::l10n::use_strings;

/// The Diagnostics dialog. `lifecycle` is the raw client state; `detail` is the
/// raw, scrubbed failure text (or `None`). `on_close` bubbles to the opener,
/// which restores focus to its trigger.
#[component]
pub fn DiagnosticsDialog(
    lifecycle: String,
    #[props(default)] detail: Option<String>,
    on_close: EventHandler<()>,
) -> Element {
    let strings = use_strings();
    let title = strings.diagnostics_title();
    let close_label = strings.diagnostics_close();
    let lifecycle_label = strings.diagnostics_lifecycle_label();
    let detail_label = strings.diagnostics_detail_label();
    let no_detail = strings.diagnostics_no_detail();

    rsx! {
        Dialog {
            title: title.to_string(),
            close_label: close_label.to_string(),
            on_close,
            dl { class: "diagnostics", id: "diagnostics-body",
                dt { "{lifecycle_label}" }
                dd { class: "mono", id: "diagnostics-lifecycle", "{lifecycle}" }
                dt { "{detail_label}" }
                match detail.as_ref() {
                    Some(detail) => rsx! {
                        dd { class: "mono", id: "diagnostics-detail", "{detail}" }
                    },
                    None => rsx! {
                        dd { class: "muted", id: "diagnostics-detail", "{no_detail}" }
                    },
                }
            }
        }
    }
}
