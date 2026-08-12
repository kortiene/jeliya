//! The form-field primitive (§5.6).
//!
//! A field's `<label>` is associated with its control by `for`/`id` (explicit
//! association) so the accessible name cannot drift from the visual label, and
//! an optional/required marker is a **label fragment**, not a separate visual
//! glyph — a bare `*` is not an accessible signal. The marker text is supplied
//! by the caller (from the catalog), so this primitive carries no copy of its
//! own. Routing forms through this primitive is what lets the scan forbid an
//! unlabelled input elsewhere (Decision-6). The foundation ships no form yet;
//! the primitive exists so the first one has an accessible path and no other.
//!
//! The `hint` is rendered OUTSIDE the `<label>` (so it never folds into the
//! control's accessible NAME) and carries a stable `{id}-hint` id; the control
//! the caller renders as `children` must set `id="{id}"` and
//! `aria-describedby="{id}-hint"`, so the hint is exposed as a DESCRIPTION, not
//! part of the name — the association a wrapping label cannot provide.

use dioxus::prelude::*;

/// A labelled form field. `id` is the control's id, used for the explicit
/// `label[for]` / `aria-describedby` linkage. `label` is the visible and
/// accessible name; `optional_label`, when present, is appended as a label
/// fragment (catalog copy, e.g. "(optional)"); `hint` is help text associated
/// as a DESCRIPTION (id `{id}-hint`). The control is `children`, which MUST
/// carry `id="{id}"` and, when a hint is present, `aria-describedby="{id}-hint"`.
#[component]
pub fn Field(
    id: String,
    label: String,
    #[props(default)] optional_label: Option<String>,
    #[props(default)] hint: Option<String>,
    children: Element,
) -> Element {
    let hint_id = format!("{id}-hint");
    rsx! {
        div { class: "field",
            label { r#for: "{id}",
                "{label}"
                if let Some(optional_label) = optional_label {
                    span { class: "field-optional", "{optional_label}" }
                }
            }
            {children}
            if let Some(hint) = hint {
                // Outside the label, with the id the control points at via
                // aria-describedby — a description, not part of the name.
                span { class: "field-hint", id: "{hint_id}", "{hint}" }
            }
        }
    }
}
