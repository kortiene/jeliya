//! Shared, renderer-agnostic RSX components.
//!
//! **No platform business-logic `cfg` forks live here** (architecture
//! Decision 3 / #174). A component that needs a platform capability takes it as
//! a prop or an injected service; it never branches on `cfg`. Target
//! differences live only in [`crate::compose`] and the per-target `bin`. As a
//! result this whole module compiles identically for native and `wasm32`.
//!
//! Every class name is the one the shared stylesheet (`ui/src/styles.css`,
//! consumed canonically per §7) already styles, so the design system drives
//! this markup unchanged — the property the #158 spike measured byte-identical.
//!
//! **All user-visible copy comes from the catalog** ([`crate::l10n`], #177): no
//! component holds a hardcoded English string (the literal-copy gate enforces
//! this), and dialogs, navigation, status, and forms are reachable **only**
//! through the semantic primitives below (Decision-6).

pub mod a11y;
pub mod activity;
pub mod agents;
pub mod composer;
pub mod confirm;
pub mod diagnostics;
pub mod dialog;
pub mod fleet;
pub mod form;
pub mod global_nav;
pub mod invite_form;
pub mod live_region;
pub mod nav;
pub mod onboarding;
pub mod people;
pub mod read;
pub mod recovery;
pub mod room_shell;
pub mod settings;
pub mod status;
pub mod timeline_row;

pub use a11y::{Heading, MainRegion, SkipLink, SkipLinks, VisuallyHidden};
pub use activity::{ActivityPane, SavedViewMap};
pub use agents::AgentsPane;
pub use confirm::ConfirmDialog;
pub use diagnostics::DiagnosticsDialog;
pub use dialog::Dialog;
pub use fleet::FleetPane;
pub use form::Field;
pub use global_nav::GlobalNav;
pub use invite_form::InviteForm;
pub use live_region::{use_announce, use_announce_context, Announcer, LiveRegion};
pub use nav::NavLandmark;
pub use onboarding::Onboarding;
pub use people::PeoplePane;
pub use recovery::RecoveryBanner;
pub use room_shell::{RoomShell, RoomUnavailable};
pub use settings::SettingsPane;
pub use status::{StatusIndicator, StatusTone};

use dioxus::prelude::*;
use jeliya_api::RoomRow;
use jeliya_client::State;

use crate::l10n::{use_strings, wire};

/// The full-viewport status cover shown when the shell is not mounted:
/// initial activation ("connecting…") and the stop/failure states, each with
/// its own honest label — never a generic "connecting" over a terminal state.
/// Any recorded notice renders INSIDE the cover: `.boot-screen` fills the
/// viewport (`height: var(--vh-full)`), so a sibling would start below the
/// fold and the one diagnostic that explains a failure state would need a
/// scroll nobody knows to perform.
///
/// `target` is already-localized copy chosen by the caller; the heading is the
/// (never-translated) brand from the catalog.
#[component]
pub fn BootScreen(target: String, notice: Option<String>) -> Element {
    let strings = use_strings();
    let brand = strings.app_name();
    rsx! {
        main {
            class: "boot-screen",
            id: "boot-screen",
            // Focusable and focused on mount: when this cover REPLACES the shell (a
            // `Ready → Stopping`/`Stopped`/`Failed` transition, possibly with the
            // Diagnostics dialog open), keyboard/screen-reader focus lands on the
            // cover's heading region instead of falling to the document body — the
            // dialog's opener is unmounted with the shell, so its focus-restoration
            // path cannot run (§5.6).
            tabindex: "-1",
            onmounted: move |evt: MountedEvent| {
                spawn(async move {
                    let _ = evt.set_focus(true).await;
                });
            },
            h1 { "{brand}" }
            p { class: "boot-target mono", "{target}" }
            if let Some(notice) = notice.as_ref() {
                div { class: "error-note", id: "notice", "{notice}" }
            }
        }
    }
}

/// One room row in the sidebar. Renders the room's label and id using the
/// shared `.room-item` styling; selection and actions are later slices. A blank
/// room name renders the catalog's "untitled" label, never an empty row.
#[component]
pub fn RoomListItem(room: RoomRow, selected: bool) -> Element {
    let strings = use_strings();
    let class = if selected {
        "room-item selected"
    } else {
        "room-item"
    };
    let untitled = strings.room_untitled();
    let label = if room.name.trim().is_empty() {
        untitled.to_string()
    } else {
        room.name.clone()
    };
    rsx! {
        div { class: "{class}", "data-room": "{room.room_id}",
            // `.room-select` carries the row's padding, flex growth, and
            // alignment in the canonical stylesheet; `.room-item` is only
            // the border/hover container. Mirrors the React shell's
            // `button.room-select`; selection wiring is a later slice — the
            // element exists here for the design-system layout.
            button { class: "room-select",
                span { class: "room-info",
                    span { class: "room-name-line",
                        span { class: "room-name", "{label}" }
                    }
                }
            }
        }
    }
}

/// The empty-center placeholder shown when no room is open. Its heading is an
/// `<h2>` under the shell's root `<h1>` (the page title): the centre is a
/// section of the page, not the page itself, and making it an h1 would give the
/// mounted shell two h1s on desktop.
#[component]
pub fn EmptyCenter() -> Element {
    let strings = use_strings();
    let choose = strings.center_choose_room();
    rsx! {
        div { class: "center-empty", id: "center-empty",
            Heading { level: 2, class: "center-empty-title".to_string(), "{choose}" }
        }
    }
}

/// The sidebar footer: an accessible connection status (dot + text, never
/// colour-only) plus the **Diagnostics** disclosure — the primitive-backed
/// modal that carries the raw lifecycle state and the raw, secret-scrubbed
/// failure detail out of primary copy (§5.8).
///
/// The footer owns the dialog's open state and restores focus to its trigger on
/// close, so the modal returns focus to the opener as the checklist requires.
#[component]
pub fn StatusFooter(state: State, #[props(default)] detail: Option<String>) -> Element {
    let strings = use_strings();
    let status_word = wire::status_for(strings, state);
    let footer = strings.client_status(status_word);
    let open_label = strings.diagnostics_open();
    let tone = match state {
        State::Ready => StatusTone::Positive,
        State::Interrupted => StatusTone::Warn,
        State::Failed | State::Stopped => StatusTone::Danger,
        State::Idle | State::Connecting | State::Stopping => StatusTone::Neutral,
    };
    let lifecycle_raw = format!("{state:?}");

    let mut open = use_signal(|| false);
    let mut trigger = use_signal(|| None::<MountedEvent>);

    rsx! {
        // `.status-footer` is defined in the canonical `ui/src/styles.css` (a
        // layout-only rule: bottom-anchored flex row, tokens for spacing/border);
        // both clients consume that single source, so this is not a divergent
        // copy (AC-4). The status badge and the Diagnostics control themselves use
        // canonical `.conn-badge`/`.dot`/`.btn` classes. `#status-footer` is the
        // e2e/test hook.
        div { class: "status-footer", id: "status-footer",
            StatusIndicator { tone, label: footer }
            // `btn btn-ghost btn-sm`: the canonical small ghost control the React
            // shell uses (`btn` base = padding/border, `btn-ghost` = transparent
            // tone, `btn-sm` = compact). `btn-ghost` alone is only the tone
            // modifier and renders an unstyled browser-default button.
            button {
                class: "btn btn-ghost btn-sm",
                id: "diagnostics-open",
                onmounted: move |evt: MountedEvent| trigger.set(Some(evt)),
                onclick: move |_| open.set(true),
                "{open_label}"
            }
            if open() {
                DiagnosticsDialog {
                    lifecycle: lifecycle_raw.clone(),
                    detail: detail.clone(),
                    on_close: move |_| {
                        open.set(false);
                        // Return focus to the opener (the checklist's rule).
                        if let Some(element) = trigger() {
                            spawn(async move {
                                let _ = element.set_focus(true).await;
                            });
                        }
                    },
                }
            }
        }
    }
}
