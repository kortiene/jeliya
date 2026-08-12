//! The status-vocabulary primitive (§5.6).
//!
//! Status is **two separate facts**: a dot (tone) and a text label. Never
//! colour-only — a colour a colour-blind user cannot distinguish is not a
//! status. The dot carries `aria-hidden` because the label already states the
//! fact; the label is the accessible truth.

use dioxus::prelude::*;

/// The tone of a status dot. Maps to a shared class; the label, not the colour,
/// is the meaning.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusTone {
    /// Neutral / in-progress.
    Neutral,
    /// Healthy / connected.
    Positive,
    /// Recovering / attention.
    Warn,
    /// Failed / disconnected.
    Danger,
}

impl StatusTone {
    /// The tone modifier class appended to `.status-dot`.
    fn class(self) -> &'static str {
        match self {
            StatusTone::Neutral => "status-dot",
            StatusTone::Positive => "status-dot is-positive",
            StatusTone::Warn => "status-dot is-warn",
            StatusTone::Danger => "status-dot is-danger",
        }
    }
}

/// A status indicator: a decorative dot plus a text label. The label is always
/// present, so the status is legible without colour.
#[component]
pub fn StatusIndicator(tone: StatusTone, label: String) -> Element {
    rsx! {
        span { class: "status",
            span { class: "{tone.class()}", "aria-hidden": "true" }
            span { class: "status-label", "{label}" }
        }
    }
}
