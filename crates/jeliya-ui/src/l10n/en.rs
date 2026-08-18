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

    fn format_just_now(&self) -> &'static str {
        "just now"
    }
    fn format_minutes_ago(&self, n: &str) -> String {
        format!("{n}m ago")
    }
    fn format_hours_ago(&self, n: &str) -> String {
        format!("{n}h ago")
    }
    fn format_days_ago(&self, n: &str) -> String {
        format!("{n}d ago")
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

    fn nav_global_label(&self) -> &'static str {
        "Primary"
    }
    fn dest_rooms(&self) -> &'static str {
        "Rooms"
    }
    fn dest_fleet(&self) -> &'static str {
        "Agent Fleet"
    }
    fn dest_settings(&self) -> &'static str {
        "Settings"
    }

    fn onboarding_identity_title(&self) -> &'static str {
        "Create your identity"
    }
    fn onboarding_identity_body(&self) -> &'static str {
        "Your identity is a keypair on this device. There is no account and no password to recover."
    }
    fn onboarding_create_identity(&self) -> &'static str {
        "Create identity"
    }
    fn identity_id_label(&self) -> &'static str {
        "Your identity"
    }
    fn identity_copy(&self) -> &'static str {
        "Copy"
    }
    fn identity_unrecoverable(&self) -> &'static str {
        "This is your permanent peer-to-peer identity. It cannot be recovered if lost."
    }

    fn onboarding_rooms_title(&self) -> &'static str {
        "Start or join a room"
    }
    fn onboarding_create_room(&self) -> &'static str {
        "Create a room"
    }
    fn room_name_label(&self) -> &'static str {
        "Room name"
    }
    fn onboarding_join_room(&self) -> &'static str {
        "Join with a ticket"
    }
    fn ticket_label(&self) -> &'static str {
        "Invite ticket"
    }
    fn ticket_help(&self) -> &'static str {
        "Paste an invite ticket a room member shared with you."
    }

    fn self_label_label(&self) -> &'static str {
        "Your name"
    }
    fn self_label_help(&self) -> &'static str {
        "Shown on this device only. It is never sent to anyone."
    }

    fn settings_heading(&self) -> &'static str {
        "Settings"
    }
    fn settings_identity_heading(&self) -> &'static str {
        "Identity"
    }
    fn settings_language_heading(&self) -> &'static str {
        "Language"
    }
    fn settings_text_locale_label(&self) -> &'static str {
        "Interface language"
    }
    fn settings_formatting_locale_label(&self) -> &'static str {
        "Number and date format"
    }
    fn settings_locale_follow_system(&self) -> &'static str {
        "Follow system"
    }
    fn settings_session_only_note(&self) -> &'static str {
        "Applies this session — not saved on this browser."
    }

    fn fleet_heading(&self) -> &'static str {
        "Agent Fleet"
    }
    fn fleet_loading(&self) -> &'static str {
        "Agent Fleet is not available yet."
    }

    fn room_nav_label(&self) -> &'static str {
        "Room"
    }
    fn room_dest_activity(&self) -> &'static str {
        "Activity"
    }
    fn room_dest_people(&self) -> &'static str {
        "People"
    }
    fn room_dest_agents(&self) -> &'static str {
        "Agents"
    }
    fn room_dest_files(&self) -> &'static str {
        "Files"
    }
    fn room_dest_pipes(&self) -> &'static str {
        "Pipes"
    }
    fn room_dest_skeleton(&self) -> &'static str {
        "Nothing to show here yet."
    }
    fn room_unavailable(&self) -> &'static str {
        "This room isn’t available on this device. Return to Rooms."
    }

    fn recovery_title(&self) -> &'static str {
        "Local preferences reset"
    }
    fn recovery_body(&self) -> &'static str {
        "Some local preferences couldn’t be read and were reset to their defaults."
    }
    fn recovery_reset_action(&self) -> &'static str {
        "Reset local preferences"
    }

    fn err_onboarding_identity(&self) -> &'static str {
        "Couldn't create your identity"
    }
    fn err_onboarding_identity_body(&self) -> &'static str {
        "The daemon did not respond. Check your connection and try again."
    }

    fn err_onboarding_room_create(&self) -> &'static str {
        "Couldn't create the room"
    }
    fn err_onboarding_room_create_body(&self) -> &'static str {
        "The daemon did not respond. Check your connection and try again."
    }

    // ---- #180 truthful status vocabulary -----------------------------------

    fn room_open(&self) -> &'static str {
        "Open"
    }
    fn room_closed(&self) -> &'static str {
        "Closed"
    }
    fn reachability_connecting(&self) -> &'static str {
        "Connecting"
    }
    fn reachability_connected(&self) -> &'static str {
        "Connected"
    }
    fn reachability_alone(&self) -> &'static str {
        "No peers connected"
    }
    fn reachability_offline(&self) -> &'static str {
        "Offline"
    }
    fn liveness_working(&self) -> &'static str {
        "Working"
    }
    fn liveness_online(&self) -> &'static str {
        "Online"
    }
    fn liveness_offline(&self) -> &'static str {
        "Offline"
    }
    fn liveness_stale(&self) -> &'static str {
        "Stale"
    }
    fn status_label_online(&self) -> &'static str {
        "Online"
    }
    fn status_label_idle(&self) -> &'static str {
        "Idle"
    }
    fn status_label_claiming(&self) -> &'static str {
        "Claiming"
    }
    fn status_label_working(&self) -> &'static str {
        "Working"
    }
    fn status_label_done(&self) -> &'static str {
        "Done"
    }
    fn status_label_failed(&self) -> &'static str {
        "Failed"
    }
    fn status_label_blocked(&self) -> &'static str {
        "Needs a person"
    }
    fn redeemability_outstanding(&self) -> &'static str {
        "Outstanding"
    }
    fn redeemability_expired(&self) -> &'static str {
        "Expired"
    }
    fn redeemability_revoked(&self) -> &'static str {
        "Revoked"
    }
    fn redeemability_redeemed(&self) -> &'static str {
        "Redeemed"
    }
    fn link_reason_never_dialed(&self) -> &'static str {
        "never dialed"
    }
    fn link_reason_dial_failed(&self) -> &'static str {
        "dial failed"
    }
    fn link_reason_no_route(&self) -> &'static str {
        "no route"
    }
    fn link_reason_closed(&self) -> &'static str {
        "closed"
    }
    fn link_not_connected(&self) -> &'static str {
        "not connected"
    }
    fn status_none_yet(&self) -> &'static str {
        "No status yet"
    }
    fn last_seen_never(&self) -> &'static str {
        "Never seen"
    }
    fn self_you(&self) -> &'static str {
        "You"
    }

    // ---- #180 People pane --------------------------------------------------

    fn people_heading(&self) -> &'static str {
        "People"
    }
    fn people_presence_heading(&self) -> &'static str {
        "Presence"
    }
    fn presence_unavailable_closed(&self) -> &'static str {
        "Presence unavailable — the room session is Closed."
    }
    fn presence_absent(&self) -> &'static str {
        "not connected"
    }
    fn presence_unavailable(&self) -> &'static str {
        "presence unavailable"
    }
    fn people_roster_heading(&self) -> &'static str {
        "Members"
    }
    fn roster_role_label(&self) -> &'static str {
        "Role"
    }
    fn roster_standing_label(&self) -> &'static str {
        "Standing"
    }
    fn roster_joined_label(&self) -> &'static str {
        "Joined"
    }
    fn roster_presence_label(&self) -> &'static str {
        "Presence"
    }
    fn roster_agent_marker(&self) -> &'static str {
        "Agent"
    }
    fn self_this_device(&self) -> &'static str {
        "this device"
    }
    fn people_no_members(&self) -> &'static str {
        "No members yet."
    }
    fn people_invites_heading(&self) -> &'static str {
        "Invitations"
    }
    fn invites_empty(&self) -> &'static str {
        "No invitations."
    }
    fn invite_expires_label(&self) -> &'static str {
        "Expires"
    }
    fn invite_bound_label(&self) -> &'static str {
        "For"
    }
    fn action_invite(&self) -> &'static str {
        "Invite"
    }
    fn action_revoke(&self) -> &'static str {
        "Revoke"
    }
    fn action_remove(&self) -> &'static str {
        "Remove"
    }
    fn action_reinvite(&self) -> &'static str {
        "Re-invite"
    }
    fn action_leave(&self) -> &'static str {
        "Leave room"
    }
    fn action_activate(&self) -> &'static str {
        "Activate"
    }
    fn invite_capability_heading(&self) -> &'static str {
        "Invitation ticket"
    }
    fn invite_capability_note(&self) -> &'static str {
        "Shown once. Hand it to the invited person now — it is never saved."
    }
    fn invite_capability_copy(&self) -> &'static str {
        "Copy ticket"
    }

    // ---- #180 Invite form --------------------------------------------------

    fn invite_form_heading(&self) -> &'static str {
        "Issue an invitation"
    }
    fn invite_subject_label(&self) -> &'static str {
        "Identity to invite"
    }
    fn invite_subject_help(&self) -> &'static str {
        "Paste the person’s identity id, e.g. blake3:…"
    }
    fn invite_role_label(&self) -> &'static str {
        "Role"
    }
    fn invite_role_member_note(&self) -> &'static str {
        "Only member can be granted today."
    }
    fn invite_expiry_label(&self) -> &'static str {
        "Expires in"
    }
    fn invite_expiry_1h(&self) -> &'static str {
        "1 hour"
    }
    fn invite_expiry_1d(&self) -> &'static str {
        "1 day"
    }
    fn invite_expiry_7d(&self) -> &'static str {
        "7 days"
    }
    fn invite_submit(&self) -> &'static str {
        "Create invitation"
    }

    // ---- #180 Agents & Runs pane -------------------------------------------

    fn agents_heading(&self) -> &'static str {
        "Agents & runs"
    }
    fn agents_liveness_label(&self) -> &'static str {
        "Liveness"
    }
    fn agents_status_label(&self) -> &'static str {
        "Latest status"
    }
    fn agents_last_seen_label(&self) -> &'static str {
        "Last seen"
    }
    fn agents_empty(&self) -> &'static str {
        "No agents in this room yet."
    }
    fn run_history_open(&self) -> &'static str {
        "Run history"
    }
    fn run_history_heading(&self) -> &'static str {
        "Run history"
    }
    fn run_history_empty(&self) -> &'static str {
        "No runs recorded."
    }
    fn run_progress_label(&self) -> &'static str {
        "Progress"
    }

    // ---- #180 Agent Fleet pane ---------------------------------------------

    fn fleet_attention_needs_person(&self) -> &'static str {
        "Needs a person"
    }
    fn fleet_attention_failed(&self) -> &'static str {
        "Failed"
    }
    fn fleet_attention_ok(&self) -> &'static str {
        "OK"
    }
    fn fleet_room_label(&self) -> &'static str {
        "Room"
    }
    fn fleet_empty(&self) -> &'static str {
        "No agents yet."
    }
    fn fleet_filter_all(&self) -> &'static str {
        "All"
    }
    fn fleet_filter_live(&self) -> &'static str {
        "Live"
    }
    fn fleet_filter_label(&self) -> &'static str {
        "Filter agents"
    }

    // ---- #180 Settings (aliases + diagnostics) -----------------------------

    fn settings_aliases_heading(&self) -> &'static str {
        "Local names"
    }
    fn settings_alias_help(&self) -> &'static str {
        "On this device, never sent."
    }
    fn settings_alias_subject_label(&self) -> &'static str {
        "Name"
    }
    fn settings_alias_add_label(&self) -> &'static str {
        "Identity id"
    }
    fn settings_alias_add_help(&self) -> &'static str {
        "Paste an identity id to give it a local name."
    }
    fn settings_diagnostics_heading(&self) -> &'static str {
        "Diagnostics"
    }
    fn settings_diagnostics_state_label(&self) -> &'static str {
        "Connection"
    }
    fn settings_diagnostics_detail_label(&self) -> &'static str {
        "Last error"
    }
    fn settings_diagnostics_copy(&self) -> &'static str {
        "Copy diagnostics"
    }
    fn settings_diagnostics_redaction_note(&self) -> &'static str {
        "Identities are shortened and secrets removed."
    }

    // ---- #180 Destructive/sensitive confirmation ---------------------------

    fn confirm_cancel(&self) -> &'static str {
        "Cancel"
    }
    fn confirm_room_label(&self) -> &'static str {
        "Room"
    }
    fn confirm_remove_title(&self) -> &'static str {
        "Remove this member?"
    }
    fn confirm_remove_body(&self) -> &'static str {
        "This authors a signed removal every member converges on. It cannot be undone."
    }
    fn confirm_remove_confirm(&self) -> &'static str {
        "Remove member"
    }
    fn confirm_leave_title(&self) -> &'static str {
        "Leave this room?"
    }
    fn confirm_leave_body(&self) -> &'static str {
        "This authors a signed departure every member converges on. It cannot be undone."
    }
    fn confirm_leave_confirm(&self) -> &'static str {
        "Leave room"
    }
    fn confirm_revoke_title(&self) -> &'static str {
        "Revoke this invitation?"
    }
    fn confirm_revoke_body(&self) -> &'static str {
        "The ticket can no longer be redeemed after this."
    }
    fn confirm_revoke_confirm(&self) -> &'static str {
        "Revoke invitation"
    }

    // ---- #180 Read/mutation states -----------------------------------------

    fn state_loading(&self) -> &'static str {
        "Loading…"
    }
    fn state_offline(&self) -> &'static str {
        "Offline — reconnecting."
    }
    fn state_stale(&self) -> &'static str {
        "Showing the last known values."
    }
    fn state_unauthorized(&self) -> &'static str {
        "You aren’t allowed to see this."
    }
    fn state_failed_title(&self) -> &'static str {
        "Couldn’t load this"
    }
    fn state_failed_body(&self) -> &'static str {
        "Something went wrong. See Diagnostics for details."
    }
    fn load_show_more(&self) -> &'static str {
        "Show more"
    }
    fn couldnt_confirm(&self) -> &'static str {
        "Couldn’t confirm the result — reload to check."
    }
    fn activity_empty(&self) -> &'static str {
        "No activity yet."
    }
    fn activity_loading(&self) -> &'static str {
        "Loading activity…"
    }
    fn activity_resyncing(&self) -> &'static str {
        "Reconciling with the daemon…"
    }
    fn activity_recovering_loss(&self) -> &'static str {
        "Some updates were dropped — recovering…"
    }
    fn activity_new_messages(&self, count_display: &str, category: PluralCategory) -> String {
        match category {
            PluralCategory::One => format!("{count_display} new message"),
            PluralCategory::Other => format!("{count_display} new messages"),
        }
    }
    fn activity_new_activity(&self, count_display: &str, category: PluralCategory) -> String {
        match category {
            PluralCategory::One => format!("{count_display} new update"),
            PluralCategory::Other => format!("{count_display} new updates"),
        }
    }
    fn activity_departed(&self) -> &'static str {
        "You are no longer in this room. Its history stays here, read-only."
    }

    fn timeline_you(&self) -> &'static str {
        "You"
    }
    fn timeline_unresolved_sender(&self) -> &'static str {
        "Unknown sender"
    }
    fn timeline_agent_chip(&self) -> &'static str {
        "agent"
    }
    fn timeline_run_summary(&self, count_display: &str, category: PluralCategory) -> String {
        match category {
            PluralCategory::One => format!("{count_display} status update"),
            PluralCategory::Other => format!("{count_display} status updates"),
        }
    }

    fn filter_conversation(&self) -> &'static str {
        "Messages"
    }
    fn filter_agent_runs(&self) -> &'static str {
        "Agent runs"
    }
    fn filter_membership(&self) -> &'static str {
        "Membership"
    }
    fn filter_files(&self) -> &'static str {
        "Files"
    }
    fn filter_pipes(&self) -> &'static str {
        "Pipes"
    }

    fn sysline_room_created(&self, who: &str) -> String {
        format!("{who} created the room")
    }
    fn sysline_member_joined(&self, who: &str, role: &str) -> String {
        format!("{who} joined as {role}")
    }
    fn sysline_member_left(&self, who: &str) -> String {
        format!("{who} left")
    }
    fn sysline_member_removed(&self, who: &str, by: &str) -> String {
        format!("{who} was removed by {by}")
    }
    fn sysline_invite_revoked(&self) -> &'static str {
        "An invitation was revoked"
    }
    fn sysline_pipe_revoked(&self) -> &'static str {
        "A pipe was revoked"
    }
    fn file_open_in_files(&self) -> &'static str {
        "Open in Files"
    }
    fn pipe_open_in_pipes(&self) -> &'static str {
        "Open in Pipes"
    }

    fn composer_label(&self) -> &'static str {
        "Message"
    }
    fn composer_placeholder(&self) -> &'static str {
        "Write a message…"
    }
    fn composer_send(&self) -> &'static str {
        "Send"
    }
    fn composer_enter_hint(&self) -> &'static str {
        "Enter to send, Shift+Enter for a new line"
    }
    fn composer_attach(&self) -> &'static str {
        "Attach a file"
    }
    fn composer_attach_unavailable(&self) -> &'static str {
        "Attachments are not available yet."
    }
    fn composer_too_long(&self) -> &'static str {
        "Message too long — trimmed to the maximum length."
    }

    fn send_sending(&self) -> &'static str {
        "Sending…"
    }
    fn send_failed_not_sent(&self) -> &'static str {
        "Not sent"
    }
    fn send_failed_maybe(&self) -> &'static str {
        "May not have sent"
    }
    fn send_retry(&self) -> &'static str {
        "Retry"
    }

    fn archive_banner_left_title(&self) -> &'static str {
        "You left this room"
    }
    fn archive_banner_removed_title(&self) -> &'static str {
        "You were removed from this room"
    }
    fn archive_banner_body(&self) -> &'static str {
        "This is a local, read-only archive. Its history is shown as it was; it is not live and receives no new activity."
    }
    fn archive_banner_rejoin(&self) -> &'static str {
        "To rejoin, you need a new invite."
    }
    fn archive_timeline_label(&self) -> &'static str {
        "Archived timeline"
    }
    fn archive_roster_heading(&self) -> &'static str {
        "Members when you left"
    }
    fn archive_load_more(&self) -> &'static str {
        "Show earlier activity"
    }
    fn archive_still_active(&self) -> &'static str {
        "This room is active again. Open it from Rooms."
    }
    fn archive_empty(&self) -> &'static str {
        "No archived activity."
    }
    fn archive_loading(&self) -> &'static str {
        "Loading the archive…"
    }

    fn event_room_created(&self) -> &'static str {
        "Room created"
    }
    fn event_agent_status(&self) -> &'static str {
        "Agent status"
    }
    fn event_member_joined(&self) -> &'static str {
        "Member joined"
    }
    fn event_member_left(&self) -> &'static str {
        "Member left"
    }
    fn event_member_removed(&self) -> &'static str {
        "Member removed"
    }
    fn event_invite_revoked(&self) -> &'static str {
        "Invite revoked"
    }
    fn event_file_shared(&self) -> &'static str {
        "File shared"
    }
    fn event_pipe_published(&self) -> &'static str {
        "Pipe published"
    }
    fn event_pipe_revoked(&self) -> &'static str {
        "Pipe revoked"
    }
}
