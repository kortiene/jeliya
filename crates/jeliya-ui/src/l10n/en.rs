//! English — the source of truth (§4-D1). Reviewed as product copy.
//!
//! Every message is declared here first; [`super::Fr`] must implement the same
//! trait or the crate does not compile, which is how key and placeholder parity
//! are enforced by `rustc`. Apostrophes and ellipses use the typographic
//! characters (U+2019, U+2026) as house style, matching what the French
//! typography gate requires of the French catalog.

use super::{Catalog, PluralCategory};

/// The English catalog. A zero-sized dispatch target; [`super::catalog_for`]
/// hands out `&'static En`.
pub struct En;

impl Catalog for En {
    fn locale_tag(&self) -> &'static str {
        "en"
    }
    fn app_name(&self) -> &'static str {
        // The catalog gate (text-based) requires a string literal here; the
        // `tokens::BRAND` equality is enforced by a test (see `tokens.rs`), so a
        // brand change that does not update both fails CI.
        "Jeliya"
    }

    fn boot_connecting(&self) -> &'static str {
        "connecting to the local daemon…"
    }
    fn boot_stopping(&self) -> &'static str {
        "stopping — draining accepted work…"
    }
    fn boot_stopped(&self) -> &'static str {
        "stopped"
    }
    fn boot_failed(&self) -> &'static str {
        "the client failed and will not retry"
    }

    fn rooms_heading(&self) -> &'static str {
        "Rooms"
    }
    fn rooms_empty(&self) -> &'static str {
        "No rooms yet"
    }
    fn rooms_loading(&self) -> &'static str {
        "Loading rooms…"
    }
    fn room_untitled(&self) -> &'static str {
        "Untitled room"
    }
    fn center_choose_room(&self) -> &'static str {
        "Choose a room"
    }

    fn skip_to_rooms(&self) -> &'static str {
        "Skip to rooms"
    }
    fn skip_to_content(&self) -> &'static str {
        "Skip to main content"
    }

    fn diagnostics_open(&self) -> &'static str {
        "Diagnostics"
    }
    fn diagnostics_title(&self) -> &'static str {
        "Diagnostics"
    }
    fn diagnostics_close(&self) -> &'static str {
        "Close"
    }
    fn diagnostics_lifecycle_label(&self) -> &'static str {
        "Client lifecycle"
    }
    fn diagnostics_detail_label(&self) -> &'static str {
        "Last error detail"
    }
    fn diagnostics_no_detail(&self) -> &'static str {
        "No errors recorded."
    }

    fn err_room_list_title(&self) -> &'static str {
        "Couldn’t load rooms"
    }
    fn err_room_list_message(&self) -> &'static str {
        "The room list didn’t load. Jeliya will retry when the connection returns."
    }
    fn err_room_list_terminal_message(&self) -> &'static str {
        "The room list couldn’t load. Open Diagnostics for details."
    }
    fn err_unknown_title(&self) -> &'static str {
        "Something went wrong"
    }
    fn err_unknown_message(&self) -> &'static str {
        "An unexpected error occurred. See Diagnostics for details."
    }

    fn status_connecting(&self) -> &'static str {
        "Connecting"
    }
    fn status_ready(&self) -> &'static str {
        "Connected"
    }
    fn status_interrupted(&self) -> &'static str {
        "Reconnecting"
    }
    fn status_stopping(&self) -> &'static str {
        "Stopping"
    }
    fn status_stopped(&self) -> &'static str {
        "Stopped"
    }
    fn status_failed(&self) -> &'static str {
        "Disconnected"
    }

    fn conn_announcement(&self, status: &str) -> String {
        format!("Connection status: {status}")
    }

    fn wire_role_authority(&self) -> &'static str {
        "Authority"
    }
    fn wire_role_member(&self) -> &'static str {
        "Member"
    }
    fn wire_status_active(&self) -> &'static str {
        "Member"
    }
    fn wire_status_invited(&self) -> &'static str {
        "Invited"
    }
    fn wire_status_left(&self) -> &'static str {
        "Left"
    }
    fn wire_status_removed(&self) -> &'static str {
        "Removed"
    }
    fn wire_status_unknown(&self) -> &'static str {
        "Unknown"
    }
    fn wire_path_direct(&self) -> &'static str {
        // Tier-2 protocol token: rendered EXACTLY as the daemon reports it
        // (docs/glossary-fr.md), identical in every language.
        "direct"
    }
    fn wire_path_relay(&self) -> &'static str {
        "relay"
    }

    fn format_percent(&self, n: &str) -> String {
        format!("{n}%")
    }
    fn format_bytes_b(&self, n: &str) -> String {
        format!("{n} B")
    }
    fn format_bytes_kb(&self, n: &str) -> String {
        format!("{n} KB")
    }
    fn format_bytes_mb(&self, n: &str) -> String {
        format!("{n} MB")
    }
    fn format_bytes_gb(&self, n: &str) -> String {
        format!("{n} GB")
    }
    fn format_today(&self) -> &'static str {
        "Today"
    }
    fn format_yesterday(&self) -> &'static str {
        "Yesterday"
    }
    fn month_name(&self, month: u32) -> Option<&'static str> {
        match month {
            1 => Some("January"),
            2 => Some("February"),
            3 => Some("March"),
            4 => Some("April"),
            5 => Some("May"),
            6 => Some("June"),
            7 => Some("July"),
            8 => Some("August"),
            9 => Some("September"),
            10 => Some("October"),
            11 => Some("November"),
            12 => Some("December"),
            // Out-of-range guard: unreachable for a validated 1..=12 month (the
            // domain of the Gregorian calendar). `None`, not a panic — a total
            // function keeps a stray index from crashing a render.
            _ => None,
        }
    }

    fn client_status(&self, lifecycle: &str) -> String {
        format!(
            "client {dot} {lifecycle}",
            dot = crate::l10n::tokens::MIDDLE_DOT
        )
    }

    fn rooms_count(&self, count_display: &str, category: PluralCategory) -> String {
        match category {
            PluralCategory::One => format!("{count_display} room"),
            PluralCategory::Other => format!("{count_display} rooms"),
        }
    }
}
