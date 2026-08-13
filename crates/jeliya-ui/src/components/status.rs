//! The status-vocabulary primitive (§5.6).
//!
//! Status is **two separate facts**: a dot (tone) and a text label. Never
//! colour-only — a colour a colour-blind user cannot distinguish is not a
//! status. The dot carries `aria-hidden` because the label already states the
//! fact; the label is the accessible truth.
//!
//! Rendered with the canonical `.conn-badge` + `.dot` classes the shared
//! stylesheet already defines (AC-4), not re-invented `.status-*` classes.

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
    /// The CANONICAL badge class for this tone: the shared stylesheet's
    /// `.conn-badge` (base = neutral grey) plus the connection-state modifier
    /// that colours it, exactly as the React shell's connection badge does
    /// (`ui/src/components/Sidebar.tsx`). No divergent `.status-*` classes —
    /// AC-4: consume the canonical design system, never re-invent it. The inner
    /// `.dot` inherits the badge's `currentColor`, so it is coloured by the same
    /// state class.
    fn badge_class(self) -> &'static str {
        match self {
            // Base `.conn-badge` is `color: var(--text-mute)` — the neutral tone.
            StatusTone::Neutral => "conn-badge",
            StatusTone::Positive => "conn-badge conn-connected",
            StatusTone::Warn => "conn-badge conn-reconnecting",
            StatusTone::Danger => "conn-badge conn-disconnected",
        }
    }
}

/// A status indicator: a decorative dot plus a text label, rendered as the
/// canonical `.conn-badge`. The label is always present, so the status is
/// legible without colour; the `.dot` is `aria-hidden` because the label
/// already states the fact.
#[component]
pub fn StatusIndicator(tone: StatusTone, label: String) -> Element {
    rsx! {
        span { class: "{tone.badge_class()}",
            span { class: "dot", "aria-hidden": "true" }
            " "
            "{label}"
        }
    }
}
