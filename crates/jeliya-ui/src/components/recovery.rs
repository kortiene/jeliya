//! The recovery banner (#178 §6.4, AC-5): visible, non-blocking recovery for
//! corrupt/unsupported new-format preference state that was reset to defaults.
//!
//! Fail safe **and** fail visible (D7): corrupt or unsupported-version
//! preference state is never silently reinterpreted. When the schema layer
//! resets it, this banner states the fact plainly and offers an explicit "reset
//! local preferences" action (a full-namespace reset). It rides alongside the
//! shell rather than replacing it — the app stays usable (§5.D).

use dioxus::prelude::*;

use crate::l10n::use_strings;
use crate::shell::bootstrap::RecoveryView;

/// A visible banner shown when corrupt/unsupported new-format preference state
/// was reset. `on_reset` performs the explicit full-namespace reset the user can
/// request; the banner itself only reports and offers the action.
#[component]
pub fn RecoveryBanner(recovery: RecoveryView, on_reset: EventHandler<()>) -> Element {
    let strings = use_strings();
    let title = strings.recovery_title();
    let body = strings.recovery_body();
    let reset = strings.recovery_reset_action();
    // `role: "status"` is owned by the LiveRegion primitive; this banner is a
    // plain, non-live advisory (the boot-time reset already happened), so it is
    // an ordinary landmark-free note, not a live region.
    rsx! {
        div { class: "recovery-banner", id: "recovery-banner",
            div { class: "recovery-copy",
                strong { class: "recovery-title", "{title}" }
                span { class: "recovery-body", "{body}" }
            }
            button {
                class: "btn btn-ghost btn-sm",
                id: "recovery-reset",
                onclick: move |_| on_reset.call(()),
                "{reset}"
            }
        }
    }
}
