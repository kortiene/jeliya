//! The canonical EN/FR localization layer for the Dioxus stack (#177, AC-1/2/3).
//!
//! This is the **one authoritative catalog** for the surviving stack. React
//! (`ui/src/l10n/`) and Flutter (`app/lib/src/l10n/`) are requirements-mining
//! input only; neither is a parity authority, and this crate does not add a
//! third *parallel* catalog to hand-maintain — it replaces them as they retire
//! (#200/#201/#202).
//!
//! Design (spec §4-D1, §5.2):
//!
//! - The [`Catalog`] trait declares every message **once**. Plain messages are
//!   `&'static str`; parameterized messages are methods with **typed
//!   arguments**, so a missing or mistyped argument is a compile error — the
//!   Rust analogue of the React `MessageFn<[room: string]>` contract.
//! - [`En`] is the source of truth (reviewed as product copy). [`Fr`]
//!   implements the same trait; **a missing key does not compile**, so `rustc`
//!   enforces key and placeholder parity exactly as `tsc` does on the React
//!   side. The node gate (`scripts/check-jeliya-ui-i18n.mjs`) is defence in
//!   depth for what types cannot see: an empty value, a French value left in
//!   English, plural-category coverage, French typography, and hardcoded
//!   component literals.
//! - **No runtime i18n dependency.** The two supported locales are a static
//!   dispatch table; formatting is a small internal table (`format.rs`), not
//!   `Intl`/`icu4x` (deferred — spec §14 Q2).
//!
//! Live switching (§5.3): the resolved locale lives in a Dioxus context signal.
//! [`use_strings`] and [`use_formats`] read it **per render**, so flipping
//! either preference re-resolves every consumer with no restart.

use dioxus::prelude::*;

mod en;
pub mod error;
mod format;
mod fr;
pub mod palette;
pub mod plural;
pub mod tokens;
pub mod wire;

pub use en::En;
pub use error::{ErrorDisplay, FriendlyError};
pub use format::Formats;
pub use fr::Fr;
pub use plural::{category as plural_category, PluralCategory};

/// A locale with a complete catalog. A tag outside this set falls back:
/// shipping a half-translated interface is worse than shipping one language.
///
/// The same enum names both the **text** locale (which catalog renders copy)
/// and the resolved **formatting convention** (numeric/calendar rules), because
/// for the foundation the supported conventions are exactly the supported
/// languages. The two are still tracked independently in [`LocaleState`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Locale {
    /// English (source of truth).
    En,
    /// French.
    Fr,
}

impl Locale {
    /// The BCP-47 primary subtag this locale renders under (`"en"` / `"fr"`),
    /// used for `<html lang>` and as the identity of the locale on the wire of
    /// a stored preference.
    pub fn tag(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Fr => "fr",
        }
    }

    /// Resolve a BCP-47 tag to a supported locale by its **primary subtag**, so
    /// `fr-CA` and `fr-BE` both resolve to French — a regional variant must not
    /// silently fall through to English. `None` when no primary subtag is
    /// supported.
    pub fn from_tag(tag: &str) -> Option<Locale> {
        let primary = tag
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match primary.as_str() {
            "en" => Some(Locale::En),
            "fr" => Some(Locale::Fr),
            _ => None,
        }
    }
}

/// Every locale with a complete catalog, in preference order.
pub const SUPPORTED_LOCALES: [Locale; 2] = [Locale::En, Locale::Fr];

/// The locale used when nothing else resolves. English, because the source of
/// truth is always complete.
pub const FALLBACK_LOCALE: Locale = Locale::En;

/// The static catalog for a locale. Returned as `&'static dyn Catalog` so a
/// component holds one trait object regardless of language, and a live switch is
/// just a different pointer on the next render.
pub fn catalog_for(locale: Locale) -> &'static dyn Catalog {
    match locale {
        Locale::En => &En,
        Locale::Fr => &Fr,
    }
}

/// The resolved locale pair a render sees. `Copy`, so it rides in a signal and a
/// switch is a plain assignment.
///
/// `text` and `formatting` are **independent** (§5.3): text=fr/format=en and
/// text=en/format=fr are both expressible, which is what "independently
/// switchable" requires. `text_follows_system` records whether the text locale
/// came from the platform rather than an explicit choice, so a settings picker
/// can show "follow system" honestly rather than pretend the user picked it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LocaleState {
    /// The catalog copy renders from.
    pub text: Locale,
    /// The convention numbers, dates, and percentages format under.
    pub formatting: Locale,
    /// True when `text` came from the platform, not an explicit preference.
    pub text_follows_system: bool,
}

impl LocaleState {
    /// Resolve both locales from the two persisted preferences plus the platform
    /// signal (§5.3).
    ///
    /// - Text: the explicit preference, else the platform language narrowed to a
    ///   supported catalog, else [`FALLBACK_LOCALE`].
    /// - Formatting: the explicit preference resolved to a supported convention,
    ///   else the platform formatting tag resolved likewise, else the text
    ///   locale's own conventions (the documented fallback for an unsupported
    ///   third formatting locale — spec §4-D4).
    pub fn resolve(
        text_pref: Option<&str>,
        formatting_pref: Option<&str>,
        platform_text: Option<&str>,
        platform_formatting: Option<&str>,
    ) -> LocaleState {
        let explicit_text = text_pref.and_then(Locale::from_tag);
        let text = explicit_text
            .or_else(|| platform_text.and_then(Locale::from_tag))
            .unwrap_or(FALLBACK_LOCALE);
        let formatting = formatting_pref
            .and_then(Locale::from_tag)
            .or_else(|| platform_formatting.and_then(Locale::from_tag))
            .unwrap_or(text);
        LocaleState {
            text,
            formatting,
            text_follows_system: explicit_text.is_none(),
        }
    }
}

impl Default for LocaleState {
    fn default() -> Self {
        LocaleState {
            text: FALLBACK_LOCALE,
            formatting: FALLBACK_LOCALE,
            text_follows_system: true,
        }
    }
}

/// The canonical message set. Every user-visible string in the Dioxus stack is
/// declared here exactly once, so [`Fr`] cannot omit one without a compile
/// error.
///
/// Method-per-message rather than a field struct because a parameterized
/// message needs typed arguments (`client_status(&self, lifecycle: &str)`), and
/// a trait method is the only shape that gives both the plain and the
/// parameterized case one uniform, compiler-checked declaration.
///
/// A sentence with styled or interactive segments is **one** message with slot
/// markers, rendered by a template helper — never fragments concatenated in RSX
/// (i18n.md rule 2). The foundation carries no such multi-slot sentence yet.
pub trait Catalog {
    /// The BCP-47 tag this catalog renders under. Excepted from the French
    /// typography gate (it is an identifier, not copy).
    fn locale_tag(&self) -> &'static str;
    /// The product name. Never translated (brand, Tier 3).
    fn app_name(&self) -> &'static str;

    /// Boot cover: the client is establishing the first connection.
    fn boot_connecting(&self) -> &'static str;
    /// Boot cover: the client is draining accepted work before stopping.
    fn boot_stopping(&self) -> &'static str;
    /// Boot cover: the client has stopped.
    fn boot_stopped(&self) -> &'static str;
    /// Boot cover: the client failed and will not retry.
    fn boot_failed(&self) -> &'static str;

    /// The rooms navigation landmark's accessible name.
    fn rooms_heading(&self) -> &'static str;
    /// The sidebar empty state, shown only after the first reply lands.
    fn rooms_empty(&self) -> &'static str;
    /// The sidebar loading state, shown before the first reply.
    fn rooms_loading(&self) -> &'static str;
    /// The label for a room whose name is blank.
    fn room_untitled(&self) -> &'static str;
    /// The empty-center placeholder when no room is open.
    fn center_choose_room(&self) -> &'static str;

    /// Skip link: jump focus to the rooms navigation.
    fn skip_to_rooms(&self) -> &'static str;
    /// Skip link: jump focus to the main content region.
    fn skip_to_content(&self) -> &'static str;

    /// The status footer's Diagnostics disclosure trigger.
    fn diagnostics_open(&self) -> &'static str;
    /// The Diagnostics dialog title.
    fn diagnostics_title(&self) -> &'static str;
    /// The Diagnostics dialog's close control.
    fn diagnostics_close(&self) -> &'static str;
    /// The Diagnostics dialog's lifecycle-state row label.
    fn diagnostics_lifecycle_label(&self) -> &'static str;
    /// The Diagnostics dialog's raw-error row label.
    fn diagnostics_detail_label(&self) -> &'static str;
    /// The Diagnostics dialog's placeholder when no error has been recorded.
    fn diagnostics_no_detail(&self) -> &'static str;

    /// Friendly title for a failed room-list read (primary copy).
    fn err_room_list_title(&self) -> &'static str;
    /// Friendly body for a RETRYABLE failed room-list read (a transient
    /// disconnect the shell will retry on recovery) — may promise recovery.
    fn err_room_list_message(&self) -> &'static str;
    /// Friendly body for a TERMINAL failed room-list read (a refusal / timeout /
    /// decode failure the shell will NOT retry) — must not promise recovery.
    fn err_room_list_terminal_message(&self) -> &'static str;
    /// Generic friendly title for an unrecognized error code.
    fn err_unknown_title(&self) -> &'static str;
    /// Generic friendly body for an unrecognized error code.
    fn err_unknown_message(&self) -> &'static str;

    /// Status vocabulary: connecting.
    fn status_connecting(&self) -> &'static str;
    /// Status vocabulary: connected and ready.
    fn status_ready(&self) -> &'static str;
    /// Status vocabulary: was ready, reconnecting.
    fn status_interrupted(&self) -> &'static str;
    /// Status vocabulary: stopping.
    fn status_stopping(&self) -> &'static str;
    /// Status vocabulary: stopped.
    fn status_stopped(&self) -> &'static str;
    /// Status vocabulary: failed / disconnected with no retry.
    fn status_failed(&self) -> &'static str;

    /// A polite live-region announcement of a connection-lifecycle transition,
    /// given the already-localized status word. Distinct from the visual footer
    /// text so a screen-reader user not focused on the footer still hears a
    /// connection loss or recovery (§5.6).
    fn conn_announcement(&self, status: &str) -> String;

    /// Wire role `authority` — the room's authority (jeliya_api's `Role::Authority`;
    /// the v1 `owner` spelling is retired). Translatable display, NOT a Tier-2 token.
    fn wire_role_authority(&self) -> &'static str;
    /// Wire role `member`.
    fn wire_role_member(&self) -> &'static str;
    /// Wire member status `active` (signed membership).
    fn wire_status_active(&self) -> &'static str;
    /// Wire member status `invited`.
    fn wire_status_invited(&self) -> &'static str;
    /// Wire member status `left`.
    fn wire_status_left(&self) -> &'static str;
    /// Wire member status `removed`.
    fn wire_status_removed(&self) -> &'static str;
    /// The empty member status — the daemon reported nothing, not an unknown
    /// value it did report (which passes through raw).
    fn wire_status_unknown(&self) -> &'static str;
    /// Wire peer path `direct` (kept verbatim; Tier 2).
    fn wire_path_direct(&self) -> &'static str;
    /// Wire peer path `relay` (kept verbatim; Tier 2).
    fn wire_path_relay(&self) -> &'static str;

    /// The percent template. Owns the locale-dependent SPACING: English `42%`,
    /// French `42 %` with a U+202F narrow no-break space.
    fn format_percent(&self, n: &str) -> String;
    /// Byte-size unit word: bytes. English `{n} B`, French `{n} o` (octets).
    fn format_bytes_b(&self, n: &str) -> String;
    /// Byte-size unit word: kibibytes. English `{n} KB`, French `{n} Ko`.
    fn format_bytes_kb(&self, n: &str) -> String;
    /// Byte-size unit word: mebibytes. English `{n} MB`, French `{n} Mo`.
    fn format_bytes_mb(&self, n: &str) -> String;
    /// Byte-size unit word: gibibytes. English `{n} GB`, French `{n} Go`.
    fn format_bytes_gb(&self, n: &str) -> String;
    /// The "today" day-divider label (vocabulary, text locale).
    fn format_today(&self) -> &'static str;
    /// The "yesterday" day-divider label (vocabulary, text locale).
    fn format_yesterday(&self) -> &'static str;
    /// The month name for `1..=12` (vocabulary, text locale): English `January`,
    /// French `janvier`. `None` outside that range — a total function so a stray
    /// index degrades ([`format::Formats::date`] renders just the day) instead of
    /// crashing a render; callers pass 1–12.
    fn month_name(&self, month: u32) -> Option<&'static str>;

    /// Relative-time vocabulary (text locale), the `< 45s` bucket:
    /// English `just now`, French `à l’instant`. Selection lives in
    /// [`format::Formats::rel_time`]; the number (when present) is grouped under
    /// the formatting locale.
    fn format_just_now(&self) -> &'static str;
    /// Relative-time vocabulary: `{n}` minutes ago. English `{n}m ago`, French
    /// `il y a {n} min`. `n` is pre-formatted under the formatting locale.
    fn format_minutes_ago(&self, n: &str) -> String;
    /// Relative-time vocabulary: `{n}` hours ago. English `{n}h ago`, French
    /// `il y a {n} h`.
    fn format_hours_ago(&self, n: &str) -> String;
    /// Relative-time vocabulary: `{n}` days ago. English `{n}d ago`, French
    /// `il y a {n} j`.
    fn format_days_ago(&self, n: &str) -> String;

    /// The status footer sentence: `client · {lifecycle}`.
    fn client_status(&self, lifecycle: &str) -> String;

    /// A room count for the rooms landmark's live-region announcement. Plural:
    /// the caller passes the count already formatted under the formatting locale
    /// and the [`PluralCategory`] computed under the text locale.
    fn rooms_count(&self, count_display: &str, category: PluralCategory) -> String;

    // ---- #178 global shell, onboarding, settings, recovery -----------------

    /// The global-destination navigation landmark's accessible name.
    fn nav_global_label(&self) -> &'static str;
    /// Global destination: the Rooms list.
    fn dest_rooms(&self) -> &'static str;
    /// Global destination: the Agent Fleet.
    fn dest_fleet(&self) -> &'static str;
    /// Global destination: Settings.
    fn dest_settings(&self) -> &'static str;

    /// Onboarding identity step: the title.
    fn onboarding_identity_title(&self) -> &'static str;
    /// Onboarding identity step: the explanatory body.
    fn onboarding_identity_body(&self) -> &'static str;
    /// Onboarding identity step: the "create identity" action.
    fn onboarding_create_identity(&self) -> &'static str;
    /// The label beside the shortened, copyable subject id.
    fn identity_id_label(&self) -> &'static str;
    /// The "copy" action for the subject id.
    fn identity_copy(&self) -> &'static str;
    /// The note that the identity is the unrecoverable P2P identity.
    fn identity_unrecoverable(&self) -> &'static str;

    /// Onboarding rooms step: the title.
    fn onboarding_rooms_title(&self) -> &'static str;
    /// Onboarding rooms step: the "create a room" action.
    fn onboarding_create_room(&self) -> &'static str;
    /// The room-name field label.
    fn room_name_label(&self) -> &'static str;
    /// Onboarding rooms step: the "join with a ticket" action.
    fn onboarding_join_room(&self) -> &'static str;
    /// The invite-ticket field label.
    fn ticket_label(&self) -> &'static str;
    /// Help text for the invite-ticket field.
    fn ticket_help(&self) -> &'static str;

    /// The self-label field label (shared by onboarding and settings).
    fn self_label_label(&self) -> &'static str;
    /// Help text for the self-label field ("on this device, never sent").
    fn self_label_help(&self) -> &'static str;

    /// The Settings destination heading.
    fn settings_heading(&self) -> &'static str;
    /// The Settings identity section heading.
    fn settings_identity_heading(&self) -> &'static str;
    /// The Settings language section heading.
    fn settings_language_heading(&self) -> &'static str;
    /// The text-locale switcher label.
    fn settings_text_locale_label(&self) -> &'static str;
    /// The formatting-locale switcher label.
    fn settings_formatting_locale_label(&self) -> &'static str;
    /// The "follow system" option in a locale switcher.
    fn settings_locale_follow_system(&self) -> &'static str;
    /// The honesty note that a browser preference applies this session only.
    fn settings_session_only_note(&self) -> &'static str;

    /// The Agent Fleet destination heading.
    fn fleet_heading(&self) -> &'static str;
    /// The Agent Fleet skeleton/loading placeholder.
    fn fleet_loading(&self) -> &'static str;

    /// The room-destination navigation strip's accessible name.
    fn room_nav_label(&self) -> &'static str;
    /// Room destination: Activity.
    fn room_dest_activity(&self) -> &'static str;
    /// Room destination: People.
    fn room_dest_people(&self) -> &'static str;
    /// Room destination: Agents.
    fn room_dest_agents(&self) -> &'static str;
    /// Room destination: Files.
    fn room_dest_files(&self) -> &'static str;
    /// Room destination: Pipes.
    fn room_dest_pipes(&self) -> &'static str;
    /// A per-destination skeleton placeholder shown while its content
    /// (#179–#181) is not yet built.
    fn room_dest_skeleton(&self) -> &'static str;
    /// The recoverable state for a route naming an unreachable/departed room:
    /// the plain fact, with Rooms as the way out.
    fn room_unavailable(&self) -> &'static str;

    /// The recovery banner title (corrupt/unsupported new-format state was reset).
    fn recovery_title(&self) -> &'static str;
    /// The recovery banner body: a plain "your local preferences were reset".
    fn recovery_body(&self) -> &'static str;
    /// The recovery banner's explicit "reset local preferences" action.
    fn recovery_reset_action(&self) -> &'static str;

    // ---- Onboarding operation errors ( Finding 1 — honest error surface ) ----

    /// The user-facing title when identity creation fails.
    fn err_onboarding_identity(&self) -> &'static str;
    /// The user-facing body when identity creation fails (retryable).
    fn err_onboarding_identity_body(&self) -> &'static str;

    /// The user-facing title when room creation fails.
    fn err_onboarding_room_create(&self) -> &'static str;
    /// The user-facing body when room creation fails (retryable).
    fn err_onboarding_room_create_body(&self) -> &'static str;

    // ---- #181 Files destination ------------------------------------------

    /// The Files destination heading.
    fn files_heading(&self) -> &'static str;
    /// The Files empty state — shown only after `file.list` has answered.
    fn files_empty(&self) -> &'static str;
    /// The Files loading state — shown before the first answer (unknown ≠ zero).
    fn files_loading(&self) -> &'static str;
    /// The "share a file" action.
    fn files_share_action(&self) -> &'static str;
    /// The "fetch" action (bring a file's bytes local).
    fn files_fetch_action(&self) -> &'static str;
    /// The "export / download" action.
    fn files_export_action(&self) -> &'static str;
    /// The opt-in, sandboxed preview action (inert types only).
    fn files_preview_action(&self) -> &'static str;
    /// The "cancel" action for an in-flight transfer.
    fn files_cancel_action(&self) -> &'static str;
    /// The "retry" action after a failed list/transfer.
    fn files_retry_action(&self) -> &'static str;
    /// The label for the peer-declared content type, marked untrusted (spec D6).
    fn files_declared_type_label(&self) -> &'static str;
    /// The provider-evidence section label.
    fn files_provider_label(&self) -> &'static str;
    /// The digest (short form) label.
    fn files_digest_label(&self) -> &'static str;
    /// Availability: a fetch can be served now.
    fn files_avail_fetchable(&self) -> &'static str;
    /// Availability: this device holds the bytes.
    fn files_avail_on_device(&self) -> &'static str;
    /// Availability: not currently fetchable (with provider evidence beside it).
    fn files_avail_not_fetchable(&self) -> &'static str;
    /// Placeholder shown for a file whose declared name failed validation
    /// (a hostile path/control name — spec D4); the real string is never shown.
    fn files_name_hidden(&self) -> &'static str;
    /// The read-only-archive note (share/fetch suppressed as a capability).
    fn files_read_only(&self) -> &'static str;
    /// In-flight upload progress label.
    fn files_uploading(&self) -> &'static str;
    /// In-flight fetch progress label.
    fn files_fetching(&self) -> &'static str;

    // ---- #181 Pipes destination ------------------------------------------

    /// The Pipes destination heading.
    fn pipes_heading(&self) -> &'static str;
    /// The Pipes empty state — shown only after `pipe.list` has answered.
    fn pipes_empty(&self) -> &'static str;
    /// The Pipes loading state — shown before the first answer.
    fn pipes_loading(&self) -> &'static str;
    /// Pipe state: a local connection is held.
    fn pipes_state_connected(&self) -> &'static str;
    /// Pipe state: published, nothing connected locally.
    fn pipes_state_open(&self) -> &'static str;
    /// Publisher reachability: a direct link.
    fn pipes_reach_direct(&self) -> &'static str;
    /// Publisher reachability: a relayed link.
    fn pipes_reach_relay(&self) -> &'static str;
    /// Publisher reachability: no link (with the reason).
    fn pipes_reach_unavailable(&self) -> &'static str;
    /// The "expose a pipe" action.
    fn pipes_expose_action(&self) -> &'static str;
    /// The "connect" action.
    fn pipes_connect_action(&self) -> &'static str;
    /// The "release" action (releases the local connection).
    fn pipes_release_action(&self) -> &'static str;
    /// The "revoke" action (owner-only; withdraws a published pipe).
    fn pipes_revoke_action(&self) -> &'static str;
    /// The loopback-host field label.
    fn pipes_target_label(&self) -> &'static str;
    /// The loopback-port field label.
    fn pipes_port_label(&self) -> &'static str;
    /// The audience field label.
    fn pipes_audience_label(&self) -> &'static str;
    /// Audience: any room member.
    fn pipes_audience_room(&self) -> &'static str;
    /// Audience: named subjects only.
    fn pipes_audience_subjects(&self) -> &'static str;
    /// The "published by" label.
    fn pipes_published_by_label(&self) -> &'static str;

    // ---- #181 Files/Pipes flow-failure bodies (spec §6) ------------------

    /// Over the served size limit — interpolates the *served* ceiling (spec D3);
    /// no baked number appears here. States that chunking is not offered.
    fn err_over_limit(&self, limit_display: &str) -> String;
    /// A share's bytes disagreed with its declared size (a size disagreement,
    /// never "corruption" — spec D3).
    fn err_size_mismatch(&self) -> &'static str;
    /// The picked file was empty.
    fn err_file_empty(&self) -> &'static str;
    /// No reachable provider could serve the file (spec D2).
    fn err_no_provider(&self) -> &'static str;
    /// Content did not verify — a genuine integrity failure (spec D3).
    fn err_digest_mismatch(&self) -> &'static str;
    /// No such file in this room.
    fn err_file_unknown(&self) -> &'static str;
    /// The file is not held locally — fetch first.
    fn err_not_fetched(&self) -> &'static str;
    /// A transfer stalled, exceeded its deadline, or was aborted mid-stream.
    fn err_transfer_interrupted(&self) -> &'static str;
    /// The publish target is not allowed (loopback only).
    fn err_pipe_target_refused(&self) -> &'static str;
    /// Publishing is not permitted in this room (distinct from a bad target).
    fn err_pipe_policy_refused(&self) -> &'static str;
    /// The publisher is offline — the distinctive `pipe_unreachable` (#94).
    fn err_pipe_unreachable(&self) -> &'static str;
    /// No such pipe (or outside its audience — deliberately one answer).
    fn err_pipe_unknown(&self) -> &'static str;
    /// The pipe was withdrawn.
    fn err_pipe_revoked(&self) -> &'static str;
    /// Not the pipe's publisher.
    fn err_pipe_not_publisher(&self) -> &'static str;
    /// No such local connection.
    fn err_connection_unknown(&self) -> &'static str;
    /// The room is not live — activate first.
    fn err_room_not_live(&self) -> &'static str;
    /// The file/pipe index could not be read.
    fn err_index_unreadable(&self) -> &'static str;
    /// The capability is not available in this browser.
    fn err_capability_unavailable(&self) -> &'static str;
    /// Permission was refused by the user or platform.
    fn err_capability_denied(&self) -> &'static str;
    /// A local file could not be read or written.
    fn err_source_unreadable(&self) -> &'static str;
}

/// Provide the resolved-locale context to a subtree and return its signal.
///
/// Called once by each per-target root (`WebRoot` / `NativeRoot`); descendants
/// read it through [`use_strings`] / [`use_formats`] / [`use_locale`]. The
/// returned signal is how a settings surface (a later slice) switches locale
/// live — an assignment re-renders every consumer.
pub fn use_locale_context(initial: LocaleState) -> Signal<LocaleState> {
    use_context_provider(|| Signal::new(initial))
}

/// The current resolved locale pair, or the default when no context is present
/// (an isolated component test), so a consumer never panics for lack of a
/// provider.
pub fn use_locale() -> LocaleState {
    match try_use_context::<Signal<LocaleState>>() {
        Some(signal) => signal(),
        None => LocaleState::default(),
    }
}

/// The catalog for the current **text** locale, resolved per render.
pub fn use_strings() -> &'static dyn Catalog {
    catalog_for(use_locale().text)
}

/// The formatting seam bound to the current (text vocabulary, formatting
/// convention) pair, resolved per render.
pub fn use_formats() -> Formats {
    let state = use_locale();
    Formats::new(state.text, state.formatting)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_tag_matches_on_primary_subtag() {
        assert_eq!(Locale::from_tag("en"), Some(Locale::En));
        assert_eq!(Locale::from_tag("fr"), Some(Locale::Fr));
        assert_eq!(Locale::from_tag("fr-CA"), Some(Locale::Fr));
        assert_eq!(Locale::from_tag("FR_be"), Some(Locale::Fr));
        assert_eq!(Locale::from_tag("de-CH"), None);
        assert_eq!(Locale::from_tag(""), None);
    }

    #[test]
    fn resolve_prefers_explicit_then_platform_then_fallback() {
        // Explicit text preference wins; formatting falls back to text.
        let s = LocaleState::resolve(Some("fr"), None, Some("en"), None);
        assert_eq!(s.text, Locale::Fr);
        assert_eq!(s.formatting, Locale::Fr);
        assert!(!s.text_follows_system);

        // No preference: follow platform, formatting independent of text.
        let s = LocaleState::resolve(None, Some("en"), Some("fr-CA"), None);
        assert_eq!(s.text, Locale::Fr);
        assert_eq!(s.formatting, Locale::En);
        assert!(s.text_follows_system);

        // Nothing resolves: English.
        let s = LocaleState::resolve(None, None, Some("de-CH"), Some("de-CH"));
        assert_eq!(s.text, Locale::En);
        assert_eq!(s.formatting, Locale::En);
    }

    #[test]
    fn independently_switchable_both_directions() {
        // text=fr, format=en
        let a = LocaleState::resolve(Some("fr"), Some("en"), None, None);
        assert_eq!((a.text, a.formatting), (Locale::Fr, Locale::En));
        // text=en, format=fr
        let b = LocaleState::resolve(Some("en"), Some("fr"), None, None);
        assert_eq!((b.text, b.formatting), (Locale::En, Locale::Fr));
    }

    #[test]
    fn every_supported_locale_has_a_catalog_and_matching_tag() {
        for locale in SUPPORTED_LOCALES {
            let catalog = catalog_for(locale);
            assert_eq!(catalog.locale_tag(), locale.tag());
            assert_eq!(catalog.app_name(), "Jeliya");
        }
    }
}
