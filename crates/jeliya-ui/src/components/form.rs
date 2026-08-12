//! The form-field primitive (§5.6).
//!
//! A field's `<label>` is associated with its control by wrapping (implicit
//! association) so the accessible name cannot drift from the visual label, and
//! an optional/required marker is a **label fragment**, not a separate visual
//! glyph — a bare `*` is not an accessible signal. The marker text is supplied
//! by the caller (from the catalog), so this primitive carries no copy of its
//! own. Routing forms through this primitive is what lets the scan forbid an
//! unlabelled input elsewhere (Decision-6). The foundation ships no form yet;
//! the primitive exists so the first one has an accessible path and no other.

use dioxus::prelude::*;

/// A labelled form field. `label` is the visible and accessible name;
/// `optional_label`, when present, is appended to the label as a text fragment
/// (the caller passes catalog copy, e.g. "(optional)"); `hint` is associated
/// help text. The control(s) are `children`, wrapped by the `<label>` so
/// association needs no matching `for`/`id`.
#[component]
pub fn Field(
    label: String,
    #[props(default)] optional_label: Option<String>,
    #[props(default)] hint: Option<String>,
    children: Element,
) -> Element {
    rsx! {
        label { class: "field",
            span {
                "{label}"
                if let Some(optional_label) = optional_label {
                    span { class: "field-optional", "{optional_label}" }
                }
            }
            {children}
            if let Some(hint) = hint {
                span { class: "field-hint", "{hint}" }
            }
        }
    }
}
