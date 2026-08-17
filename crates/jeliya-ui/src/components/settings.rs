//! The Settings destination (#178 §5.F): identity display, live locale
//! switchers, and the device-local self-label editor.
//!
//! Unlike the other destinations, Settings carries real in-scope content:
//! - **Identity** — the `subject_id`, shortened and copyable, described as the
//!   unrecoverable P2P identity.
//! - **Language** — the live text-locale and formatting-locale switchers that
//!   assign the resolved-locale context signal (`use_locale_context`, provided by
//!   `app.rs`). Selecting a locale writes the preference and applies live (no
//!   reload), honestly noting browser session-only durability.
//! - **Self-label** — the device-local name (`PreferenceKey::SelfLabel`), stated
//!   "on this device, never sent".

use dioxus::prelude::*;
use jeliya_api::SubjectId;
use jeliya_platform::{Durability, PreferenceKey};

use crate::components::{Field, Heading};
use crate::l10n::{use_locale, Locale, LocaleState};
use crate::PlatformServices;

/// A short, copyable rendering of the subject id (last colon-segment, truncated)
/// — the same disambiguator discipline the room header uses.
fn short_subject(id: &SubjectId) -> String {
    let raw = id.as_str();
    let tail = raw.rsplit(':').next().unwrap_or(raw);
    tail.chars().take(12).collect()
}

/// The Settings destination pane.
///
/// `subject_id` is the local identity from the connection snapshot (`None` while
/// unknown). `services` is the injected platform-authority seam (preferences,
/// clipboard).
#[component]
pub fn SettingsPane(services: PlatformServices, subject_id: Option<SubjectId>) -> Element {
    // The resolved-locale context signal AppRoot provides; writing it applies a
    // switch live (every consumer reads it per render).
    let mut locale = use_context::<Signal<LocaleState>>();
    let resolved = use_locale();
    let strings = crate::l10n::catalog_for(resolved.text);

    let heading = strings.settings_heading();
    let identity_heading = strings.settings_identity_heading();
    let language_heading = strings.settings_language_heading();
    let session_note = strings.settings_session_only_note();
    let is_session_only = services.preferences().durability() == Durability::SessionScoped;

    rsx! {
        section { class: "settings-pane", id: "settings-pane",
            Heading { level: 2, class: "settings-title".to_string(), "{heading}" }

            // -- Identity -----------------------------------------------------
            section { class: "settings-section", id: "settings-identity",
                Heading { level: 3, class: "settings-section-title".to_string(), "{identity_heading}" }
                IdentityRow { services: services.clone(), subject_id }
            }

            // -- Language -----------------------------------------------------
            section { class: "settings-section", id: "settings-language",
                Heading { level: 3, class: "settings-section-title".to_string(), "{language_heading}" }
                LocaleSelect {
                    id: "settings-text-locale".to_string(),
                    label: strings.settings_text_locale_label().to_string(),
                    selected: resolved.text,
                    follows_system: resolved.text_follows_system,
                    on_pick: {
                        let services = services.clone();
                        move |choice: LocaleChoice| {
                            apply_text_locale(&services, &mut locale, choice);
                        }
                    },
                }
                LocaleSelect {
                    id: "settings-formatting-locale".to_string(),
                    label: strings.settings_formatting_locale_label().to_string(),
                    selected: resolved.formatting,
                    follows_system: false,
                    on_pick: {
                        let services = services.clone();
                        move |choice: LocaleChoice| {
                            apply_formatting_locale(&services, &mut locale, choice);
                        }
                    },
                }
                if is_session_only {
                    p { class: "settings-note muted", id: "settings-session-note", "{session_note}" }
                }
            }

            // -- Self-label ---------------------------------------------------
            section { class: "settings-section", id: "settings-self-label",
                SelfLabelEditor { services: services.clone() }
            }
        }
    }
}

/// The identity row: the shortened, copyable subject id + the unrecoverable note.
#[component]
fn IdentityRow(services: PlatformServices, subject_id: Option<SubjectId>) -> Element {
    let strings = crate::l10n::use_strings();
    let id_label = strings.identity_id_label();
    let copy = strings.identity_copy();
    let unrecoverable = strings.identity_unrecoverable();

    let full = subject_id.as_ref().map(|id| id.as_str().to_owned());
    let short = subject_id.as_ref().map(short_subject);

    rsx! {
        div { class: "identity-row",
            span { class: "identity-label", "{id_label}" }
            if let (Some(short), Some(full)) = (short, full) {
                span { class: "identity-id mono", id: "identity-id", "{short}" }
                button {
                    class: "btn btn-ghost btn-sm",
                    id: "identity-copy",
                    onclick: move |_| {
                        let services = services.clone();
                        let full = full.clone();
                        spawn(async move {
                            let _ = services.clipboard().write_text(&full).await;
                        });
                    },
                    "{copy}"
                }
            }
        }
        p { class: "identity-note muted", "{unrecoverable}" }
    }
}

/// The self-label editor. Validation (contract §"Identity, aliases, and self
/// label"): trim, empty clears, soft 40-char cap. Writes `PreferenceKey::SelfLabel`.
#[component]
fn SelfLabelEditor(services: PlatformServices) -> Element {
    let strings = crate::l10n::use_strings();
    let label = strings.self_label_label().to_string();
    let help = strings.self_label_help().to_string();
    let current = services
        .preferences()
        .get(&PreferenceKey::SelfLabel)
        .unwrap_or_default();
    // The Field id is a String expression, so the hint association must be a
    // DYNAMIC `aria-describedby` that constructs the same `{id}-hint` inline —
    // a runtime value, not a literal (#177 form-association gate).
    let base = "self-label";

    rsx! {
        Field { id: base.to_string(), label, hint: help.clone(),
            input {
                class: "input",
                id: base.to_string(),
                aria_describedby: format!("{base}-hint"),
                value: "{current}",
                oninput: move |evt| {
                    let value = normalize_self_label(&evt.value());
                    if value.is_empty() {
                        services.preferences().remove(&PreferenceKey::SelfLabel);
                    } else {
                        services.preferences().set(PreferenceKey::SelfLabel, &value);
                    }
                },
            }
        }
    }
}

/// Trim and soft-cap the self-label at 40 characters (contract).
fn normalize_self_label(raw: &str) -> String {
    raw.trim().chars().take(40).collect()
}

/// A locale switcher choice: a concrete locale, or "follow the platform".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LocaleChoice {
    /// Follow the platform's preferred language (clears the explicit preference).
    System,
    /// A concrete locale.
    Explicit(Locale),
}

/// The `<option>` value for the "follow system" choice — a structural form key
/// submitted by the `<select>`, not user-visible copy (the visible label comes
/// from the catalog).
// i18n-exempt: structural <option> form value (the submitted key), not copy.
const LOCALE_VALUE_SYSTEM: &str = "system";

/// Parse a `<select>` value back into a [`LocaleChoice`]. The concrete-locale
/// values are the locale BCP-47 tags (`Locale::tag`), so the comparison and the
/// option values share one source. Kept out of the `rsx!` closure so its
/// comparison strings are plain logic, not scanned as component copy.
fn parse_locale_choice(value: &str) -> LocaleChoice {
    if value == Locale::En.tag() {
        LocaleChoice::Explicit(Locale::En)
    } else if value == Locale::Fr.tag() {
        LocaleChoice::Explicit(Locale::Fr)
    } else {
        LocaleChoice::System
    }
}

/// A language's endonym — the name it gives itself. Tier-3 (a language names
/// itself the same in every UI language), so it is deliberately not catalog copy
/// (docs/glossary-fr.md). A plain function so no endonym literal sits in `rsx!`.
fn endonym(locale: Locale) -> &'static str {
    match locale {
        Locale::En => "English",
        Locale::Fr => "Français",
    }
}

/// A `<select>` locale switcher. `selected` is the resolved locale; a `System`
/// option is offered when the value is currently following the platform.
#[component]
fn LocaleSelect(
    id: String,
    label: String,
    selected: Locale,
    follows_system: bool,
    on_pick: EventHandler<LocaleChoice>,
) -> Element {
    let strings = crate::l10n::use_strings();
    let system = strings.settings_locale_follow_system();
    let en_tag = Locale::En.tag();
    let fr_tag = Locale::Fr.tag();
    let en_name = endonym(Locale::En);
    let fr_name = endonym(Locale::Fr);
    let system_selected = follows_system;
    let en_selected = !follows_system && selected == Locale::En;
    let fr_selected = !follows_system && selected == Locale::Fr;

    rsx! {
        Field { id: id.clone(), label,
            select {
                class: "input",
                id: id.clone(),
                onchange: move |evt| on_pick.call(parse_locale_choice(&evt.value())),
                // The "follow system" option IS catalog copy; the concrete-locale
                // options are language endonyms (Tier-3, self-named) with the
                // locale tag as the structural value.
                option { value: LOCALE_VALUE_SYSTEM, selected: system_selected, "{system}" }
                option { value: "{en_tag}", selected: en_selected, "{en_name}" }
                option { value: "{fr_tag}", selected: fr_selected, "{fr_name}" }
            }
        }
    }
}

/// Apply a text-locale choice: write/clear the preference and update the live
/// context signal so every consumer re-renders in the new language with no reload.
fn apply_text_locale(
    services: &PlatformServices,
    locale: &mut Signal<LocaleState>,
    choice: LocaleChoice,
) {
    let mut state = locale.peek().to_owned();
    match choice {
        LocaleChoice::Explicit(picked) => {
            services
                .preferences()
                .set(PreferenceKey::TextLocale, picked.tag());
            state.text = picked;
            state.text_follows_system = false;
        }
        LocaleChoice::System => {
            services.preferences().remove(&PreferenceKey::TextLocale);
            state.text_follows_system = true;
        }
    }
    locale.set(state);
}

/// Apply a formatting-locale choice — independent of the text locale (D3 of
/// #177): a Bambara UI on a French system still formats the French way.
fn apply_formatting_locale(
    services: &PlatformServices,
    locale: &mut Signal<LocaleState>,
    choice: LocaleChoice,
) {
    let mut state = locale.peek().to_owned();
    match choice {
        LocaleChoice::Explicit(picked) => {
            services
                .preferences()
                .set(PreferenceKey::FormattingLocale, picked.tag());
            state.formatting = picked;
        }
        LocaleChoice::System => {
            services
                .preferences()
                .remove(&PreferenceKey::FormattingLocale);
        }
    }
    locale.set(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_label_is_trimmed_and_soft_capped() {
        assert_eq!(normalize_self_label("  Ada  "), "Ada");
        assert_eq!(normalize_self_label("   "), "");
        let long = "x".repeat(60);
        assert_eq!(normalize_self_label(&long).chars().count(), 40);
    }

    #[test]
    fn short_subject_takes_the_tail() {
        assert_eq!(
            short_subject(&SubjectId::new("blake3:abcdefghijklmnop")),
            "abcdefghijkl"
        );
    }
}
