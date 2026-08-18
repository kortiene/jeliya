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
use jeliya_client::State;
use jeliya_platform::{Durability, PreferenceKey};

use crate::components::{Field, Heading};
use crate::l10n::{use_locale, use_strings, Locale, LocaleState};
use crate::view::alias::AliasMap;
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
pub fn SettingsPane(
    services: PlatformServices,
    subject_id: Option<SubjectId>,
    /// The client lifecycle state, for the diagnostics card (#180 §7.5).
    #[props(default = State::Idle)]
    lifecycle: State,
    /// The last recorded, secret-scrubbed failure detail, for the diagnostics
    /// card. `None` when nothing has failed.
    #[props(default)]
    detail: Option<String>,
) -> Element {
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
                IdentityRow { services: services.clone(), subject_id: subject_id.clone() }
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

            // -- Device-local aliases (#180 §7.4) -----------------------------
            section { class: "settings-section", id: "settings-aliases",
                AliasEditor { services: services.clone(), self_id: subject_id.clone() }
            }

            // -- Diagnostics card (#180 §7.5) ---------------------------------
            section { class: "settings-section", id: "settings-diagnostics",
                DiagnosticsCard {
                    services: services.clone(),
                    lifecycle,
                    detail,
                    self_id: subject_id,
                }
            }
        }
    }
}

/// The device-local alias editor (#180 §7.4): one editable name per identity id,
/// plus an add-identity row. Names are `PreferenceKey::Aliases` (caller-
/// serialized), stated "on this device, never sent", and excluded from
/// diagnostics (§11).
#[component]
fn AliasEditor(services: PlatformServices, self_id: Option<SubjectId>) -> Element {
    let strings = use_strings();
    let heading = strings.settings_aliases_heading();
    let help = strings.settings_alias_help();
    // Bump to re-read the map after a write (session-scoped prefs).
    let mut version = use_signal(|| 0u32);
    let _ = version();
    let map = AliasMap::parse(
        services
            .preferences()
            .get(&PreferenceKey::Aliases)
            .as_deref(),
    );

    // The rows to show: every existing alias, plus the self id (so the operator
    // can name themselves — self otherwise falls back to "You").
    let mut subjects: Vec<SubjectId> = map.iter().map(|(id, _)| SubjectId::new(id)).collect();
    if let Some(me) = self_id.as_ref() {
        if !subjects.iter().any(|s| s == me) {
            subjects.insert(0, me.clone());
        }
    }

    let _ = heading;
    rsx! {
        Heading { level: 3, class: "settings-section-title".to_string(), "{strings.settings_aliases_heading()}" }
        p { class: "settings-note muted", id: "aliases-help", "{help}" }
        if subjects.is_empty() {
            p { class: "muted", id: "aliases-empty", "{strings.self_you()}" }
        }
        div { class: "alias-list", id: "alias-list",
            for subject in subjects.iter() {
                AliasRow {
                    key: "{subject}",
                    services: services.clone(),
                    subject: subject.clone(),
                    current: map.get(subject).unwrap_or_default().to_string(),
                    on_changed: move |_| {
                        let next = version.peek().wrapping_add(1);
                        version.set(next);
                    },
                }
            }
        }
    }
}

/// One alias row: the identity's short id and an editable name.
#[component]
fn AliasRow(
    services: PlatformServices,
    subject: SubjectId,
    current: String,
    on_changed: EventHandler<()>,
) -> Element {
    let strings = use_strings();
    let short = short_subject(&subject);
    let field_id = format!("alias-{short}");
    let subject_attr = subject.as_str().to_owned();
    rsx! {
        div { class: "alias-row", "data-subject": "{subject_attr}",
            span { class: "alias-subject mono", "{short}" }
            Field { id: field_id.clone(), label: strings.settings_alias_subject_label().to_string(),
                input {
                    class: "input",
                    id: field_id.clone(),
                    r#type: "text",
                    value: "{current}",
                    oninput: move |evt| {
                        let mut map = AliasMap::parse(
                            services.preferences().get(&PreferenceKey::Aliases).as_deref(),
                        );
                        map.set(&subject, &evt.value());
                        services.preferences().set(PreferenceKey::Aliases, &map.serialize());
                        on_changed.call(());
                    },
                }
            }
        }
    }
}

/// The diagnostics card (#180 §7.5): bounded (a fixed field set), redacted
/// (secret-scrubbed; no self-label, aliases, capabilities, or full identities),
/// and actionable (a copy action). The same content the `DiagnosticsDialog`
/// discloses, inline.
#[component]
fn DiagnosticsCard(
    services: PlatformServices,
    lifecycle: State,
    detail: Option<String>,
    self_id: Option<SubjectId>,
) -> Element {
    let strings = use_strings();
    let heading = strings.settings_diagnostics_heading();
    let state_label = strings.settings_diagnostics_state_label();
    let detail_label = strings.settings_diagnostics_detail_label();
    let no_detail = strings.diagnostics_no_detail();
    let redaction = strings.settings_diagnostics_redaction_note();

    // The BOUNDED, REDACTED field set: the client state, a SHORTENED identity
    // (never the full id), and the already-scrubbed last-error detail. The
    // self-label, alias map, and any minted capability are deliberately absent.
    let lifecycle_raw = format!("{lifecycle:?}");
    let identity_short = self_id.as_ref().map(short_subject);
    let copy_text =
        diagnostics_copy_text(&lifecycle_raw, identity_short.as_deref(), detail.as_deref());

    rsx! {
        Heading { level: 3, class: "settings-section-title".to_string(), "{heading}" }
        dl { class: "diagnostics-card", id: "diagnostics-card",
            dt { "{state_label}" }
            dd { class: "mono", id: "diagnostics-card-state", "{lifecycle_raw}" }
            if let Some(short) = identity_short.as_ref() {
                dt { "{strings.identity_id_label()}" }
                dd { class: "mono", id: "diagnostics-card-identity", "{short}" }
            }
            dt { "{detail_label}" }
            match detail.as_ref() {
                Some(detail) => rsx! { dd { class: "mono", id: "diagnostics-card-detail", "{detail}" } },
                None => rsx! { dd { class: "muted", id: "diagnostics-card-detail", "{no_detail}" } },
            }
        }
        p { class: "settings-note muted", id: "diagnostics-redaction", "{redaction}" }
        button {
            r#type: "button",
            class: "btn btn-sm",
            id: "diagnostics-copy",
            onclick: move |_| {
                let services = services.clone();
                let text = copy_text.clone();
                spawn(async move {
                    let _ = services.clipboard().write_text(&text).await;
                });
            },
            "{strings.settings_diagnostics_copy()}"
        }
    }
}

/// Build the copyable diagnostics text — the same bounded, redacted field set.
fn diagnostics_copy_text(
    lifecycle: &str,
    identity_short: Option<&str>,
    detail: Option<&str>,
) -> String {
    let mut out = format!("state: {lifecycle}\n");
    if let Some(short) = identity_short {
        out.push_str(&format!("identity: {short}\n"));
    }
    match detail {
        Some(detail) => out.push_str(&format!("last_error: {detail}\n")),
        None => out.push_str("last_error: none\n"),
    }
    out
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

    // ---- diagnostics_copy_text: AC-6 bounded, redacted, no secrets --------

    #[test]
    fn diagnostics_copy_text_contains_lifecycle_state() {
        let text = diagnostics_copy_text("Ready", None, None);
        assert!(text.contains("state: Ready"), "lifecycle state must appear");
    }

    #[test]
    fn diagnostics_copy_text_includes_short_identity_when_supplied() {
        // The caller is responsible for shortening the id before passing it
        // here; this test verifies the field is forwarded verbatim and that
        // no full-id prefix (e.g. "blake3:") is added by the function itself.
        let text = diagnostics_copy_text("Ready", Some("abcdefghijkl"), None);
        assert!(text.contains("identity: abcdefghijkl"));
        assert!(!text.contains("blake3:"), "full id prefix must not appear");
    }

    #[test]
    fn diagnostics_copy_text_omits_identity_line_when_none() {
        let text = diagnostics_copy_text("Idle", None, None);
        assert!(
            !text.contains("identity:"),
            "identity line must be absent when the subject_id is unknown"
        );
    }

    #[test]
    fn diagnostics_copy_text_says_none_when_no_detail() {
        let text = diagnostics_copy_text("Ready", None, None);
        assert!(
            text.contains("last_error: none"),
            "absent detail renders as none"
        );
        assert!(
            !text.contains("last_error: none\nlast_error:"),
            "must not be doubled"
        );
    }

    #[test]
    fn diagnostics_copy_text_includes_detail_when_present() {
        let text = diagnostics_copy_text("Ready", None, Some("connection refused"));
        assert!(text.contains("last_error: connection refused"));
        assert!(!text.contains("last_error: none"));
    }

    #[test]
    fn diagnostics_copy_text_all_fields_present_and_bounded() {
        // With all three inputs: state + identity + detail, no extra fields.
        let text = diagnostics_copy_text("Reconnecting", Some("shortid123"), Some("timeout"));
        assert!(text.contains("state: Reconnecting"));
        assert!(text.contains("identity: shortid123"));
        assert!(text.contains("last_error: timeout"));
        // Bounded: no unexpected sensitive fields (no self-label, no full
        // capabilities, no raw error backtraces).
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "exactly three lines: state, identity, last_error"
        );
    }

    // ---- parse_locale_choice: locale switcher correctness -----------------

    #[test]
    fn parse_locale_choice_maps_en_and_fr_tags_to_explicit() {
        assert_eq!(
            parse_locale_choice(Locale::En.tag()),
            LocaleChoice::Explicit(Locale::En)
        );
        assert_eq!(
            parse_locale_choice(Locale::Fr.tag()),
            LocaleChoice::Explicit(Locale::Fr)
        );
    }

    #[test]
    fn parse_locale_choice_falls_back_to_system_for_unknown_input() {
        // Any value that is not a known BCP-47 tag maps to System (clears
        // the explicit preference).
        assert_eq!(
            parse_locale_choice(LOCALE_VALUE_SYSTEM),
            LocaleChoice::System
        );
        assert_eq!(parse_locale_choice("de"), LocaleChoice::System);
        assert_eq!(parse_locale_choice(""), LocaleChoice::System);
    }
}
