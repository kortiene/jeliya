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

    // ---- #180 truthful status vocabulary (status/mod.rs seam) --------------

    /// Room session: the live room fact.
    fn room_open(&self) -> &'static str;
    /// Room session: the closed room fact.
    fn room_closed(&self) -> &'static str;

    /// Reachability: bringing transports up.
    fn reachability_connecting(&self) -> &'static str;
    /// Reachability: at least one peer link exists.
    fn reachability_connected(&self) -> &'static str;
    /// Reachability: live with no peers ("No peers connected" — never "Alone").
    fn reachability_alone(&self) -> &'static str;
    /// Reachability: not live.
    fn reachability_offline(&self) -> &'static str;

    /// Agent liveness: executing.
    fn liveness_working(&self) -> &'static str;
    /// Agent liveness: reachable and not executing.
    fn liveness_online(&self) -> &'static str;
    /// Agent liveness: not reachable.
    fn liveness_offline(&self) -> &'static str;
    /// Agent liveness: evidence too old to vouch for.
    fn liveness_stale(&self) -> &'static str;

    /// Status label: announced, not executing.
    fn status_label_online(&self) -> &'static str;
    /// Status label: not executing, ready.
    fn status_label_idle(&self) -> &'static str;
    /// Status label: in claim arbitration.
    fn status_label_claiming(&self) -> &'static str;
    /// Status label: executing.
    fn status_label_working(&self) -> &'static str;
    /// Status label: task succeeded.
    fn status_label_done(&self) -> &'static str;
    /// Status label: task failed.
    fn status_label_failed(&self) -> &'static str;
    /// Status label: stopped and needs a person ("needs a person").
    fn status_label_blocked(&self) -> &'static str;

    /// Invite redeemability: redeemable now.
    fn redeemability_outstanding(&self) -> &'static str;
    /// Invite redeemability: past its expiry.
    fn redeemability_expired(&self) -> &'static str;
    /// Invite redeemability: withdrawn by the authority.
    fn redeemability_revoked(&self) -> &'static str;
    /// Invite redeemability: already converted into membership.
    fn redeemability_redeemed(&self) -> &'static str;

    /// Link reason: no dial was ever attempted.
    fn link_reason_never_dialed(&self) -> &'static str;
    /// Link reason: a dial was attempted and failed.
    fn link_reason_dial_failed(&self) -> &'static str;
    /// Link reason: no route to the peer.
    fn link_reason_no_route(&self) -> &'static str;
    /// Link reason: the link was up and closed.
    fn link_reason_closed(&self) -> &'static str;
    /// The "not connected" lead for an absent per-device link.
    fn link_not_connected(&self) -> &'static str;

    /// Absent latest status ("no status yet") — never a fabricated liveness.
    fn status_none_yet(&self) -> &'static str;
    /// Absent last-seen ("never seen").
    fn last_seen_never(&self) -> &'static str;
    /// The unlabelled-self fallback name.
    fn self_you(&self) -> &'static str;

    // ---- #180 People pane --------------------------------------------------

    /// The People destination heading.
    fn people_heading(&self) -> &'static str;
    /// The presence-summary region heading.
    fn people_presence_heading(&self) -> &'static str;
    /// The honest note when a Closed room's presence is unavailable.
    fn presence_unavailable_closed(&self) -> &'static str;
    /// The per-member "no peer link" presence fact for a live room.
    fn presence_absent(&self) -> &'static str;
    /// The roster region heading.
    fn people_roster_heading(&self) -> &'static str;
    /// The roster role column label.
    fn roster_role_label(&self) -> &'static str;
    /// The roster standing column label.
    fn roster_standing_label(&self) -> &'static str;
    /// The roster joined-at column label.
    fn roster_joined_label(&self) -> &'static str;
    /// The roster per-member presence line label.
    fn roster_presence_label(&self) -> &'static str;
    /// The derived agent marker on a roster/agent row.
    fn roster_agent_marker(&self) -> &'static str;
    /// The "this device" marker beside the self row.
    fn self_this_device(&self) -> &'static str;
    /// The roster empty state (only after the answer).
    fn people_no_members(&self) -> &'static str;
    /// The invitations region heading.
    fn people_invites_heading(&self) -> &'static str;
    /// The invitations empty state.
    fn invites_empty(&self) -> &'static str;
    /// The label beside an invite's absolute expiry.
    fn invite_expires_label(&self) -> &'static str;
    /// The label beside an invite's bound identity.
    fn invite_bound_label(&self) -> &'static str;

    /// Action: issue an invitation.
    fn action_invite(&self) -> &'static str;
    /// Action: revoke an outstanding invitation.
    fn action_revoke(&self) -> &'static str;
    /// Action: remove a member.
    fn action_remove(&self) -> &'static str;
    /// Action: re-invite (mint fresh + revoke stale) after expiry.
    fn action_reinvite(&self) -> &'static str;
    /// Action: leave the room.
    fn action_leave(&self) -> &'static str;
    /// Action: activate a Closed room to read presence.
    fn action_activate(&self) -> &'static str;

    /// The once-shown minted-capability disclosure heading.
    fn invite_capability_heading(&self) -> &'static str;
    /// The once-shown minted-capability note (hand off now; never stored).
    fn invite_capability_note(&self) -> &'static str;
    /// The copy action for the minted capability.
    fn invite_capability_copy(&self) -> &'static str;

    // ---- #180 Invite form --------------------------------------------------

    /// The issue-invitation form heading.
    fn invite_form_heading(&self) -> &'static str;
    /// The subject-id field label.
    fn invite_subject_label(&self) -> &'static str;
    /// The subject-id field help/example (never pre-seeded with the self id).
    fn invite_subject_help(&self) -> &'static str;
    /// The role selector label.
    fn invite_role_label(&self) -> &'static str;
    /// The "member only; authority not grantable" note.
    fn invite_role_member_note(&self) -> &'static str;
    /// The expiry selector label.
    fn invite_expiry_label(&self) -> &'static str;
    /// Expiry option: one hour.
    fn invite_expiry_1h(&self) -> &'static str;
    /// Expiry option: one day.
    fn invite_expiry_1d(&self) -> &'static str;
    /// Expiry option: seven days.
    fn invite_expiry_7d(&self) -> &'static str;
    /// The issue-invitation submit action.
    fn invite_submit(&self) -> &'static str;

    // ---- #180 Agents & Runs pane -------------------------------------------

    /// The Agents & Runs destination heading.
    fn agents_heading(&self) -> &'static str;
    /// The agent-row liveness fact label.
    fn agents_liveness_label(&self) -> &'static str;
    /// The agent-row latest-status fact label.
    fn agents_status_label(&self) -> &'static str;
    /// The agent-row last-seen fact label.
    fn agents_last_seen_label(&self) -> &'static str;
    /// The Agents empty state (only after the answer).
    fn agents_empty(&self) -> &'static str;
    /// The run-history disclosure trigger.
    fn run_history_open(&self) -> &'static str;
    /// The run-history disclosure heading.
    fn run_history_heading(&self) -> &'static str;
    /// The run-history empty state.
    fn run_history_empty(&self) -> &'static str;
    /// The run-history progress column label.
    fn run_progress_label(&self) -> &'static str;

    // ---- #180 Agent Fleet pane ---------------------------------------------

    /// Fleet attention group: stopped and needs a person.
    fn fleet_attention_needs_person(&self) -> &'static str;
    /// Fleet attention group: work failed.
    fn fleet_attention_failed(&self) -> &'static str;
    /// Fleet attention group: nominal.
    fn fleet_attention_ok(&self) -> &'static str;
    /// The per-row room label in the fleet.
    fn fleet_room_label(&self) -> &'static str;
    /// The Fleet empty state (only after the answer).
    fn fleet_empty(&self) -> &'static str;
    /// The "all agents" filter option.
    fn fleet_filter_all(&self) -> &'static str;
    /// The "Live" filter option (Working + Online).
    fn fleet_filter_live(&self) -> &'static str;
    /// The Fleet filter group's accessible name.
    fn fleet_filter_label(&self) -> &'static str;

    // ---- #180 Settings (aliases + diagnostics) -----------------------------

    /// The device-local aliases section heading.
    fn settings_aliases_heading(&self) -> &'static str;
    /// The aliases help ("on this device, never sent").
    fn settings_alias_help(&self) -> &'static str;
    /// The alias-row identity label.
    fn settings_alias_subject_label(&self) -> &'static str;
    /// The alias editor's add-identity field label.
    fn settings_alias_add_label(&self) -> &'static str;
    /// The alias editor's add-identity help.
    fn settings_alias_add_help(&self) -> &'static str;
    /// The diagnostics section heading.
    fn settings_diagnostics_heading(&self) -> &'static str;
    /// The diagnostics client-state row label.
    fn settings_diagnostics_state_label(&self) -> &'static str;
    /// The diagnostics last-error row label.
    fn settings_diagnostics_detail_label(&self) -> &'static str;
    /// The "copy diagnostics" action.
    fn settings_diagnostics_copy(&self) -> &'static str;
    /// The diagnostics redaction note.
    fn settings_diagnostics_redaction_note(&self) -> &'static str;

    // ---- #180 Destructive/sensitive confirmation ---------------------------

    /// The confirm dialog's cancel (abandon) action — where initial focus lands.
    fn confirm_cancel(&self) -> &'static str;
    /// The confirm dialog's room disambiguator label.
    fn confirm_room_label(&self) -> &'static str;
    /// Remove-member confirm title.
    fn confirm_remove_title(&self) -> &'static str;
    /// Remove-member confirm body.
    fn confirm_remove_body(&self) -> &'static str;
    /// Remove-member confirm action.
    fn confirm_remove_confirm(&self) -> &'static str;
    /// Leave-room confirm title.
    fn confirm_leave_title(&self) -> &'static str;
    /// Leave-room confirm body.
    fn confirm_leave_body(&self) -> &'static str;
    /// Leave-room confirm action.
    fn confirm_leave_confirm(&self) -> &'static str;
    /// Revoke-invite confirm title.
    fn confirm_revoke_title(&self) -> &'static str;
    /// Revoke-invite confirm body.
    fn confirm_revoke_body(&self) -> &'static str;
    /// Revoke-invite confirm action.
    fn confirm_revoke_confirm(&self) -> &'static str;

    // ---- #180 Read/mutation states -----------------------------------------

    /// A read in flight (before the first answer).
    fn state_loading(&self) -> &'static str;
    /// A read interrupted by a transient disconnect (recoverable).
    fn state_offline(&self) -> &'static str;
    /// A shown value that could not be refreshed (possibly stale).
    fn state_stale(&self) -> &'static str;
    /// A read refused for authorization reasons.
    fn state_unauthorized(&self) -> &'static str;
    /// A terminal read-failure title.
    fn state_failed_title(&self) -> &'static str;
    /// A terminal read-failure body (no false recovery promise).
    fn state_failed_body(&self) -> &'static str;
    /// The bounded-page "show more" continuation.
    fn load_show_more(&self) -> &'static str;
    /// The ambiguous-mutation "couldn't confirm — reload to check" state.
    fn couldnt_confirm(&self) -> &'static str;
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
